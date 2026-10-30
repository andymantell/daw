// SPDX-License-Identifier: GPL-3.0-or-later
//! MIDI devices (roadmap 3.1): keyboards, pads and controllers in, and
//! synthesisers or other programs out, on every operating system.
//!
//! [`list`] names the ports. [`Midi::start`] opens every input port
//! (or the ones chosen) and sends what they play into the engine's live
//! queue, each event marked with its port; and opens every output port,
//! with a thread that takes the engine's events for MIDI outputs and
//! writes them to the port the track chose. Ports are numbered as the
//! list has them when started. If a device comes or goes, [`changed`]
//! says so, and the app starts again.
//!
//! Timing: an input event is given to the engine's next audio callback,
//! so it is heard within one buffer. Output events are written as soon
//! as the thread sees them, which is within a millisecond or so of the
//! callback that made them.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use daw_engine::midi::{Event, LiveEvent, OutEvent};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use rtrb::{Consumer, Producer};

/// The name this program gives itself to the MIDI system.
const CLIENT: &str = "DAW";

/// The ports there are now, by name, in the order they are numbered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ports {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

/// Lists the ports. Fails quietly to empty lists when the MIDI system
/// can't be reached (no sound server, say).
pub fn list() -> Ports {
    let mut p = Ports::default();
    if let Ok(input) = MidiInput::new(CLIENT) {
        p.inputs = (input.ports().iter())
            .filter_map(|port| input.port_name(port).ok())
            .collect();
    }
    if let Ok(output) = MidiOutput::new(CLIENT) {
        p.outputs = (output.ports().iter())
            .filter_map(|port| output.port_name(port).ok())
            .collect();
    }
    p
}

/// Whether the ports differ from `known`, which is when the app should
/// start MIDI again.
pub fn changed(known: &Ports) -> bool {
    list() != *known
}

/// Counts of what went through, for the device window.
#[derive(Default)]
pub struct Counts {
    /// Events received from inputs.
    pub received: AtomicU64,
    /// Events dropped because the engine's queue was full.
    pub dropped: AtomicU64,
    /// Events written to outputs.
    pub sent: AtomicU64,
    /// The last input port that played something, plus one; 0 for none.
    pub last_port: AtomicU64,
}

/// Open MIDI ports, running until dropped.
pub struct Midi {
    pub ports: Ports,
    /// Which input ports were opened (indices into `ports.inputs`).
    pub open_inputs: Vec<usize>,
    pub counts: Arc<Counts>,
    _inputs: Vec<MidiInputConnection<()>>,
    stop: Arc<AtomicBool>,
    out_thread: Option<JoinHandle<()>>,
}

impl Midi {
    /// Opens the inputs named in `use_inputs` (every input when empty)
    /// and every output. Inputs that can't be opened are skipped;
    /// `problems` says which.
    pub fn start(
        use_inputs: &[String],
        to_engine: Producer<LiveEvent>,
        from_engine: Consumer<OutEvent>,
    ) -> (Midi, Vec<String>) {
        let ports = list();
        let counts = Arc::new(Counts::default());
        let mut problems = Vec::new();
        let to_engine = Arc::new(Mutex::new(to_engine));
        let mut inputs = Vec::new();
        let mut open_inputs = Vec::new();
        for (i, name) in ports.inputs.iter().enumerate() {
            if !use_inputs.is_empty() && !use_inputs.contains(name) {
                continue;
            }
            let Ok(input) = MidiInput::new(CLIENT) else {
                problems.push(format!("{name}: the MIDI system can't be reached"));
                continue;
            };
            let Some(port) = (input.ports().into_iter())
                .find(|p| input.port_name(p).ok().as_deref() == Some(name))
            else {
                continue;
            };
            let (queue, counts_in) = (to_engine.clone(), counts.clone());
            let conn = input.connect(
                &port,
                "in",
                move |_stamp, bytes, _| {
                    let Some(event) = Event::from_midi(0, bytes) else {
                        return;
                    };
                    counts_in.received.fetch_add(1, Ordering::Relaxed);
                    counts_in.last_port.store(i as u64 + 1, Ordering::Relaxed);
                    if let Ok(mut q) = queue.lock()
                        && q.push(LiveEvent { port: i, event }).is_err()
                    {
                        counts_in.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                },
                (),
            );
            match conn {
                Ok(c) => {
                    inputs.push(c);
                    open_inputs.push(i);
                }
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
        let mut outputs: Vec<Option<MidiOutputConnection>> = Vec::new();
        for name in &ports.outputs {
            let conn = MidiOutput::new(CLIENT).ok().and_then(|output| {
                let port = (output.ports().into_iter())
                    .find(|p| output.port_name(p).ok().as_deref() == Some(name))?;
                match output.connect(&port, "out") {
                    Ok(c) => Some(c),
                    Err(e) => {
                        problems.push(format!("{name}: {e}"));
                        None
                    }
                }
            });
            outputs.push(conn);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let out_thread = {
            let (stop, counts) = (stop.clone(), counts.clone());
            std::thread::Builder::new()
                .name("midi-out".into())
                .spawn(move || output_main(from_engine, outputs, stop, counts))
                .ok()
        };
        (
            Midi {
                ports,
                open_inputs,
                counts,
                _inputs: inputs,
                stop,
                out_thread,
            },
            problems,
        )
    }
}

impl Drop for Midi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.out_thread.take() {
            let _ = t.join();
        }
    }
}

/// Writes the engine's events to the outputs until told to stop, then
/// silences every output.
fn output_main(
    mut from_engine: Consumer<OutEvent>,
    mut outputs: Vec<Option<MidiOutputConnection>>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
) {
    while !stop.load(Ordering::Relaxed) {
        let mut any = false;
        while let Ok(e) = from_engine.pop() {
            any = true;
            let Some(Some(out)) = outputs.get_mut(e.port) else {
                continue;
            };
            match e.event.to_midi() {
                Some(bytes) => {
                    if out.send(&bytes[..e.event.midi_len()]).is_ok() {
                        counts.sent.fetch_add(1, Ordering::Relaxed);
                    }
                }
                None => {
                    for bytes in Event::to_midi_all() {
                        let _ = out.send(&bytes);
                    }
                }
            }
        }
        if !any {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    for out in outputs.iter_mut().flatten() {
        for bytes in Event::to_midi_all() {
            let _ = out.send(&bytes);
        }
    }
}
