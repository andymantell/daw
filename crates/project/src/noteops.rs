// SPDX-License-Identifier: GPL-3.0-or-later
//! Note editing tools (roadmap 3.1): humanising, legato, keeping notes
//! on a scale, choosing notes by what they are, and groove templates.
//!
//! Each tool works on the notes of one clip that `pick` chooses (the
//! selection, or all of them), with times in ticks from the clip's
//! content start as the clip keeps them. Tools that need the bars take
//! `origin`, where the content starts on the timeline, and a function
//! giving the start of the bar a timeline tick is in.

use crate::{Groove, GrooveStep, Note, Scale};

/// A small random number source (SplitMix64), so humanising can be
/// repeated from a seed in tests.
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Random {
        Random(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// In [−1, 1], more often near 0 than near the ends (the sum of two
    /// uniform numbers), as a player's small errors are.
    pub fn wobble(&mut self) -> f64 {
        self.unit() + self.unit() - 1.0
    }
}

/// How much humanising moves notes, each at most.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Humanise {
    /// Earlier or later, in ticks.
    pub timing: i64,
    /// Softer or harder, in velocity steps.
    pub velocity: u8,
    /// Shorter or longer, in percent of the note's length.
    pub length_pct: f64,
}

impl Default for Humanise {
    fn default() -> Humanise {
        Humanise {
            timing: crate::PPQ / 48,
            velocity: 8,
            length_pct: 10.0,
        }
    }
}

/// Moves the picked notes by small random amounts in time, velocity and
/// length, up to the amounts given. Notes never start before the
/// content start. Returns whether any changed.
pub fn humanise(
    notes: &mut [Note],
    pick: &dyn Fn(&Note) -> bool,
    h: Humanise,
    rng: &mut Random,
) -> bool {
    let mut changed = false;
    for n in notes.iter_mut().filter(|n| pick(n)) {
        let start = (n.start + (rng.wobble() * h.timing as f64).round() as i64).max(0);
        let velocity = (n.velocity as f64 + rng.wobble() * h.velocity as f64)
            .round()
            .clamp(1.0, 127.0) as u8;
        let len =
            ((n.len as f64 * (1.0 + rng.wobble() * h.length_pct / 100.0)).round() as i64).max(1);
        if (start, velocity, len) != (n.start, n.velocity, n.len) {
            (n.start, n.velocity, n.len) = (start, velocity, len);
            changed = true;
        }
    }
    changed
}

/// Makes each picked note last until the next picked note starts (the
/// next chord, for notes played together), less `gap` ticks (a negative
/// gap overlaps them). The last notes keep their length. Returns whether
/// any changed.
pub fn legato(notes: &mut [Note], pick: &dyn Fn(&Note) -> bool, gap: i64) -> bool {
    let mut starts: Vec<i64> = notes.iter().filter(|n| pick(n)).map(|n| n.start).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut changed = false;
    for n in notes.iter_mut().filter(|n| pick(n)) {
        let i = starts.partition_point(|&s| s <= n.start);
        if let Some(&next) = starts.get(i) {
            let len = (next - n.start - gap).max(1);
            if len != n.len {
                n.len = len;
                changed = true;
            }
        }
    }
    changed
}

/// Moves each picked note to the nearest note of `scale`. Returns
/// whether any moved.
pub fn fit_to_scale(notes: &mut [Note], pick: &dyn Fn(&Note) -> bool, scale: Scale) -> bool {
    let mut changed = false;
    for n in notes.iter_mut().filter(|n| pick(n)) {
        let p = scale.nearest(n.pitch);
        if p != n.pitch {
            n.pitch = p;
            changed = true;
        }
    }
    changed
}

/// Which notes to choose, by what they are: each range given (inclusive)
/// must hold for a note to match; `None` matches all.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Filter {
    /// Where it starts, in ticks on the timeline.
    pub time: Option<(i64, i64)>,
    pub pitch: Option<(u8, u8)>,
    pub velocity: Option<(u8, u8)>,
    /// The chance it plays, 0 to 1.
    pub chance: Option<(f64, f64)>,
    /// How long it is, in ticks.
    pub length: Option<(i64, i64)>,
    /// Only notes outside this scale (to find the wrong notes).
    pub off_scale: Option<Scale>,
}

impl Filter {
    /// Whether `n` matches, with the clip's content starting at
    /// `origin` on the timeline.
    pub fn matches(&self, n: &Note, origin: i64) -> bool {
        fn inside<T: PartialOrd>(r: Option<(T, T)>, v: T) -> bool {
            r.is_none_or(|(lo, hi)| v >= lo && v <= hi)
        }
        inside(self.time, origin + n.start)
            && inside(self.pitch, n.pitch)
            && inside(self.velocity, n.velocity)
            && inside(self.chance, n.chance)
            && inside(self.length, n.len)
            && self.off_scale.is_none_or(|s| !s.contains(n.pitch))
    }
}

/// A groove's steps taken from where a part's notes fall: `hits` are
/// each note's (or each sound's) start in ticks on the timeline and how
/// hard it was played (any unit: velocities, or levels from audio).
/// Each hit goes to the grid line nearest it, counted from the start of
/// its bar, and each step of `steps` (repeating every `steps` lines) is
/// the average of the hits on it. Hits further than half a step from
/// every line still go to the nearest.
pub fn groove_steps(
    hits: &[(i64, f64)],
    bar_start: &dyn Fn(i64) -> i64,
    step: i64,
    steps: usize,
) -> Vec<GrooveStep> {
    let step = step.max(1);
    let steps = steps.max(1);
    let mut sums = vec![(0i64, 0f64, 0usize); steps];
    for &(t, level) in hits {
        let bar = bar_start(t);
        let line = ((t - bar) as f64 / step as f64).round() as i64;
        let k = line.rem_euclid(steps as i64) as usize;
        let s = &mut sums[k];
        s.0 += t - (bar + line * step);
        s.1 += level.max(0.0);
        s.2 += 1;
    }
    let played: Vec<_> = sums.iter().filter(|s| s.2 > 0).collect();
    let mean = played.iter().map(|s| s.1 / s.2 as f64).sum::<f64>() / played.len().max(1) as f64;
    sums.iter()
        .map(|&(off, level, count)| {
            if count == 0 {
                GrooveStep {
                    offset: 0,
                    accent: 1.0,
                    played: false,
                }
            } else {
                GrooveStep {
                    offset: (off as f64 / count as f64).round() as i64,
                    accent: if mean > 0.0 {
                        level / count as f64 / mean
                    } else {
                        1.0
                    },
                    played: true,
                }
            }
        })
        .collect()
}

/// Moves the picked notes towards `groove`: each goes to its nearest
/// grid line, pushed by that step's offset, by `timing` (0 to 1) of the
/// way; and its velocity is scaled by that step's accent, by `accent`
/// (0 to 1) of the way. Steps the groove never played keep the grid and
/// the velocity. Returns whether any changed.
pub fn apply_groove(
    notes: &mut [Note],
    pick: &dyn Fn(&Note) -> bool,
    groove: &Groove,
    origin: i64,
    bar_start: &dyn Fn(i64) -> i64,
    timing: f64,
    accent: f64,
) -> bool {
    if groove.is_empty() || groove.step <= 0 {
        return false;
    }
    let (timing, accent) = (timing.clamp(0.0, 1.0), accent.clamp(0.0, 1.0));
    let mut changed = false;
    for n in notes.iter_mut().filter(|n| pick(n)) {
        let t = origin + n.start;
        let bar = bar_start(t);
        let line = ((t - bar) as f64 / groove.step as f64).round() as i64;
        let s = groove.steps[line.rem_euclid(groove.steps.len() as i64) as usize];
        let target = bar + line * groove.step + if s.played { s.offset } else { 0 };
        let start = ((t as f64 + (target - t) as f64 * timing).round() as i64 - origin).max(0);
        let velocity = if s.played {
            (n.velocity as f64 * (1.0 + (s.accent - 1.0) * accent))
                .round()
                .clamp(1.0, 127.0) as u8
        } else {
            n.velocity
        };
        if (start, velocity) != (n.start, n.velocity) {
            (n.start, n.velocity) = (start, velocity);
            changed = true;
        }
    }
    changed
}

/// A wobbling line for a note's expression, `len` ticks long: a sine
/// of `per_beat` cycles a beat about `centre`, `depth` either side,
/// with eight points a cycle. Pitch vibrato, say, or a pulsing timbre.
pub fn wobble(len: i64, per_beat: f64, centre: f64, depth: f64) -> Vec<crate::AutoPoint> {
    let cycle = (crate::PPQ as f64 / per_beat.max(0.01)).max(8.0);
    let step = (cycle / 8.0).max(1.0);
    let mut out = Vec::new();
    let mut t = 0.0;
    while t < len as f64 {
        let v = centre + depth * (t / cycle * std::f64::consts::TAU).sin();
        out.push(crate::AutoPoint {
            at: t.round() as i64,
            value: v,
        });
        t += step;
    }
    out.dedup_by_key(|p| p.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PPQ, ScaleKind};

    fn all(_: &Note) -> bool {
        true
    }

    /// Bars of four beats from 0.
    fn bars(t: i64) -> i64 {
        t.div_euclid(PPQ * 4) * PPQ * 4
    }

    #[test]
    fn humanising_stays_within_the_amounts_and_repeats_from_a_seed() {
        let make = || -> Vec<Note> {
            (0..64)
                .map(|i| Note::new(PPQ + i * PPQ / 4, PPQ / 4, 60, 100))
                .collect()
        };
        let h = Humanise {
            timing: 10,
            velocity: 5,
            length_pct: 20.0,
        };
        let mut a = make();
        assert!(humanise(&mut a, &all, h, &mut Random::new(7)));
        let before = make();
        for (n, b) in a.iter().zip(&before) {
            assert!((n.start - b.start).abs() <= 10);
            assert!((n.velocity as i32 - 100).abs() <= 5);
            assert!((n.len - b.len).abs() <= b.len / 5 + 1);
        }
        assert!(a.iter().zip(&before).any(|(n, b)| n.start != b.start));
        let mut again = make();
        humanise(&mut again, &all, h, &mut Random::new(7));
        assert_eq!(
            a.iter()
                .map(|n| (n.start, n.velocity, n.len))
                .collect::<Vec<_>>(),
            again
                .iter()
                .map(|n| (n.start, n.velocity, n.len))
                .collect::<Vec<_>>()
        );
        // Never before the content start.
        let mut first = vec![Note::new(0, PPQ, 60, 100)];
        for seed in 0..20 {
            humanise(&mut first, &all, h, &mut Random::new(seed));
            assert!(first[0].start >= 0);
        }
    }

    #[test]
    fn legato_joins_notes_and_chords_to_the_next() {
        let mut notes = vec![
            Note::new(0, 100, 60, 100),
            Note::new(0, 50, 64, 100),
            Note::new(PPQ, 100, 62, 100),
            Note::new(PPQ * 3, 100, 65, 100),
        ];
        assert!(legato(&mut notes, &all, 0));
        let lens: Vec<i64> = notes.iter().map(|n| n.len).collect();
        assert_eq!(lens, [PPQ, PPQ, PPQ * 2, 100], "the last keeps its length");
        legato(&mut notes, &all, 10);
        assert_eq!(notes[0].len, PPQ - 10);
        legato(&mut notes, &all, -20);
        assert_eq!(notes[2].len, PPQ * 2 + 20, "a negative gap overlaps");
        // Only the picked notes count: the second chord note alone has
        // no next.
        let mut notes = vec![Note::new(0, 10, 60, 100), Note::new(PPQ, 10, 62, 100)];
        let first = notes[0].id.clone();
        assert!(!legato(&mut notes, &|n: &Note| n.id == first, 0));
    }

    #[test]
    fn filters_choose_by_each_range() {
        let mut n = Note::new(PPQ, PPQ / 2, 61, 40);
        n.chance = 0.5;
        let origin = PPQ * 4;
        assert!(Filter::default().matches(&n, origin));
        let f = Filter {
            time: Some((PPQ * 5, PPQ * 5)),
            pitch: Some((60, 62)),
            velocity: Some((1, 64)),
            chance: Some((0.0, 0.5)),
            length: Some((0, PPQ)),
            off_scale: Some(Scale::new(0, ScaleKind::Major)),
        };
        assert!(f.matches(&n, origin));
        for g in [
            Filter {
                time: Some((0, PPQ * 4)),
                ..f
            },
            Filter {
                pitch: Some((62, 70)),
                ..f
            },
            Filter {
                velocity: Some((41, 127)),
                ..f
            },
            Filter {
                chance: Some((0.6, 1.0)),
                ..f
            },
            Filter {
                length: Some((PPQ, PPQ * 4)),
                ..f
            },
            Filter {
                off_scale: Some(Scale::new(1, ScaleKind::Major)),
                ..f
            },
        ] {
            assert!(!g.matches(&n, origin), "{g:?}");
        }
    }

    #[test]
    fn fitting_to_a_scale_moves_only_the_wrong_notes() {
        let mut notes = vec![Note::new(0, 1, 61, 100), Note::new(0, 1, 64, 100)];
        assert!(fit_to_scale(
            &mut notes,
            &all,
            Scale::new(0, ScaleKind::Major)
        ));
        assert_eq!((notes[0].pitch, notes[1].pitch), (60, 64));
        assert!(!fit_to_scale(
            &mut notes,
            &all,
            Scale::new(0, ScaleKind::Major)
        ));
    }

    #[test]
    fn a_groove_taken_from_a_swung_part_swings_a_straight_one() {
        // Eighths over two bars, every second one 80 ticks late and
        // softer, the first of each beat harder.
        let e = PPQ / 2;
        let hits: Vec<(i64, f64)> = (0..16)
            .map(|i| {
                let late = if i % 2 == 1 { 80 } else { 0 };
                (i * e + late, if i % 2 == 0 { 120.0 } else { 60.0 })
            })
            .collect();
        let steps = groove_steps(&hits, &bars, e, 8);
        assert_eq!(steps.len(), 8);
        assert_eq!(steps[0].offset, 0);
        assert_eq!(steps[1].offset, 80);
        assert!(steps[0].accent > 1.2 && steps[1].accent < 0.8, "{steps:?}");
        let groove = Groove {
            id: "g".into(),
            name: "Swing".into(),
            step: e,
            steps,
        };
        assert_eq!(groove.len(), PPQ * 4);
        // A straight part in a clip starting at bar 2 takes the swing.
        let origin = PPQ * 4;
        let mut notes: Vec<Note> = (0..8).map(|i| Note::new(i * e, e, 60, 90)).collect();
        assert!(apply_groove(
            &mut notes, &all, &groove, origin, &bars, 1.0, 1.0
        ));
        assert_eq!(notes[1].start, e + 80);
        assert_eq!(notes[2].start, 2 * e);
        assert!(notes[0].velocity > 90 && notes[1].velocity < 90);
        // Half way, timing only.
        let mut notes: Vec<Note> = (0..8).map(|i| Note::new(i * e, e, 60, 90)).collect();
        apply_groove(&mut notes, &all, &groove, origin, &bars, 0.5, 0.0);
        assert_eq!(notes[1].start, e + 40);
        assert_eq!(notes[0].velocity, 90);
    }

    #[test]
    fn steps_never_played_leave_notes_on_the_grid() {
        let steps = groove_steps(&[(10, 1.0)], &bars, PPQ, 4);
        assert!(steps[0].played && !steps[1].played);
        assert_eq!(steps[0].offset, 10);
        assert_eq!(steps[0].accent, 1.0);
        let g = Groove {
            id: "g".into(),
            name: "G".into(),
            step: PPQ,
            steps,
        };
        let mut notes = vec![Note::new(PPQ + 7, 10, 60, 50)];
        apply_groove(&mut notes, &all, &g, 0, &bars, 1.0, 1.0);
        assert_eq!((notes[0].start, notes[0].velocity), (PPQ, 50));
    }

    #[test]
    fn a_wobble_swings_about_its_centre() {
        let w = wobble(PPQ * 2, 2.0, 0.0, 0.5);
        assert_eq!(w.len(), 32, "eight points a cycle, four cycles");
        assert_eq!(w[0].value, 0.0);
        assert!((w[2].value - 0.5).abs() < 1e-9, "the top a quarter in");
        assert!(
            w.iter()
                .all(|p| p.value.abs() <= 0.5 + 1e-9 && p.at < PPQ * 2)
        );
    }
}
