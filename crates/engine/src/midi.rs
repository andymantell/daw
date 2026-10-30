// SPDX-License-Identifier: GPL-3.0-or-later
//! Notes as the engine passes them around (roadmap 3.1): the events an
//! instrument is given with each block, and the instrument itself.
//!
//! An [`Event`] is one thing that happens at one frame of a block: a
//! note starting or stopping, a controller moving, the pitch wheel, or
//! pressure. Events for a block come sorted by frame. Instruments run
//! on the audio thread, so like [`Insert`](crate::graph::Insert) they
//! must never allocate, lock or block.
//!
//! Live input arrives from MIDI devices with a time of its own; the
//! engine lines it up to the block it is played in (see `engine.rs`).
//! Notes from clips are worked out from the tempo map, so an instrument
//! sees the same events whichever way a note reached it.

/// One thing that happens at a frame of a block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    /// The frame within the block, from 0.
    pub frame: u32,
    /// 0 to 15.
    pub channel: u8,
    pub kind: Kind,
}

/// What an event is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A note starts: pitch 0 to 127 (60 is middle C), velocity 1 to
    /// 127. `id` tells a note from another at the same pitch (the note's
    /// index in its clip, or a count for live notes), so an instrument
    /// that plays one voice per note can match the note-off to it.
    NoteOn { pitch: u8, velocity: u8, id: u32 },
    /// A note stops, with the release velocity (0 when unknown).
    NoteOff { pitch: u8, velocity: u8, id: u32 },
    /// A controller moved: number and value, both 0 to 127.
    Control { number: u8, value: u8 },
    /// The pitch wheel: −8192 to 8191, 0 at rest.
    PitchBend(i16),
    /// Pressure on the whole keyboard, 0 to 127.
    Pressure(u8),
    /// Pressure on one key, 0 to 127.
    KeyPressure { pitch: u8, value: u8 },
    /// A program (sound) chosen, 0 to 127.
    Program(u8),
    /// Every note stops at once (the transport stopped, say), with
    /// controllers left as they are.
    AllNotesOff,
}

impl Event {
    pub fn note_on(frame: u32, channel: u8, pitch: u8, velocity: u8, id: u32) -> Event {
        Event {
            frame,
            channel,
            kind: Kind::NoteOn {
                pitch,
                velocity,
                id,
            },
        }
    }

    pub fn note_off(frame: u32, channel: u8, pitch: u8, id: u32) -> Event {
        Event {
            frame,
            channel,
            kind: Kind::NoteOff {
                pitch,
                velocity: 0,
                id,
            },
        }
    }

    pub fn all_notes_off(frame: u32) -> Event {
        Event {
            frame,
            channel: 0,
            kind: Kind::AllNotesOff,
        }
    }

    /// The event as MIDI 1.0 bytes, for a MIDI output; `None` for one
    /// that is not a MIDI message ([`Kind::AllNotesOff`] becomes the
    /// controller message of that name, on every channel, so it is
    /// returned once per channel through [`Event::to_midi_all`]).
    pub fn to_midi(&self) -> Option<[u8; 3]> {
        let ch = self.channel & 0x0f;
        Some(match self.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } => [0x90 | ch, pitch & 0x7f, velocity.clamp(1, 127)],
            Kind::NoteOff {
                pitch, velocity, ..
            } => [0x80 | ch, pitch & 0x7f, velocity & 0x7f],
            Kind::Control { number, value } => [0xb0 | ch, number & 0x7f, value & 0x7f],
            Kind::PitchBend(v) => {
                let v = (v.clamp(-8192, 8191) as i32 + 8192) as u16;
                [0xe0 | ch, (v & 0x7f) as u8, (v >> 7) as u8]
            }
            Kind::Pressure(v) => [0xd0 | ch, v & 0x7f, 0],
            Kind::KeyPressure { pitch, value } => [0xa0 | ch, pitch & 0x7f, value & 0x7f],
            Kind::Program(p) => [0xc0 | ch, p & 0x7f, 0],
            Kind::AllNotesOff => return None,
        })
    }

    /// How many bytes of [`to_midi`](Self::to_midi) carry the message:
    /// 2 for pressure and program, 3 for the rest.
    pub fn midi_len(&self) -> usize {
        match self.kind {
            Kind::Pressure(_) | Kind::Program(_) => 2,
            _ => 3,
        }
    }

    /// "All notes off" (controller 123) on each of the 16 channels.
    pub fn to_midi_all() -> impl Iterator<Item = [u8; 3]> {
        (0..16u8).map(|ch| [0xb0 | ch, 123, 0])
    }

    /// An event from MIDI 1.0 bytes (a status byte and its data), at
    /// `frame`; `None` for a message that is not one of the kinds here
    /// (clock, system exclusive) or that is malformed. A note-on with
    /// velocity 0 is a note-off, as MIDI has it. Live notes get `id` 0;
    /// the caller sets one if it needs to tell notes apart.
    pub fn from_midi(frame: u32, bytes: &[u8]) -> Option<Event> {
        let &status = bytes.first()?;
        let channel = status & 0x0f;
        let d1 = bytes.get(1).copied().unwrap_or(0) & 0x7f;
        let d2 = bytes.get(2).copied().unwrap_or(0) & 0x7f;
        let kind = match status & 0xf0 {
            0x80 => Kind::NoteOff {
                pitch: d1,
                velocity: d2,
                id: 0,
            },
            0x90 if d2 == 0 => Kind::NoteOff {
                pitch: d1,
                velocity: 0,
                id: 0,
            },
            0x90 => Kind::NoteOn {
                pitch: d1,
                velocity: d2,
                id: 0,
            },
            0xa0 => Kind::KeyPressure {
                pitch: d1,
                value: d2,
            },
            0xb0 if d1 == 123 => Kind::AllNotesOff,
            0xb0 => Kind::Control {
                number: d1,
                value: d2,
            },
            0xc0 => Kind::Program(d1),
            0xd0 => Kind::Pressure(d1),
            0xe0 => Kind::PitchBend((d1 as i16 | ((d2 as i16) << 7)) - 8192),
            _ => return None,
        };
        Some(Event {
            frame,
            channel,
            kind,
        })
    }

    /// The same event `by` semitones higher (lower when negative), for
    /// practice transpose; notes that would leave the keyboard are left
    /// where they are.
    pub fn transposed(self, by: i32) -> Event {
        let shift = |p: u8| {
            u8::try_from(p as i32 + by)
                .ok()
                .filter(|&p| p <= 127)
                .unwrap_or(p)
        };
        let kind = match self.kind {
            Kind::NoteOn {
                pitch,
                velocity,
                id,
            } => Kind::NoteOn {
                pitch: shift(pitch),
                velocity,
                id,
            },
            Kind::NoteOff {
                pitch,
                velocity,
                id,
            } => Kind::NoteOff {
                pitch: shift(pitch),
                velocity,
                id,
            },
            Kind::KeyPressure { pitch, value } => Kind::KeyPressure {
                pitch: shift(pitch),
                value,
            },
            k => k,
        };
        Event { kind, ..self }
    }
}

/// A note event from a MIDI device, on its way to the engine: which
/// input port (as the app numbers them) it came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveEvent {
    pub port: usize,
    pub event: Event,
}

/// A note event on its way from the engine to a MIDI output port.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutEvent {
    pub port: usize,
    pub event: Event,
}

/// A live event as the engine saw it, for recording and for keeping
/// what was just played: where the transport was when it arrived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Captured {
    pub port: usize,
    pub event: Event,
    /// The timeline frame the event was played at.
    pub at: i64,
    /// The transport was playing (not counting in).
    pub playing: bool,
    /// The transport was recording, so armed tracks keep it.
    pub recording: bool,
}

/// The most events one block carries. Anything past it is dropped,
/// which is counted; a real performance never comes near.
pub const MAX_EVENTS: usize = 1024;

/// An instrument on a track: given the block's events, it writes the
/// block's audio. Run on the audio thread.
pub trait Instrument: Send {
    /// Fills `left` and `right` (at most the graph's `max_block` frames)
    /// with the audio for this block, playing `events`, which come in
    /// frame order. The buffers hold the track's audio clips already;
    /// an instrument adds to them. Must not allocate, lock or block.
    fn process(&mut self, events: &[Event], left: &mut [f32], right: &mut [f32]);

    /// Frames of delay the instrument adds, for compensation.
    fn latency(&self) -> u32 {
        0
    }
}

/// A frequency, in Hz, for a pitch, with the A above middle C at
/// `a4_hz`.
pub fn pitch_hz(pitch: f64, a4_hz: f64) -> f64 {
    a4_hz * 2f64.powf((pitch - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_bytes_round_trip() {
        let on = Event::note_on(7, 3, 60, 100, 0);
        assert_eq!(on.to_midi(), Some([0x93, 60, 100]));
        assert_eq!(Event::from_midi(7, &[0x93, 60, 100]), Some(on));
        let off = Event::note_off(0, 3, 60, 0);
        assert_eq!(Event::from_midi(0, &[0x93, 60, 0]), Some(off));
        assert_eq!(Event::from_midi(0, &[0x83, 60, 0]), Some(off));
        let bend = Event {
            frame: 1,
            channel: 0,
            kind: Kind::PitchBend(-8192),
        };
        assert_eq!(bend.to_midi(), Some([0xe0, 0, 0]));
        assert_eq!(Event::from_midi(1, &[0xe0, 0, 0]), Some(bend));
        let centre = Event::from_midi(0, &[0xe1, 0, 64]).unwrap();
        assert_eq!(centre.kind, Kind::PitchBend(0));
        assert_eq!(centre.channel, 1);
        assert_eq!(Event::from_midi(0, &[0xf8]), None);
        assert_eq!(Event::from_midi(0, &[]), None);
        assert_eq!(
            Event::from_midi(0, &[0xb0, 123, 0]).unwrap().kind,
            Kind::AllNotesOff
        );
    }

    #[test]
    fn transposing_keeps_notes_on_the_keyboard() {
        let e = Event::note_on(0, 0, 125, 1, 0).transposed(5);
        assert!(matches!(e.kind, Kind::NoteOn { pitch: 125, .. }));
        let e = Event::note_on(0, 0, 60, 1, 0).transposed(-12);
        assert!(matches!(e.kind, Kind::NoteOn { pitch: 48, .. }));
    }

    #[test]
    fn pitches_to_hz() {
        assert!((pitch_hz(69.0, 440.0) - 440.0).abs() < 1e-9);
        assert!((pitch_hz(57.0, 440.0) - 220.0).abs() < 1e-9);
    }
}
