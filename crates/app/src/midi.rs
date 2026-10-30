// SPDX-License-Identifier: GPL-3.0-or-later
//! MIDI devices and recording notes in the app (roadmap 3.1).
//!
//! The devices are opened when the app starts and again whenever the
//! engine is made afresh or a device comes or goes. Every input plays
//! into every instrument track that is armed or monitoring, and is
//! recorded onto the armed ones while the transport records.
//!
//! Recording keeps the notes played in one pass (from when recording
//! started, or the loop last went round, to when it stopped or went
//! round again) and turns them into a note clip per armed track: a new
//! clip on top of what is there (overdub: the older clips keep
//! playing), or merged into the clip already under them. Notes played
//! while playing without Record are kept too, for the last little
//! while, so a good part played without Record on can still be kept
//! ([`App::keep_notes`]).

use std::collections::VecDeque;

use daw_audio_io::midi::{self, Ports};
use daw_engine::midi::{Captured, Kind};
use daw_engine::time::{Frames, TempoMap};
use daw_project::{Note, NoteClip, PPQ};
use eframe::egui;

use crate::App;

/// How notes recorded in a pass go onto the track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// A new clip each pass, over the clips there (they keep playing).
    #[default]
    NewClip,
    /// Into the clip already there under the notes, if there is one.
    Merge,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::NewClip => "New clip each pass",
            Mode::Merge => "Merge into the clip there",
        }
    }
}

/// Seconds of notes played without recording that are kept.
pub const KEEP_SECONDS: i64 = 120;

/// A note still held down during a pass.
struct Open {
    pitch: u8,
    channel: u8,
    velocity: u8,
    /// Timeline frame.
    at: i64,
}

/// A note played in a pass, in timeline frames.
#[derive(Clone, Copy)]
struct Played {
    pitch: u8,
    channel: u8,
    velocity: u8,
    start: i64,
    end: i64,
}

#[derive(Default)]
pub struct Midi {
    /// The open devices, while the MIDI system can be reached.
    running: Option<midi::Midi>,
    /// The ports when last looked, and when that was.
    pub ports: Ports,
    last_scan: Option<std::time::Instant>,
    /// Problems opening ports, for the device window.
    pub problems: Vec<String>,
    pub mode: Mode,
    /// The pass being recorded: the notes held down and the ones done.
    open: Vec<Open>,
    played: Vec<Played>,
    /// The last timeline frame seen while recording, to notice the loop
    /// going round, and whether a pass is under way.
    last_at: Option<i64>,
    /// Notes played while playing without recording, newest last.
    recent: VecDeque<Played>,
    recent_open: Vec<Open>,
    /// Passes recorded, for naming clips.
    passes: u32,
}

/// How often to look for devices that came or went.
const RESCAN: std::time::Duration = std::time::Duration::from_secs(3);

impl Midi {
    /// Opens the devices, with the engine's queues.
    pub fn start(&mut self, ctl: &mut daw_engine::engine::Controller) {
        self.running = None;
        self.problems.clear();
        if cfg!(test) {
            // Tests run without devices (and without a MIDI system).
            return;
        }
        if let (Some(to), Some(from)) = (ctl.take_midi_input(), ctl.take_midi_output()) {
            let (m, problems) = midi::Midi::start(&[], to, from);
            self.ports = m.ports.clone();
            self.problems = problems;
            self.running = Some(m);
        }
        self.last_scan = Some(std::time::Instant::now());
    }

    /// Whether the devices changed since they were opened, looked at
    /// now and then. True means the app should make its engine's MIDI
    /// side again.
    pub fn changed(&mut self) -> bool {
        if cfg!(test) || self.running.is_none() {
            return false;
        }
        if self.last_scan.is_some_and(|t| t.elapsed() < RESCAN) {
            return false;
        }
        self.last_scan = Some(std::time::Instant::now());
        midi::changed(&self.ports)
    }

    pub fn counts(&self) -> Option<&midi::Counts> {
        self.running.as_ref().map(|m| &*m.counts)
    }

    /// Takes in what the engine saw. Returns the notes of any pass
    /// that finished: as timeline frames.
    fn take(&mut self, captured: &[Captured], rate: u32) -> Vec<Vec<Played>> {
        let mut done = Vec::new();
        for c in captured {
            if c.recording {
                // The loop went round: the pass so far is done.
                if self.last_at.is_some_and(|last| c.at < last) {
                    done.push(self.finish_pass(c.at));
                }
                self.last_at = Some(c.at);
                note(&mut self.open, &mut self.played, c);
            } else {
                if self.last_at.is_some() {
                    done.push(self.finish_pass(c.at));
                }
                if c.playing {
                    note(&mut self.recent_open, &mut self.recent, c);
                    let keep = KEEP_SECONDS * rate as i64;
                    while let Some(f) = self.recent.front()
                        && f.end < c.at - keep
                    {
                        self.recent.pop_front();
                    }
                } else {
                    // Stopped: notes still down end here.
                    for o in self.recent_open.drain(..) {
                        self.recent.push_back(Played {
                            pitch: o.pitch,
                            channel: o.channel,
                            velocity: o.velocity,
                            start: o.at,
                            end: c.at.max(o.at + 1),
                        });
                    }
                }
            }
        }
        done.retain(|p| !p.is_empty());
        done
    }

    /// Ends the pass at `at`: notes still held end there.
    fn finish_pass(&mut self, at: i64) -> Vec<Played> {
        self.last_at = None;
        for o in self.open.drain(..) {
            self.played.push(Played {
                pitch: o.pitch,
                channel: o.channel,
                velocity: o.velocity,
                start: o.at,
                end: at.max(o.at + 1),
            });
        }
        std::mem::take(&mut self.played)
    }

    /// Whether anything played without recording is kept.
    pub fn has_recent(&self) -> bool {
        !self.recent.is_empty() || !self.recent_open.is_empty()
    }
}

/// Applies one event to the notes held and the notes done.
fn note(open: &mut Vec<Open>, played: &mut impl Extend<Played>, c: &Captured) {
    match c.event.kind {
        Kind::NoteOn {
            pitch, velocity, ..
        } => {
            open.push(Open {
                pitch,
                channel: c.event.channel,
                velocity,
                at: c.at,
            });
        }
        Kind::NoteOff { pitch, .. } => {
            if let Some(i) =
                (open.iter()).position(|o| o.pitch == pitch && o.channel == c.event.channel)
            {
                let o = open.remove(i);
                played.extend([Played {
                    pitch: o.pitch,
                    channel: o.channel,
                    velocity: o.velocity,
                    start: o.at,
                    end: c.at.max(o.at + 1),
                }]);
            }
        }
        Kind::AllNotesOff => {
            for o in open.drain(..) {
                played.extend([Played {
                    pitch: o.pitch,
                    channel: o.channel,
                    velocity: o.velocity,
                    start: o.at,
                    end: c.at.max(o.at + 1),
                }]);
            }
        }
        _ => {}
    }
}

/// Turns a pass's notes into notes in ticks, with the clip they go in:
/// from the start of the bar the first note is in to the end of the bar
/// the last ends in.
fn to_clip(map: &TempoMap, played: &[Played], name: String) -> NoteClip {
    let first = played.iter().map(|p| p.start).min().unwrap_or(0);
    let last = played.iter().map(|p| p.end).max().unwrap_or(0);
    let first_t = map.ticks_at(Frames(first));
    let start = map.bar_start(map.bar_beat(first_t).bar);
    let last_t = map.ticks_at(Frames(last));
    let end_bar = map.bar_beat(last_t).bar + 1;
    let end = map.bar_start(end_bar);
    let mut clip = NoteClip::new(
        daw_project::new_id(),
        name,
        start.0,
        (end.0 - start.0).max(PPQ),
    );
    for p in played {
        let s = map.ticks_at(Frames(p.start)).0 - start.0;
        let e = map.ticks_at(Frames(p.end)).0 - start.0;
        let mut n = Note::new(s, (e - s).max(PPQ / 32), p.pitch, p.velocity);
        n.channel = p.channel;
        clip.notes.push(n);
    }
    clip.sort_notes();
    clip
}

/// Puts `clip` on `clips`: on top as a new clip, or merged into one
/// already there that it overlaps (the latest), which grows to hold it.
fn place(clips: &mut Vec<NoteClip>, mut clip: NoteClip, mode: Mode) {
    if mode == Mode::Merge
        && let Some(into) = (clips.iter_mut().rev())
            .find(|c| !c.muted && c.start < clip.end() && clip.start < c.end())
    {
        let shift = clip.start - (into.start - into.offset);
        for mut n in clip.notes.drain(..) {
            n.start += shift;
            into.notes.push(n);
        }
        if clip.end() > into.end() {
            into.len = clip.end() - into.start;
        }
        if clip.start < into.start {
            // Grow leftwards: the content start stays where it was.
            let by = into.start - clip.start;
            into.start = clip.start;
            into.len += by;
            into.offset -= by;
            if into.offset < 0 {
                let fix = -into.offset;
                into.offset = 0;
                for n in &mut into.notes {
                    n.start += fix;
                }
            }
        }
        into.sort_notes();
        return;
    }
    clips.push(clip);
}

impl App {
    /// Takes in the notes the engine saw since last frame: finished
    /// passes become clips on the armed instrument tracks. True if the
    /// tracks changed.
    pub(crate) fn finish_notes(&mut self) -> bool {
        let mut captured = Vec::new();
        self.ctl.take_captured(&mut captured);
        self.kits.hear(&captured, &self.midi.ports.inputs);
        if captured.is_empty() {
            return false;
        }
        let learned = self.midi_learn(&captured);
        let passes = self.midi.take(&captured, self.rate);
        if passes.is_empty() {
            return learned;
        }
        let map = self.tempo_map();
        let mode = self.midi.mode;
        let mut any = false;
        for played in passes {
            self.midi.passes += 1;
            let n = self.midi.passes;
            for t in (self.tracks.iter_mut()).filter(|t| t.kind.plays_notes() && t.arm) {
                let mut clip = to_clip(&map, &played, format!("Take {n}"));
                clip.take = Some(n - 1);
                place(&mut t.note_clips, clip, mode);
                any = true;
            }
        }
        any
    }

    /// Makes clips from the notes played without recording, on the
    /// armed instrument tracks (or those monitoring, when none is
    /// armed). True if any were made.
    pub(crate) fn keep_notes(&mut self) -> bool {
        let played: Vec<Played> = self.midi.recent.drain(..).collect();
        if played.is_empty() {
            return false;
        }
        let map = self.tempo_map();
        let mode = self.midi.mode;
        let armed = self.tracks.iter().any(|t| t.kind.plays_notes() && t.arm);
        let mut any = false;
        for t in (self.tracks.iter_mut())
            .filter(|t| t.kind.plays_notes() && if armed { t.arm } else { t.monitor })
        {
            let clip = to_clip(&map, &played, "Kept".into());
            place(&mut t.note_clips, clip, mode);
            any = true;
        }
        any
    }

    /// The MIDI part of the device window: the ports, and whether
    /// anything is coming in.
    pub(crate) fn midi_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("MIDI");
        if self.midi.ports.inputs.is_empty() && self.midi.ports.outputs.is_empty() {
            ui.label("No MIDI devices were found. They are looked for every few seconds.");
        }
        let last = self.midi.counts().map_or(0, |c| {
            c.last_port.load(std::sync::atomic::Ordering::Relaxed)
        });
        if !self.midi.ports.inputs.is_empty() {
            ui.label("Inputs (all are played into armed or monitoring instrument tracks):");
            for (i, name) in self.midi.ports.inputs.iter().enumerate() {
                let playing = last == i as u64 + 1;
                ui.horizontal(|ui| {
                    ui.label(format!("• {name}"));
                    if playing {
                        ui.label(egui::RichText::new("last played").weak());
                    }
                });
            }
        }
        if !self.midi.ports.outputs.is_empty() {
            ui.label("Outputs (chosen per track in its instrument window):");
            for name in &self.midi.ports.outputs {
                ui.label(format!("• {name}"));
            }
        }
        if let Some(c) = self.midi.counts() {
            use std::sync::atomic::Ordering::Relaxed;
            ui.label(
                egui::RichText::new(format!(
                    "{} received, {} sent, {} dropped",
                    c.received.load(Relaxed),
                    c.sent.load(Relaxed),
                    c.dropped.load(Relaxed)
                ))
                .weak(),
            );
        }
        for p in &self.midi.problems {
            ui.colored_label(ui.visuals().warn_fg_color, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::midi::Event;
    use daw_engine::time::{Tempo, TimeSignature};

    fn map() -> TempoMap {
        TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON)
    }

    fn cap(at: i64, event: Event, recording: bool) -> Captured {
        Captured {
            port: 0,
            event,
            at,
            playing: true,
            recording,
        }
    }

    #[test]
    fn a_pass_ends_when_recording_stops_or_the_loop_goes_round() {
        let mut m = Midi::default();
        let beat = 24_000;
        // Recording: a note, then the loop goes round (the frame goes
        // back), a second note, then stop.
        let passes = m.take(
            &[
                cap(beat, Event::note_on(0, 0, 60, 100, 0), true),
                cap(2 * beat, Event::note_off(0, 0, 60, 0), true),
                cap(beat / 2, Event::note_on(0, 0, 62, 90, 0), true),
                cap(beat, Event::note_off(0, 0, 62, 0), true),
                Captured {
                    playing: false,
                    ..cap(beat, Event::note_on(0, 0, 64, 90, 0), false)
                },
            ],
            48_000,
        );
        assert_eq!(passes.len(), 2);
        assert_eq!(passes[0][0].pitch, 60);
        assert_eq!((passes[0][0].start, passes[0][0].end), (beat, 2 * beat));
        assert_eq!(passes[1][0].pitch, 62);
        let clip = to_clip(&map(), &passes[0], "Take 1".into());
        // From the bar's start, one bar long; the note on beat 2.
        assert_eq!((clip.start, clip.len), (0, 4 * PPQ));
        assert_eq!((clip.notes[0].start, clip.notes[0].len), (PPQ, PPQ));
    }

    #[test]
    fn notes_played_without_recording_are_kept_for_a_while() {
        let mut m = Midi::default();
        let s = 48_000;
        m.take(
            &[
                cap(0, Event::note_on(0, 0, 60, 100, 0), false),
                cap(s, Event::note_off(0, 0, 60, 0), false),
                cap(200 * s, Event::note_on(0, 0, 62, 100, 0), false),
                cap(201 * s, Event::note_off(0, 0, 62, 0), false),
            ],
            48_000,
        );
        let kept: Vec<u8> = m.recent.iter().map(|p| p.pitch).collect();
        assert_eq!(kept, [62], "the first was played too long ago");
        assert!(m.has_recent());
    }

    #[test]
    fn merging_puts_notes_into_the_clip_there_and_grows_it() {
        let mut clips = vec![NoteClip::new("a".into(), "A".into(), PPQ * 4, PPQ * 4)];
        let mut new = NoteClip::new("b".into(), "B".into(), PPQ * 6, PPQ * 4);
        new.notes.push(Note::new(PPQ, PPQ, 60, 100));
        place(&mut clips, new.clone(), Mode::Merge);
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].start, clips[0].len), (PPQ * 4, PPQ * 6));
        // The note keeps its place on the timeline: beat 7 from the
        // clip's content start at beat 4.
        assert_eq!(clips[0].notes[0].start, PPQ * 3);
        place(&mut clips, new, Mode::NewClip);
        assert_eq!(clips.len(), 2);
    }
}
