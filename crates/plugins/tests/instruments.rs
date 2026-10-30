// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin instruments (roadmap 3.2) in both formats: notes reach the
//! plugin on their frame, controllers reach it as the plugin maps them,
//! its main output adds to the track, and an extra output goes to the
//! bus that takes it or else to the main one. Each runs in the app's
//! own process and in a helper process, as the app may.

#[path = "common/bundle.rs"]
mod bundle;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_engine::midi::{Event, Instrument, Kind, Outputs};
use daw_plugins::clap::cid_of;
use daw_plugins::host::{Host, RemoteProcessor};
use daw_plugins::{Layout, Library, Plugin, Processor};
use daw_test_plugin::{CLAP_SYNTH_ID, GAIN_CONTROLLER, SYNTH_CID};
use vst3::Steinberg::TUID;

const RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

fn helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_daw-plugin-helper"))
}

/// The test instrument in each format: its file and class.
fn formats() -> [(&'static str, PathBuf, TUID); 2] {
    [
        ("VST 3", bundle::bundle(), SYNTH_CID),
        ("CLAP", bundle::clap(), cid_of(CLAP_SYNTH_ID)),
    ]
}

/// Its extra "Drums" output switched on.
fn drums() -> Layout {
    Layout {
        sidechain: false,
        outputs: vec![1],
    }
}

/// The instrument's two sides, in this process or in a helper (live,
/// one block behind, or for rendering).
fn open(
    file: &Path,
    cid: &TUID,
    remote: Option<Option<usize>>,
) -> (Plugin, Processor, Option<Host>) {
    match remote {
        Some(live) => {
            let host = Host::spawn(helper()).unwrap();
            let (p, x) = host
                .instantiate_instrument(file, cid, RATE, BLOCK, live, &drums())
                .unwrap();
            (Plugin::Remote(p), Processor::Remote(x), Some(host))
        }
        None => {
            let lib = Library::load(file).unwrap();
            let (p, x) = lib.instantiate(cid, RATE, BLOCK, &drums()).unwrap();
            (p, x, None)
        }
    }
}

/// One block: what the track had (`under` everywhere) with the
/// instrument added, and output 1 if `take` asks for it.
fn play(inst: &mut dyn Instrument, events: &[Event], under: f32, take: bool) -> [Vec<f32>; 2] {
    let (mut l, mut r) = (vec![under; BLOCK], vec![under; BLOCK]);
    // The graph hands over each output silent.
    let mut pairs = vec![[vec![0.0; BLOCK], vec![0.0; BLOCK]]];
    let routed = [take];
    let mut outs = Outputs::new(&mut pairs, &routed, BLOCK);
    inst.process_outputs(events, &mut l, &mut r, &mut outs);
    assert_eq!(l, r);
    let [ol, or] = pairs.pop().unwrap();
    assert_eq!(ol, or);
    if !take {
        assert!(marks(&ol, 0.0).is_empty());
    }
    [l, ol]
}

/// The frames of `x` that are not `base`, with their values.
fn marks(x: &[f32], base: f32) -> Vec<(usize, f32)> {
    (x.iter().enumerate())
        .filter(|(_, v)| (**v - base).abs() > 1e-6)
        .map(|(i, v)| (i, *v - base))
        .collect()
}

fn notes() -> Vec<Event> {
    vec![
        Event::note_on(10, 0, 60, 127, 0),
        Event::note_on(20, 9, 36, 127, 1),
        Event::note_off(30, 0, 60, 0),
    ]
}

#[test]
fn notes_play_on_their_frame_and_an_extra_output_goes_where_it_is_taken() {
    for (format, file, cid) in formats() {
        for remote in [None, Some(None)] {
            let what = format!("{format}, in a helper: {}", remote.is_some());
            let (plugin, mut inst, _host) = open(&file, &cid, remote);
            let buses = plugin.buses();
            assert_eq!(buses.outputs.len(), 2, "{what}");
            assert_eq!(buses.outputs[1].name, "Drums", "{what}");
            assert!(buses.outputs[1].aux, "{what}");

            // Taken: the drum note goes to its own output.
            let [main, drum] = play(&mut inst, &notes(), 0.25, true);
            assert_eq!(marks(&main, 0.25), vec![(10, 1.0)], "{what}");
            assert_eq!(marks(&drum, 0.0), vec![(20, 1.0)], "{what}");

            // Not taken: both are heard on the track.
            let [main, _] = play(&mut inst, &notes(), 0.0, false);
            assert_eq!(marks(&main, 0.0), vec![(10, 1.0), (20, 1.0)], "{what}");

            // Every note stops at once without a sound of its own.
            let [main, _] = play(&mut inst, &[Event::all_notes_off(0)], 0.0, false);
            assert!(marks(&main, 0.0).is_empty(), "{what}");
        }
    }
}

#[test]
fn a_controller_reaches_the_parameter_the_plugin_maps_it_to() {
    let volume = Event {
        frame: 0,
        channel: 0,
        kind: Kind::Control {
            number: GAIN_CONTROLLER,
            value: 64,
        },
    };
    for (format, file, cid) in formats() {
        for remote in [None, Some(None)] {
            let what = format!("{format}, in a helper: {}", remote.is_some());
            let (_plugin, mut inst, _host) = open(&file, &cid, remote);
            play(&mut inst, &[volume], 0.0, false);
            let [main, _] = play(&mut inst, &notes()[..1], 0.0, false);
            let got = marks(&main, 0.0);
            assert_eq!(got.len(), 1, "{what}");
            assert!((got[0].1 - 64.0 / 127.0).abs() < 1e-3, "{what}: {got:?}");
        }
    }
}

/// Waits until a live instrument in a helper has had `frames` answered.
fn answered(inst: &Processor, frames: usize) {
    let Processor::Remote(r @ RemoteProcessor::Live(_)) = inst else {
        panic!("not live in a helper")
    };
    let start = Instant::now();
    while r.available() < frames {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the helper never answered"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_live_instrument_in_a_helper_plays_one_block_behind_and_says_so() {
    for (format, file, cid) in formats() {
        let (plugin, mut inst, _host) = open(&file, &cid, Some(Some(BLOCK)));
        assert_eq!(plugin.latency(), BLOCK as u32, "{format}");
        // The first block plays the silence queued ahead of it.
        let [main, drum] = play(&mut inst, &notes(), 0.0, true);
        assert!(marks(&main, 0.0).is_empty(), "{format}");
        assert!(marks(&drum, 0.0).is_empty(), "{format}");
        answered(&inst, BLOCK);
        let [main, drum] = play(&mut inst, &[], 0.0, true);
        assert_eq!(marks(&main, 0.0), vec![(10, 1.0)], "{format}");
        assert_eq!(marks(&drum, 0.0), vec![(20, 1.0)], "{format}");
    }
}
