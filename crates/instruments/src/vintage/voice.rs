// SPDX-License-Identifier: GPL-3.0-or-later
//! Voice handling every synth here shares: which voice plays which note.
//! Polyphonic with the oldest voice stolen when all are busy, unison
//! stacking (several voices on one note), mono with low, high or last
//! note priority, legato (no envelope restart while a key is still
//! down) and the return to a held note when the top one is let go, as
//! the monosynths do. Glide gives the pitch to follow.
//!
//! The allocator decides; it plays nothing. The synth applies the
//! [`Action`]s it returns to its own voices.

/// Which held note a mono synth plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Priority {
    Low,
    High,
    #[default]
    Last,
}

impl Priority {
    pub const NAMES: &[&str] = &["Lowest note", "Highest note", "Last note"];

    pub fn from_index(i: f32) -> Priority {
        match i as u32 {
            0 => Priority::Low,
            1 => Priority::High,
            _ => Priority::Last,
        }
    }
}

/// How notes are handed to voices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceSettings {
    /// How many notes at once; 1 is mono.
    pub notes: usize,
    /// Voices on each note.
    pub unison: usize,
    /// In mono, which held note sounds.
    pub priority: Priority,
    /// In mono, a note played while another is held changes the pitch
    /// without restarting the envelopes.
    pub legato: bool,
}

impl Default for VoiceSettings {
    fn default() -> VoiceSettings {
        VoiceSettings {
            notes: 8,
            unison: 1,
            priority: Priority::Last,
            legato: false,
        }
    }
}

/// What the synth should do to one of its voices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Play `note` on `voice`. `fresh` is true when the voice was
    /// silent (reset its oscillators and filters); false when it was
    /// still sounding and the envelopes restart from where they are.
    Start {
        voice: usize,
        note: u8,
        velocity: u8,
        fresh: bool,
    },
    /// Move `voice` to `note` (through glide, if any) with no restart.
    Retune { voice: usize, note: u8 },
    /// Let `voice` go into its release.
    Release { voice: usize },
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    on: bool,
    note: u8,
    /// Its key is down, or the pedal holds it.
    held: bool,
    age: u64,
}

/// The voice allocator.
pub struct Allocator {
    slots: Vec<Slot>,
    /// Keys down, in the order pressed, with velocities.
    keys: Vec<(u8, u8)>,
    pedal: bool,
    clock: u64,
    /// The note mono mode is sounding.
    mono_note: Option<u8>,
}

impl Allocator {
    /// For a synth with `voices` voices.
    pub fn new(voices: usize) -> Allocator {
        Allocator {
            slots: vec![Slot::default(); voices.max(1)],
            keys: Vec::with_capacity(32),
            pedal: false,
            clock: 0,
            mono_note: None,
        }
    }

    pub fn voices(&self) -> usize {
        self.slots.len()
    }

    /// The note voice `v` plays, if it is on.
    pub fn note_of(&self, v: usize) -> Option<u8> {
        self.slots.get(v).filter(|s| s.on).map(|s| s.note)
    }

    /// The synth says voice `v` has gone silent.
    pub fn free(&mut self, v: usize) {
        if let Some(s) = self.slots.get_mut(v) {
            s.on = false;
            s.held = false;
        }
    }

    /// The pedal held down keeps released notes sounding.
    pub fn pedal(&mut self, down: bool, out: &mut Vec<Action>) {
        self.pedal = down;
        if !down {
            for (i, s) in self.slots.iter_mut().enumerate() {
                if s.on && s.held && !self.keys.iter().any(|k| k.0 == s.note) {
                    s.held = false;
                    out.push(Action::Release { voice: i });
                }
            }
            if let Some(n) = self.mono_note
                && !self.keys.iter().any(|k| k.0 == n)
            {
                self.mono_note = None;
            }
        }
    }

    /// Lets every voice go.
    pub fn all_off(&mut self, out: &mut Vec<Action>) {
        self.keys.clear();
        self.pedal = false;
        self.mono_note = None;
        for (i, s) in self.slots.iter_mut().enumerate() {
            if s.on {
                s.held = false;
                out.push(Action::Release { voice: i });
            }
        }
    }

    /// The mono note to sound, by priority.
    fn chosen(&self, p: Priority) -> Option<(u8, u8)> {
        match p {
            Priority::Low => self.keys.iter().min_by_key(|k| k.0).copied(),
            Priority::High => self.keys.iter().max_by_key(|k| k.0).copied(),
            Priority::Last => self.keys.last().copied(),
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8, s: &VoiceSettings, out: &mut Vec<Action>) {
        self.clock += 1;
        self.keys.retain(|k| k.0 != note);
        if self.keys.len() == self.keys.capacity() {
            self.keys.remove(0);
        }
        self.keys.push((note, velocity));
        let unison = s.unison.clamp(1, self.slots.len());
        if s.notes <= 1 {
            let Some((n, vel)) = self.chosen(s.priority) else {
                return;
            };
            let sounding = self.mono_note.is_some() && self.slots[0].on;
            let was_held = self.keys.len() > 1;
            self.mono_note = Some(n);
            for v in 0..unison {
                let slot = &mut self.slots[v];
                let fresh = !slot.on;
                slot.on = true;
                slot.note = n;
                slot.held = true;
                slot.age = self.clock;
                if sounding && was_held && s.legato {
                    out.push(Action::Retune { voice: v, note: n });
                } else {
                    out.push(Action::Start {
                        voice: v,
                        note: n,
                        velocity: vel,
                        fresh,
                    });
                }
            }
            // Voices beyond the unison count fall silent.
            for v in unison..self.slots.len() {
                if self.slots[v].on && self.slots[v].held {
                    self.slots[v].held = false;
                    out.push(Action::Release { voice: v });
                }
            }
            return;
        }
        // Polyphonic: the same note again takes its own voices back.
        for _ in 0..unison {
            let pick = self.pick(note);
            let slot = &mut self.slots[pick];
            let fresh = !slot.on;
            slot.on = true;
            slot.note = note;
            slot.held = true;
            slot.age = self.clock;
            out.push(Action::Start {
                voice: pick,
                note,
                velocity,
                fresh,
            });
        }
        // Keep to the note count: the oldest notes beyond it go.
        let mut notes: Vec<(u8, u64)> = Vec::new();
        for sl in self.slots.iter().filter(|s| s.on && s.held) {
            match notes.iter_mut().find(|n| n.0 == sl.note) {
                Some(n) => n.1 = n.1.max(sl.age),
                None => notes.push((sl.note, sl.age)),
            }
        }
        if notes.len() > s.notes {
            notes.sort_by_key(|n| n.1);
            for (old, _) in &notes[..notes.len() - s.notes] {
                for (i, sl) in self.slots.iter_mut().enumerate() {
                    if sl.on && sl.held && sl.note == *old {
                        sl.held = false;
                        out.push(Action::Release { voice: i });
                    }
                }
            }
        }
    }

    /// A voice for `note`: its own if it is already sounding and not
    /// held, a silent one, a released one (oldest), else the oldest.
    fn pick(&self, note: u8) -> usize {
        let fresh = |s: &Slot| !s.on;
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.on && !s.held && s.note == note)
        {
            return i;
        }
        if let Some(i) = self.slots.iter().position(fresh) {
            return i;
        }
        let released = (0..self.slots.len())
            .filter(|&i| !self.slots[i].held)
            .min_by_key(|&i| self.slots[i].age);
        released.unwrap_or_else(|| {
            (0..self.slots.len())
                .min_by_key(|&i| self.slots[i].age)
                .unwrap_or(0)
        })
    }

    pub fn note_off(&mut self, note: u8, s: &VoiceSettings, out: &mut Vec<Action>) {
        self.keys.retain(|k| k.0 != note);
        let unison = s.unison.clamp(1, self.slots.len());
        if s.notes <= 1 {
            if self.mono_note != Some(note) {
                return;
            }
            match self.chosen(s.priority) {
                Some((n, vel)) if n != note => {
                    // Back to a note still held.
                    self.mono_note = Some(n);
                    for v in 0..unison {
                        self.slots[v].note = n;
                        if s.legato {
                            out.push(Action::Retune { voice: v, note: n });
                        } else {
                            out.push(Action::Start {
                                voice: v,
                                note: n,
                                velocity: vel,
                                fresh: false,
                            });
                        }
                    }
                }
                _ if self.pedal => {}
                _ => {
                    self.mono_note = None;
                    for v in 0..unison {
                        if self.slots[v].on && self.slots[v].held {
                            self.slots[v].held = false;
                            out.push(Action::Release { voice: v });
                        }
                    }
                }
            }
            return;
        }
        if self.pedal {
            return;
        }
        for (i, sl) in self.slots.iter_mut().enumerate() {
            if sl.on && sl.held && sl.note == note {
                sl.held = false;
                out.push(Action::Release { voice: i });
            }
        }
    }
}

/// How the pitch moves between notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GlideMode {
    /// Every glide takes the same time.
    #[default]
    Time,
    /// Every glide moves at the same speed, so a wide one takes longer.
    Rate,
}

impl GlideMode {
    pub const NAMES: &[&str] = &["Same time", "Same speed"];

    pub fn from_index(i: f32) -> GlideMode {
        if i as u32 == 1 {
            GlideMode::Rate
        } else {
            GlideMode::Time
        }
    }
}

/// Glide settings: the time of a glide, or the time per octave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    pub mode: GlideMode,
    pub seconds: f32,
}

impl Glide {
    /// `pitch` moved towards `target` over `samples` samples.
    #[inline]
    pub fn step(&self, pitch: f32, target: f32, samples: f32, rate: f32) -> f32 {
        if self.seconds <= 0.0 {
            return target;
        }
        match self.mode {
            GlideMode::Time => {
                // Within 5% of the way in the time set.
                let k = 1.0 - (-3.0 * samples / (self.seconds * rate)).exp();
                pitch + (target - pitch) * k
            }
            GlideMode::Rate => {
                let most = 12.0 * samples / (self.seconds * rate);
                pitch + (target - pitch).clamp(-most, most)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly() -> VoiceSettings {
        VoiceSettings {
            notes: 4,
            unison: 1,
            priority: Priority::Last,
            legato: false,
        }
    }

    fn mono(priority: Priority, legato: bool) -> VoiceSettings {
        VoiceSettings {
            notes: 1,
            unison: 1,
            priority,
            legato,
        }
    }

    #[test]
    fn polyphony_fills_free_voices_then_steals_the_oldest() {
        let mut a = Allocator::new(3);
        let mut out = Vec::new();
        let s = poly();
        a.note_on(60, 100, &s, &mut out);
        a.note_on(64, 100, &s, &mut out);
        a.note_on(67, 100, &s, &mut out);
        assert_eq!(out.len(), 3);
        assert!(matches!(
            out[2],
            Action::Start {
                voice: 2,
                note: 67,
                fresh: true,
                ..
            }
        ));
        out.clear();
        a.note_off(64, &s, &mut out);
        assert_eq!(out, vec![Action::Release { voice: 1 }]);
        out.clear();
        // The released voice is taken before a held one is stolen.
        a.note_on(72, 100, &s, &mut out);
        assert!(matches!(
            out[0],
            Action::Start {
                voice: 1,
                note: 72,
                fresh: false,
                ..
            }
        ));
        out.clear();
        a.note_on(74, 100, &s, &mut out);
        assert!(matches!(
            out[0],
            Action::Start {
                voice: 0,
                note: 74,
                ..
            }
        ));
    }

    #[test]
    fn unison_stacks_voices_and_the_note_count_limits_them() {
        let mut a = Allocator::new(8);
        let mut out = Vec::new();
        let s = VoiceSettings {
            notes: 2,
            unison: 2,
            ..poly()
        };
        a.note_on(60, 100, &s, &mut out);
        assert_eq!(out.len(), 2);
        out.clear();
        a.note_on(64, 100, &s, &mut out);
        a.note_on(67, 100, &s, &mut out);
        // The third note pushes the first out.
        assert!(
            out.iter()
                .filter(|x| matches!(x, Action::Release { .. }))
                .count()
                == 2
        );
        assert_eq!(a.note_of(0), Some(60));
        a.free(0);
        assert_eq!(a.note_of(0), None);
    }

    #[test]
    fn mono_follows_its_priority_and_goes_back_to_a_held_note() {
        for (p, sounds) in [
            (Priority::Last, 60),
            (Priority::Low, 60),
            (Priority::High, 64),
        ] {
            let mut a = Allocator::new(2);
            let mut out = Vec::new();
            let s = mono(p, false);
            a.note_on(64, 100, &s, &mut out);
            a.note_on(60, 100, &s, &mut out);
            assert_eq!(a.note_of(0), Some(sounds), "{p:?}");
        }
        let mut a = Allocator::new(2);
        let mut out = Vec::new();
        let s = mono(Priority::Last, false);
        a.note_on(60, 100, &s, &mut out);
        a.note_on(64, 100, &s, &mut out);
        assert!(matches!(
            out[1],
            Action::Start {
                voice: 0,
                note: 64,
                fresh: false,
                ..
            }
        ));
        out.clear();
        a.note_off(64, &s, &mut out);
        assert!(matches!(
            out[0],
            Action::Start {
                voice: 0,
                note: 60,
                fresh: false,
                ..
            }
        ));
        out.clear();
        a.note_off(60, &s, &mut out);
        assert_eq!(out, vec![Action::Release { voice: 0 }]);
    }

    #[test]
    fn legato_retunes_instead_of_restarting() {
        let mut a = Allocator::new(1);
        let mut out = Vec::new();
        let s = mono(Priority::Last, true);
        a.note_on(60, 100, &s, &mut out);
        a.note_on(64, 100, &s, &mut out);
        assert_eq!(out[1], Action::Retune { voice: 0, note: 64 });
        out.clear();
        a.note_off(64, &s, &mut out);
        assert_eq!(out[0], Action::Retune { voice: 0, note: 60 });
        out.clear();
        a.note_off(60, &s, &mut out);
        a.note_on(67, 100, &s, &mut out);
        // Nothing held: a new note restarts.
        assert!(matches!(out[1], Action::Start { note: 67, .. }));
    }

    #[test]
    fn the_pedal_holds_notes_until_it_lifts() {
        let mut a = Allocator::new(2);
        let mut out = Vec::new();
        let s = poly();
        a.note_on(60, 100, &s, &mut out);
        a.pedal(true, &mut out);
        a.note_off(60, &s, &mut out);
        assert!(out.iter().all(|x| !matches!(x, Action::Release { .. })));
        a.pedal(false, &mut out);
        assert!(out.contains(&Action::Release { voice: 0 }));
    }

    #[test]
    fn glide_takes_its_time_or_keeps_its_speed() {
        let t = Glide {
            mode: GlideMode::Time,
            seconds: 0.1,
        };
        let mut p = 60.0;
        for _ in 0..10 {
            p = t.step(p, 72.0, 480.0, 48_000.0);
        }
        assert!((p - 72.0).abs() < 0.6, "{p}");
        let r = Glide {
            mode: GlideMode::Rate,
            seconds: 0.1,
        };
        // An octave a tenth of a second: half way after a twentieth.
        let p = r.step(60.0, 72.0, 2400.0, 48_000.0);
        assert!((p - 66.0).abs() < 1e-3, "{p}");
        assert_eq!(r.step(60.0, 61.0, 48_000.0, 48_000.0), 61.0);
        let none = Glide {
            mode: GlideMode::Time,
            seconds: 0.0,
        };
        assert_eq!(none.step(60.0, 72.0, 1.0, 48_000.0), 72.0);
    }
}
