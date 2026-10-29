// SPDX-License-Identifier: GPL-3.0-or-later
//! The metronome (roadmap 1.1): a click on every beat, a different one on
//! the first beat of each bar, and optional quieter subdivisions. The
//! clicks are scheduled from the tempo map, so they land on the exact
//! frame of each beat, across tempo and time-signature changes.
//!
//! Click sounds are short decaying sine bursts, made when the graph is
//! compiled. One click sounds at a time; a new one cuts the last off.

use crate::graph::db_to_gain;
use crate::time::{Frames, TempoMap, Ticks};

/// Metronome settings. They can change while playing
/// (`Command::SetMetronome`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metronome {
    /// Click while playing. Count-in clicks sound regardless.
    pub enabled: bool,
    /// Clicks per beat: 1 (beats only) to 4.
    pub subdivision: u32,
    /// A higher click on the first beat of each bar.
    pub accent: bool,
    pub level_db: f32,
    /// Send clicks to the separate click output instead of the main mix
    /// (for example, to the player's headphones only).
    pub separate_output: bool,
}

impl Default for Metronome {
    fn default() -> Metronome {
        Metronome {
            enabled: false,
            subdivision: 1,
            accent: true,
            level_db: -6.0,
            separate_output: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Accent,
    Beat,
    Sub,
}

/// The three click sounds.
pub(crate) struct Clicks {
    accent: Vec<f32>,
    beat: Vec<f32>,
    sub: Vec<f32>,
}

impl Clicks {
    pub fn new(rate: u32) -> Clicks {
        let burst = |hz: f32, gain: f32| -> Vec<f32> {
            // 40 ms, with a 1 ms rise so the click itself does not pop.
            let n = (rate as f32 * 0.04) as usize;
            let rise = (rate as f32 * 0.001).max(1.0);
            (0..n)
                .map(|i| {
                    let t = i as f32 / rate as f32;
                    let env = (i as f32 / rise).min(1.0) * (-t * 120.0).exp();
                    (std::f32::consts::TAU * hz * t).sin() * env * gain
                })
                .collect()
        };
        Clicks {
            accent: burst(1760.0, 1.0),
            beat: burst(1320.0, 0.8),
            sub: burst(990.0, 0.45),
        }
    }

    fn sound(&self, k: Kind) -> &[f32] {
        match k {
            Kind::Accent => &self.accent,
            Kind::Beat => &self.beat,
            Kind::Sub => &self.sub,
        }
    }
}

/// The click that is sounding, carried from block to block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Voice {
    kind: Kind,
    at: usize,
}

/// The first click at or after frame `from`: its frame and kind.
fn next_click(map: &TempoMap, subdivision: u32, accent: bool, from: i64) -> (i64, Kind) {
    let t = map.ticks_at(Frames(from));
    let sub = (map.signature_at(t).beat_ticks() / subdivision.clamp(1, 4) as i64).max(1);
    let bb = map.bar_beat(t);
    // Back to the grid line at or before `t`, then forwards until the
    // click is not before `from`.
    let mut tick = Ticks(t.0 - bb.tick % sub);
    loop {
        let f = map.frames_at(tick).0;
        if f >= from {
            let b = map.bar_beat(tick);
            let kind = if b.tick != 0 {
                Kind::Sub
            } else if b.beat == 1 && accent {
                Kind::Accent
            } else {
                Kind::Beat
            };
            return (f, kind);
        }
        let sub = (map.signature_at(tick).beat_ticks() / subdivision.clamp(1, 4) as i64).max(1);
        tick = Ticks(tick.0 + sub);
    }
}

/// Adds clicks for frames `[from, from + out.len())` into `out`.
pub(crate) fn render(
    map: &TempoMap,
    clicks: &Clicks,
    m: &Metronome,
    voice: &mut Option<Voice>,
    from: i64,
    out: &mut [f32],
) {
    render_at(map, clicks, m, voice, from as f64, 1.0, out);
}

/// Adds clicks into `out`, where output frame `i` plays timeline
/// position `from + i × step`: `step` is the practice speed, so clicks
/// follow a slowed or quickened song while each click sounds as usual.
pub(crate) fn render_at(
    map: &TempoMap,
    clicks: &Clicks,
    m: &Metronome,
    voice: &mut Option<Voice>,
    from: f64,
    step: f64,
    out: &mut [f32],
) {
    let gain = db_to_gain(m.level_db);
    let mut next = next_click(map, m.subdivision, m.accent, from.ceil() as i64);
    for (i, s) in out.iter_mut().enumerate() {
        let end = from + (i + 1) as f64 * step;
        if (next.0 as f64) < end {
            *voice = Some(Voice {
                kind: next.1,
                at: 0,
            });
            next = next_click(map, m.subdivision, m.accent, next.0 + 1);
        }
        if let Some(v) = voice {
            let snd = clicks.sound(v.kind);
            match snd.get(v.at) {
                Some(x) => {
                    *s += x * gain;
                    v.at += 1;
                }
                None => *voice = None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{PPQ, Tempo, TimeSignature};

    #[test]
    fn clicks_land_on_beats_and_accent_bars() {
        let mut m = TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON);
        m.add_meter(m.bar_start(2), TimeSignature::new(3, 4))
            .unwrap();
        let mut f = 0;
        let mut got = Vec::new();
        for _ in 0..8 {
            let (at, k) = next_click(&m, 1, true, f);
            got.push((at, k));
            f = at + 1;
        }
        let beat = 24_000;
        assert_eq!(got[0], (0, Kind::Accent));
        assert_eq!(got[1], (beat, Kind::Beat));
        // Bar 2 (3/4) starts after four beats; bar 3 three beats later.
        assert_eq!(got[4], (4 * beat, Kind::Accent));
        assert_eq!(got[7], (7 * beat, Kind::Accent));
    }

    #[test]
    fn subdivisions_and_mid_beat_starts() {
        let m = TempoMap::new(48_000, Tempo::bpm(120), TimeSignature::COMMON);
        // Eighth notes: every 12,000 frames.
        assert_eq!(next_click(&m, 2, true, 1), (12_000, Kind::Sub));
        assert_eq!(next_click(&m, 2, true, 12_001), (24_000, Kind::Beat));
        // Starting exactly on a beat clicks there.
        assert_eq!(next_click(&m, 1, true, 24_000), (24_000, Kind::Beat));
        // Before the start of the song (count-in), beats continue backwards.
        let (at, _) = next_click(&m, 1, true, -50_000);
        assert_eq!(at, -48_000);
        assert_eq!(m.ticks_at(Frames(-48_000)), Ticks(-2 * PPQ));
    }

    #[test]
    fn clicks_speed_up_through_a_glide() {
        let mut m = TempoMap::new(48_000, Tempo::bpm(60), TimeSignature::COMMON);
        m.add_glide(m.bar_start(2), Tempo::bpm(120)).unwrap();
        let mut f = 0;
        let mut at = Vec::new();
        for _ in 0..10 {
            let (c, _) = next_click(&m, 1, true, f);
            at.push(c);
            f = c + 1;
        }
        // Each click lands on its beat's frame, gaps shrinking from a
        // second towards half a second, then steady.
        for (b, &c) in at.iter().enumerate() {
            assert_eq!(c, m.frames_at(Ticks(b as i64 * PPQ)).0);
        }
        let gaps: Vec<i64> = at.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps[0] < 48_000 && gaps[0] > gaps[3]);
        assert_eq!(gaps[4..], [24_000; 5]);
        assert_eq!(next_click(&m, 1, true, at[3] + 1).1, Kind::Accent);
    }
}
