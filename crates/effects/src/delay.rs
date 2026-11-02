// SPDX-License-Identifier: GPL-3.0-or-later
//! A stereo echo with feedback, a tone control on the repeats and an
//! option to bounce them between left and right.

use crate::biquad::{Biquad, Coefs};
use crate::{Dsp, Param, Unit, flush};

pub const PARAMS: &[Param] = &[
    Param::new("time", "Time", 1.0, 2000.0, 375.0, Unit::Ms).log(),
    Param::new("feedback", "Repeats", 0.0, 95.0, 35.0, Unit::Percent),
    Param::new("mix", "Mix", 0.0, 100.0, 25.0, Unit::Percent),
    Param::new("tone", "Tone", 1000.0, 20000.0, 6000.0, Unit::Hz)
        .log()
        .off_at(20000.0),
    Param::new(
        "ping_pong",
        "Bounce left and right",
        0.0,
        1.0,
        0.0,
        Unit::Switch,
    ),
    Param::new("sync", "Sync to tempo", 0.0, 14.0, 0.0, Unit::Choice).choices(&SYNC_NAMES),
];

/// The setting that picks a note length instead of the time.
pub const SYNC: usize = 5;

/// The note lengths the time can follow, by [`SYNC`]'s value: "T" is a
/// triplet, "." dotted. Off uses the time in ms.
const SYNC_NAMES: [&str; 15] = [
    "Off", "1/32", "1/16T", "1/16", "1/16.", "1/8T", "1/8", "1/8.", "1/4T", "1/4", "1/4.", "1/2T",
    "1/2", "1/2.", "1/1",
];

/// Each note length in quarter notes, by [`SYNC`]'s value (0, Off, has
/// none).
const SYNC_QUARTERS: [f64; 15] = [
    0.0,
    0.125,
    1.0 / 6.0,
    0.25,
    0.375,
    1.0 / 3.0,
    0.5,
    0.75,
    2.0 / 3.0,
    1.0,
    1.5,
    4.0 / 3.0,
    2.0,
    3.0,
    4.0,
];

/// The time in ms that note length `sync` (a [`SYNC`] value) takes at
/// `bpm` quarter notes a minute, as the delay plays it (no more than
/// [`MAX_SECONDS`]); `None` for Off.
pub fn synced_ms(sync: f64, bpm: f64) -> Option<f64> {
    let q = *SYNC_QUARTERS.get(sync.round().max(0.0) as usize)?;
    (q > 0.0 && bpm > 0.0).then(|| (q * 60_000.0 / bpm).min(MAX_SECONDS * 1000.0))
}

/// The longest echo: room for a whole note down to 60 bpm.
pub const MAX_SECONDS: f64 = 4.0;

/// The tempo a delay assumes until it is told one.
const DEFAULT_BPM: f64 = 120.0;

pub struct Delay {
    rate: f64,
    lines: [Vec<f32>; 2],
    at: usize,
    /// The delay in frames now, and where it is going this chunk.
    frames: f64,
    to: f64,
    feedback: f32,
    wet: f32,
    tone: [Biquad; 2],
    ping_pong: bool,
    /// The time setting in ms and the note length picked, as last
    /// updated, and the tempo heard now.
    ms: f64,
    sync: f64,
    bpm: f64,
}

impl Delay {
    pub fn new(rate: f64) -> Delay {
        let len = (MAX_SECONDS * rate) as usize + 4;
        Delay {
            rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            frames: -1.0,
            to: 0.0,
            feedback: 0.0,
            wet: 0.0,
            tone: Default::default(),
            ping_pong: false,
            ms: 0.0,
            sync: 0.0,
            bpm: DEFAULT_BPM,
        }
    }

    /// Where the delay goes from the time, note length and tempo now.
    fn aim(&mut self) {
        let ms = synced_ms(self.sync, self.bpm).unwrap_or(self.ms);
        let max = self.lines[0].len() as f64 - 3.0;
        self.to = (ms / 1000.0 * self.rate).clamp(1.0, max);
        if self.frames < 0.0 {
            self.frames = self.to;
        }
    }
}

impl Dsp for Delay {
    fn update(&mut self, v: &[f64]) {
        self.ms = v[0];
        self.sync = v[SYNC];
        self.aim();
        self.feedback = (v[1] / 100.0) as f32;
        self.wet = (v[2] / 100.0) as f32;
        let c = if PARAMS[3].off_at == Some(v[3]) {
            Coefs::THROUGH
        } else {
            Coefs::low_pass(v[3], std::f64::consts::FRAC_1_SQRT_2, self.rate)
        };
        for t in &mut self.tone {
            t.c = c;
        }
        self.ping_pong = v[4] >= 0.5;
    }

    fn tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 && bpm != self.bpm {
            self.bpm = bpm;
            self.aim();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().max(1);
        let step = (self.to - self.frames) / n as f64;
        let len = self.lines[0].len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.frames += step;
            let el = crate::delay_read(&self.lines[0], self.at, self.frames);
            let er = crate::delay_read(&self.lines[1], self.at, self.frames);
            let fl = self.tone[0].run(el) * self.feedback;
            let fr = self.tone[1].run(er) * self.feedback;
            let (wl, wr) = if self.ping_pong {
                ((*l + *r) * 0.5 + fr, fl)
            } else {
                (*l + fl, *r + fr)
            };
            self.lines[0][self.at] = flush(wl);
            self.lines[1][self.at] = flush(wr);
            self.at = (self.at + 1) % len;
            *l = *l * (1.0 - self.wet) + el * self.wet;
            *r = *r * (1.0 - self.wet) + er * self.wet;
        }
        self.frames = self.to;
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    #[test]
    fn echoes_arrive_on_time_and_die_away() {
        let e = Effect::new(Kind::Delay);
        e.set(0, 100.0);
        e.set(1, 50.0);
        e.set(2, 100.0);
        e.set(3, 20000.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 48_000];
        l[0] = 1.0;
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        // All wet: the click itself is gone, its echoes every 100 ms at
        // half the level each time.
        assert!(l[0].abs() < 1e-6);
        for k in 1..5 {
            let at = k * 4800;
            let want = 0.5f32.powi(k as i32 - 1);
            assert!((l[at] - want).abs() < 1e-3, "echo {k}: {}", l[at]);
            assert!(l[at - 10].abs() < 1e-6);
        }
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let e = Effect::new(Kind::Delay);
        e.set(0, 100.0);
        e.set(1, 50.0);
        e.set(2, 100.0);
        e.set(3, 20000.0);
        e.set(4, 1.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 48_000];
        let mut r = vec![0f32; 48_000];
        l[0] = 1.0;
        r[0] = 1.0;
        p.process(&mut l, &mut r);
        assert!(l[4800] > 0.9 && r[4800].abs() < 1e-6);
        assert!(r[9600] > 0.4 && l[9600].abs() < 1e-6);
        assert!(l[14400] > 0.2 && r[14400].abs() < 1e-6);
    }

    /// Where the first echo of a click at frame 0 lands, all wet.
    fn first_echo(sync: f64, bpm: f64) -> usize {
        let e = Effect::new(Kind::Delay);
        e.set(1, 0.0);
        e.set(2, 100.0);
        e.set(3, 20000.0);
        e.set(super::SYNC, sync);
        let mut p = e.processor(48_000);
        p.set_tempo(bpm);
        let mut l = vec![0f32; 96_000];
        l[0] = 1.0;
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        l.iter().position(|s| s.abs() > 0.5).unwrap()
    }

    #[test]
    fn a_synced_time_follows_the_note_length_and_tempo() {
        // A dotted eighth at 122 bpm: 0.75 of a beat, 368.85 ms.
        assert_eq!(
            super::synced_ms(7.0, 122.0).map(|m| (m * 100.0).round()),
            Some(36885.0)
        );
        assert_eq!(
            first_echo(7.0, 122.0),
            (0.36885 * 48_000.0f64).round() as usize
        );
        // An eighth at 120 bpm is 250 ms, and twice as long at 60.
        assert_eq!(first_echo(6.0, 120.0), 12_000);
        assert_eq!(first_echo(6.0, 60.0), 24_000);
        // A triplet eighth: a third of a beat.
        assert_eq!(first_echo(5.0, 120.0), 8_000);
        // Off plays the time setting (375 ms by default).
        assert_eq!(first_echo(0.0, 60.0), 18_000);
        assert_eq!(super::synced_ms(0.0, 120.0), None);
    }

    #[test]
    fn a_whole_note_too_slow_to_fit_is_kept_to_the_longest_echo() {
        assert_eq!(
            super::synced_ms(14.0, 30.0),
            Some(super::MAX_SECONDS * 1000.0)
        );
    }

    #[test]
    fn the_time_is_hidden_while_synced() {
        let e = Effect::new(Kind::Delay);
        assert!(e.shown("time") && e.synced().is_none());
        e.set(super::SYNC, 7.0);
        assert!(!e.shown("time") && e.shown("feedback"));
        assert_eq!(e.synced(), Some("1/8."));
        assert_eq!(e.synced_ms(120.0), Some(375.0));
    }

    #[test]
    fn a_tempo_change_moves_the_echoes() {
        let e = Effect::new(Kind::Delay);
        e.set(1, 0.0);
        e.set(2, 100.0);
        e.set(3, 20000.0);
        e.set(super::SYNC, 9.0); // a quarter note
        let mut p = e.processor(48_000);
        p.set_tempo(120.0);
        let mut l = vec![0f32; 48_000];
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        // The tempo halves; once the time has moved, a click comes back
        // a second later, not half a second.
        p.set_tempo(60.0);
        let mut l = vec![0f32; 48_000];
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        let mut l = vec![0f32; 96_000];
        l[0] = 1.0;
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        assert_eq!(l.iter().position(|s| s.abs() > 0.5), Some(48_000));
    }
}
