// SPDX-License-Identifier: GPL-3.0-or-later
//! The beat grid of sparse or free audio, such as a voice alone: the
//! tempos its note starts fit, how well, and where the beats and bars
//! fall. Made for an a cappella, where the attack-based tempo in
//! [`crate::analysis`] finds too few hits to say anything.
//!
//! It works on onset times (note starts, attacks), each with a weight.
//! For every tempo from 60 to 200 bpm, in steps of 0.05, the onsets are
//! put on a circle one beat round and their pull towards one point is
//! measured (vector strength: 1 when every onset is on a beat, about
//! 1/√n for onsets at random). The peaks become the candidates. The
//! point they pull towards is the beat's phase. Which beat of the bar is
//! the first is guessed from where long notes and phrase starts fall.
//!
//! Half or double the tempo fits onsets on every beat equally well, so
//! a candidate says when it is half or double of a better one; the
//! ranking leans towards the tempos songs usually have (around 115
//! bpm; a tempo an octave away counts about a seventh as much).

use std::f64::consts::TAU;

/// One onset: when, in seconds, and how much it counts (a long note or
/// a loud hit more than a short or soft one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    pub at: f64,
    pub weight: f64,
}

/// A beat grid that fits the onsets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub bpm: f64,
    /// How likely this is the beat, 0 to 1: one less [`chance`](Self::chance),
    /// times its share of the scores of the candidates not down to
    /// chance (each leaning towards the usual tempos of songs, and
    /// counted as far as it isn't chance). They add up to 1 at most.
    pub confidence: f64,
    /// How likely onsets at random would fit some tempo in the range as
    /// well as this one, 0 to 1 (by Rayleigh's test, over as many
    /// tempos as could be told apart in the time the onsets span).
    pub chance: f64,
    /// How tightly the onsets gather on its beats, 0 to 1 (vector
    /// strength, weighted).
    pub agreement: f64,
    /// The share of onsets within an eighth of a beat of a beat.
    pub on_beat: f64,
    /// The first beat at or after the start of the audio, in seconds.
    pub first_beat: f64,
    /// The first beat at or after the start that begins a bar, by the
    /// guess of which beat is the first.
    pub first_downbeat: f64,
    /// How far ahead the chosen first beat of the bar is of the next
    /// best, 0 (a coin toss) to 1 (only it).
    pub downbeat_confidence: f64,
    /// How far the beat moves across the audio, in seconds: the widest
    /// gap between the grid and where the onsets in each third of it put
    /// the beat. Large for a performance that speeds up or drifts.
    pub drift: f64,
    /// When it is half (0.5) or double (2) of a candidate listed before
    /// it, or 1.5 or 2/3 of one: that ratio, and which candidate (from 0).
    pub relation: Option<(f64, usize)>,
}

/// The slowest and fastest tempo tried.
pub const RANGE: (f64, f64) = (60.0, 200.0);
const STEP: f64 = 0.05;
/// Candidates given at most.
const MOST: usize = 5;
/// Fewest onsets to say anything from.
pub const FEWEST: usize = 6;

/// The beat grids that fit `onsets` (seconds from the start of the
/// audio, any order), best first, with `beats_per_bar` beats in a bar.
/// Empty when there are too few onsets.
pub fn beat_grid(onsets: &[Onset], beats_per_bar: u32) -> Vec<Grid> {
    let mut on: Vec<Onset> = (onsets.iter())
        .filter(|o| o.at.is_finite() && o.weight > 0.0 && o.at >= 0.0)
        .copied()
        .collect();
    on.sort_by(|a, b| a.at.total_cmp(&b.at));
    if on.len() < FEWEST {
        return Vec::new();
    }
    let total: f64 = on.iter().map(|o| o.weight).sum();
    let squares: f64 = on.iter().map(|o| o.weight * o.weight).sum();
    // As many onsets as these weights are worth.
    let n_eff = total * total / squares;
    let span = on.last().unwrap().at - on[0].at;

    let steps = ((RANGE.1 - RANGE.0) / STEP).round() as usize;
    let bpm_at = |i: usize| RANGE.0 + i as f64 * STEP;
    let strength: Vec<f64> = (0..=steps)
        .map(|i| pull(&on, total, 60.0 / bpm_at(i)).0)
        .collect();
    // Peaks, refined between the steps.
    let mut peaks: Vec<(f64, f64)> = Vec::new();
    for i in 1..steps {
        let (a, b, c) = (strength[i - 1], strength[i], strength[i + 1]);
        if b > a && b >= c {
            let d = a - 2.0 * b + c;
            let off = if d < 0.0 {
                (0.5 * (a - c) / d).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let bpm = bpm_at(i) + off * STEP;
            peaks.push((bpm, pull(&on, total, 60.0 / bpm).0));
        }
    }
    // Rayleigh's z for each: for onsets at random, more than z with
    // odds e^−z at one tempo, and `tries` tempos can be told apart (a
    // tempo's peak is about 1/span Hz wide).
    let z = |r: f64| n_eff * r * r;
    let tries = (span.max(1.0) * (RANGE.1 - RANGE.0) / 60.0).max(2.0);
    let chance = |r: f64| (tries * (-z(r)).exp()).min(1.0);
    // Onsets on every half beat fit double the tempo as well as the
    // tempo, and a voice's timing wobbles less against a slower grid,
    // so the lean towards the usual tempos of songs is firmer than the
    // attack-based tempo's.
    let lean = |bpm: f64| (-0.5 * ((bpm / 115.0).log2() / 0.5).powi(2)).exp();
    let score = |p: &(f64, f64)| z(p.1) * lean(p.0);
    peaks.sort_by(|a, b| score(b).total_cmp(&score(a)));
    // A peak too near a better one is the same beat.
    let mut chosen: Vec<(f64, f64)> = Vec::new();
    for p in peaks {
        if chosen.iter().all(|c| (c.0 - p.0).abs() > 1.0) {
            chosen.push(p);
        }
        if chosen.len() == MOST {
            break;
        }
    }
    let real = |p: &(f64, f64)| (1.0 - chance(p.1)) * score(p);
    let sum: f64 = chosen.iter().map(real).sum();
    let start = 0.0;
    let mut out: Vec<Grid> = Vec::new();
    for (k, &(bpm, agreement)) in chosen.iter().enumerate() {
        let period = 60.0 / bpm;
        let (_, phase) = pull(&on, total, period);
        // Beats fall at `at0 + j × period`.
        let at0 = phase / TAU * period;
        let first_beat = at0 + ((start - at0) / period).ceil() * period;
        let on_beat = (on.iter())
            .filter(|o| {
                let d = ((o.at - at0) / period).rem_euclid(1.0);
                d.min(1.0 - d) <= 0.125
            })
            .map(|o| o.weight)
            .sum::<f64>()
            / total;
        let (down, down_conf) = downbeat(&on, at0, period, beats_per_bar.max(1));
        let bar = period * beats_per_bar.max(1) as f64;
        let d0 = at0 + down as f64 * period;
        let first_downbeat = d0 + ((start - d0) / bar).ceil() * bar;
        let relation = (out.iter().enumerate()).find_map(|(i, g)| {
            [0.5, 2.0, 1.5, 2.0 / 3.0]
                .into_iter()
                .find(|r| (bpm / g.bpm - r).abs() < 0.01)
                .map(|r| (r, i))
        });
        out.push(Grid {
            bpm,
            confidence: if sum > 0.0 {
                (1.0 - chance(agreement)) * real(&chosen[k]) / sum
            } else {
                0.0
            },
            chance: chance(agreement),
            agreement,
            on_beat,
            first_beat,
            first_downbeat,
            downbeat_confidence: down_conf,
            drift: drift(&on, at0, period),
            relation,
        });
    }
    out
}

/// The onsets' weighted pull towards one point on a circle `period`
/// seconds round: how strong (0 to 1) and where (radians).
fn pull(on: &[Onset], total: f64, period: f64) -> (f64, f64) {
    let (mut c, mut s) = (0.0, 0.0);
    for o in on {
        let a = TAU * o.at / period;
        c += o.weight * a.cos();
        s += o.weight * a.sin();
    }
    ((c * c + s * s).sqrt() / total, s.atan2(c).rem_euclid(TAU))
}

/// Which beat of the bar (0 to `beats` − 1 after the beat at `at0`)
/// begins a bar, and how sure: the one where most weight lands, an
/// onset's weight raised by the gap before it (a phrase starting after
/// a rest leans hardest on a downbeat).
fn downbeat(on: &[Onset], at0: f64, period: f64, beats: u32) -> (u32, f64) {
    if beats == 1 {
        return (0, 1.0);
    }
    let mut score = vec![0.0f64; beats as usize];
    let mut prev: Option<f64> = None;
    for o in on {
        let pos = (o.at - at0) / period;
        let near = pos.round();
        if (pos - near).abs() > 0.25 {
            prev = Some(o.at);
            continue;
        }
        let gap = prev.map_or(beats as f64, |p| (o.at - p) / period);
        score[(near as i64).rem_euclid(beats as i64) as usize] += o.weight * (1.0 + gap.min(4.0));
        prev = Some(o.at);
    }
    let mut order: Vec<usize> = (0..beats as usize).collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]));
    let (best, next) = (score[order[0]], score[order[1]]);
    let sure = if best > 0.0 {
        (best - next) / best
    } else {
        0.0
    };
    (order[0] as u32, sure)
}

/// The widest gap between the grid at `at0` and the beat the onsets in
/// each third of the stretch put themselves on, in seconds.
fn drift(on: &[Onset], at0: f64, period: f64) -> f64 {
    let (a, b) = (on[0].at, on[on.len() - 1].at);
    let third = (b - a) / 3.0;
    let mut most: f64 = 0.0;
    for k in 0..3 {
        let part: Vec<Onset> = (on.iter())
            .filter(|o| o.at >= a + k as f64 * third && o.at <= a + (k + 1) as f64 * third)
            .copied()
            .collect();
        if part.len() < 3 {
            continue;
        }
        let total: f64 = part.iter().map(|o| o.weight).sum();
        let (_, phase) = pull(&part, total, period);
        let local = phase / TAU * period;
        let d = (local - at0).rem_euclid(period);
        most = most.max(d.min(period - d));
    }
    most
}

/// The onsets of `left` and `right`, the same length, at `rate`: where
/// each note read by [`crate::melody`] starts, and each sudden rise in
/// level not at a note's start. A note after a break (a new word or
/// phrase) counts fully; a note the voice slides or steps into without
/// a break, and a rise in level, count [`LEGATO`] as much, since a voice
/// changes note and breathes off the beat more often than it starts a
/// word off it. The notes give a voice's onsets; the rises give those of
/// hits that have no pitch.
pub fn onsets(left: &[f32], right: &[f32], rate: u32) -> Vec<Onset> {
    onsets_weighted(left, right, rate, LEGATO)
}

/// How much a note started without a break, or a rise in level, counts.
pub const LEGATO: f64 = 0.3;
/// The silence before a note that makes it a new start, in seconds.
const BREAK: f64 = 0.03;

#[doc(hidden)]
pub fn onsets_weighted(left: &[f32], right: &[f32], rate: u32, rise: f64) -> Vec<Onset> {
    let notes = crate::melody::melody(left, right, rate).notes;
    let mut out: Vec<Onset> = Vec::new();
    let mut end = f64::NEG_INFINITY;
    for n in &notes {
        let weight = if n.start - end >= BREAK { 1.0 } else { LEGATO };
        out.push(Onset {
            at: n.start,
            weight,
        });
        end = n.start + n.seconds;
    }
    if rise > 0.0 {
        let curve = crate::analysis::attack_curve(left, right, rate);
        let step = 1.0 / crate::analysis::STEPS_PER_SECOND as f64;
        // Rises of 3 dB or more, the top of each within 50 ms either
        // side, away from the notes' starts.
        const NEAR: usize = 10;
        let loudest = curve.iter().fold(0.0f32, |m, &x| m.max(x)).max(1e-6);
        for i in 0..curve.len() {
            let v = curve[i];
            let around = &curve[i.saturating_sub(NEAR)..(i + NEAR + 1).min(curve.len())];
            if v < 3.0
                || around.iter().any(|&x| x > v)
                || curve[i.saturating_sub(NEAR)..i].contains(&v)
            {
                continue;
            }
            let at = i as f64 * step;
            let k = out.partition_point(|o| o.at < at - 0.05);
            if out.get(k).is_some_and(|o| o.at <= at + 0.05) {
                continue;
            }
            out.insert(
                k,
                Onset {
                    at,
                    weight: rise * (v / loudest) as f64,
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple random number, the same every run.
    fn noise(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed % 10_000) as f64 / 10_000.0
    }

    /// Sparse sung phrases at `bpm`: each phrase starts on a downbeat
    /// after a rest, then a few notes on beats and half beats, each a
    /// little early or late.
    fn phrases(bpm: f64, first_downbeat: f64, bars: usize, jitter: f64) -> Vec<Onset> {
        let beat = 60.0 / bpm;
        let mut seed = 0x2545F4914F6CDD1D;
        let mut out = Vec::new();
        for bar in (0..bars).step_by(2) {
            let at = first_downbeat + bar as f64 * 4.0 * beat;
            // Bar one of the phrase: a long note on the downbeat, then
            // notes on 2, 2-and and 4; bar two: 1 and 3; then a rest.
            for (b, w) in [
                (0.0, 3.0),
                (1.0, 1.0),
                (1.5, 0.5),
                (3.0, 1.0),
                (4.0, 1.5),
                (6.0, 2.0),
            ] {
                let j = (noise(&mut seed) - 0.5) * 2.0 * jitter;
                out.push(Onset {
                    at: at + b * beat + j,
                    weight: w,
                });
            }
        }
        out
    }

    #[test]
    fn sparse_sung_phrases_give_their_tempo_and_downbeat() {
        for (bpm, down) in [(116.4, 1.3), (92.0, 0.4), (128.0, 2.0)] {
            let on = phrases(bpm, down, 40, 0.02);
            let g = beat_grid(&on, 4);
            let best = g[0];
            assert!((best.bpm - bpm).abs() < 0.1, "{bpm}: {g:?}");
            assert!(best.confidence > 0.3, "{bpm}: {best:?}");
            let beat = 60.0 / bpm;
            // The first beat is the downbeat less whole beats.
            let k = (down - best.first_beat) / beat;
            assert!((k - k.round()).abs() < 0.05, "{bpm}: {best:?}");
            assert!(best.first_beat < beat + 1e-9, "{bpm}: {best:?}");
            // The downbeat is the phrase start, less whole bars.
            let bars = (down - best.first_downbeat) / (4.0 * beat);
            assert!((bars - bars.round()).abs() < 0.02, "{bpm}: {best:?}");
            assert!(best.downbeat_confidence > 0.2, "{bpm}: {best:?}");
            assert!(best.drift < 0.05, "{bpm}: {best:?}");
        }
    }

    #[test]
    fn half_and_double_are_named_as_such() {
        let g = beat_grid(&phrases(100.0, 0.5, 30, 0.01), 4);
        assert!((g[0].bpm - 100.0).abs() < 0.1, "{g:?}");
        let half = g
            .iter()
            .find(|x| (x.bpm - 200.0).abs() < 0.2 || (x.bpm - 66.67).abs() < 0.2);
        if let Some(h) = half {
            assert!(h.relation.is_some(), "{h:?}");
        }
    }

    #[test]
    fn onsets_at_random_have_no_confident_tempo() {
        let mut seed = 99;
        let on: Vec<Onset> = (0..80)
            .map(|_| Onset {
                at: noise(&mut seed) * 60.0,
                weight: 1.0,
            })
            .collect();
        let g = beat_grid(&on, 4);
        assert!(g.iter().all(|x| x.agreement < 0.35), "{g:?}");
        assert!(
            g.iter().all(|x| x.confidence == 0.0) || g[0].confidence < 0.6,
            "{g:?}"
        );
    }

    #[test]
    fn a_singer_who_drifts_is_said_to() {
        // The same phrases, but pulling ahead of the beat and falling
        // back behind it, by up to 0.2 seconds, over the take.
        let on: Vec<Onset> = (phrases(116.4, 1.0, 44, 0.0).into_iter())
            .map(|o| Onset {
                at: o.at + 0.2 * (TAU * o.at / 90.0).sin(),
                ..o
            })
            .collect();
        let g = beat_grid(&on, 4);
        assert!((g[0].bpm - 116.4).abs() < 1.0, "{g:?}");
        assert!(g[0].drift > 0.08, "{g:?}");
    }

    #[test]
    fn too_few_onsets_say_nothing() {
        let on = [0.0, 0.5, 1.0].map(|at| Onset { at, weight: 1.0 });
        assert!(beat_grid(&on, 4).is_empty());
    }

    /// A voice singing at `bpm`: two-bar phrases starting on the
    /// downbeat at `first`, with a rest after; within a phrase it moves
    /// from note to note without a break.
    fn sung(bpm: f64, first: f64, bars: usize, rate: u32) -> Vec<f32> {
        let beat = 60.0 / bpm;
        let len = first + (bars as f64 + 1.0) * 4.0 * beat;
        let mut x = vec![0.0f32; (len * rate as f64) as usize];
        let tune = [
            (0.0, 64.0),
            (1.0, 67.0),
            (2.0, 69.0),
            (2.5, 71.0),
            (4.0, 67.0),
            (6.0, 64.0),
        ];
        let mut phase = 0.0f64;
        for bar in (0..bars).step_by(2) {
            let at = first + bar as f64 * 4.0 * beat;
            let (from, to) = (at, at + 6.5 * beat);
            for i in (from * rate as f64) as usize..((to * rate as f64) as usize).min(x.len()) {
                let t = i as f64 / rate as f64;
                let b = (t - at) / beat;
                let midi = tune.iter().rev().find(|n| b >= n.0).map_or(64.0, |n| n.1);
                let hz =
                    440.0 * 2f64.powf((midi - 69.0) / 12.0) * (1.0 + 0.004 * (TAU * 5.5 * t).sin());
                phase += TAU * hz / rate as f64;
                let env = ((t - from) / 0.04).min(1.0).min((to - t) / 0.05).max(0.0);
                x[i] = (0.4 * env * phase.sin()) as f32;
            }
        }
        x
    }

    #[test]
    fn a_voice_alone_gives_its_beat_grid() {
        let rate = 44_100;
        let x = sung(116.4, 0.8, 24, rate);
        let on = onsets(&x, &x, rate);
        assert!(on.len() >= 40, "{}", on.len());
        let g = beat_grid(&on, 4);
        assert!((g[0].bpm - 116.4).abs() < 0.3, "{g:?}");
        let beat = 60.0 / 116.4;
        let bars = (0.8 - g[0].first_downbeat) / (4.0 * beat);
        assert!((bars - bars.round()).abs() < 0.03, "{g:?}");
    }
}
