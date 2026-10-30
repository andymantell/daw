// SPDX-License-Identifier: GPL-3.0-or-later
//! Automatic string and fret assignment for notes that come without them
//! (MIDI files, notes typed in the note editor, audio-to-MIDI later).
//!
//! Notes sounding together are a "slice". For each slice we list the ways
//! its notes can be fingered, give each a cost for the hand shape (stretch,
//! height up the neck) and each change between slices a cost for how far
//! the hand moves, and pick the cheapest path through the whole part. This
//! is the shortest-path approach that the published work on guitar
//! fingering optimisation uses. Positions the user fixed (locked notes,
//! locked hand positions) are kept.

use crate::score::{Part, PartKind, Technique};
use crate::tuning::Tuning;

#[derive(Clone, Debug)]
pub struct AssignOptions {
    /// Highest fret to use, counted from the capo.
    pub max_fret: u8,
    /// Reassign notes that already have a string and fret (locked notes
    /// are always kept).
    pub reassign: bool,
    /// Hand positions the user locked for ranges of bars.
    pub hand_locks: Vec<HandLock>,
}

impl Default for AssignOptions {
    fn default() -> Self {
        Self {
            max_fret: 22,
            reassign: false,
            hand_locks: Vec::new(),
        }
    }
}

/// Keep the fretting hand at `fret` (the first finger's fret) for bars
/// `first_bar..=last_bar` (from 0), unless a note can't be reached there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandLock {
    pub first_bar: usize,
    pub last_bar: usize,
    pub fret: u8,
}

/// A note the assigner couldn't place, as (bar, event, note) indices.
pub type NoteRef = (usize, usize, usize);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssignReport {
    pub assigned: usize,
    /// Notes out of the instrument's range, or that don't fit on the
    /// strings left in a chord. Their string and fret stay empty.
    pub unplayable: Vec<NoteRef>,
}

/// Frets a hand covers without stretching (four fingers, four frets).
const COMFORT_SPAN: i32 = 3;
const MAX_CANDIDATES: usize = 400;

struct Slice {
    bar: usize,
    /// Notes to choose positions for.
    free: Vec<(NoteRef, u8)>,
    /// Notes whose positions are fixed: (string, fret).
    fixed: Vec<(u8, u8)>,
    /// Strings of fixed notes that hand on to the next note on the same
    /// string (hammer-on, pull-off, slide).
    fixed_legato: Vec<u8>,
    legato: Vec<bool>,
}

#[derive(Clone)]
struct Candidate {
    /// String and fret for each free note, in order.
    choice: Vec<(u8, u8)>,
    cost: f32,
    hand: Option<i32>,
    legato_strings: Vec<u8>,
    strings: Vec<u8>,
}

/// Assign strings and frets to a tab part's notes. Parts that aren't tab
/// are left alone.
pub fn assign(part: &mut Part, options: &AssignOptions) -> AssignReport {
    let PartKind::Tab(setup) = &part.kind else {
        return AssignReport::default();
    };
    let tuning = setup.tuning.clone();
    let capo = setup.capo;
    let mut report = AssignReport::default();

    // Group events into slices by (bar, tick).
    let mut slices: Vec<Slice> = Vec::new();
    let mut last_key = None;
    for (bar, tick, e) in part.positions() {
        if last_key != Some((bar, tick)) {
            slices.push(Slice {
                bar,
                free: Vec::new(),
                fixed: Vec::new(),
                fixed_legato: Vec::new(),
                legato: Vec::new(),
            });
            last_key = Some((bar, tick));
        }
        let slice = slices.last_mut().expect("pushed above");
        for (n, note) in part.bars[bar].events[e].notes.iter().enumerate() {
            let legato = note.techniques.iter().any(|t| {
                matches!(
                    t,
                    Technique::HammerOn | Technique::PullOff | Technique::Slide
                )
            });
            match (note.string, note.fret) {
                (Some(s), Some(f)) if note.locked || !options.reassign => {
                    slice.fixed.push((s, f));
                    if legato {
                        slice.fixed_legato.push(s);
                    }
                }
                _ => {
                    slice.free.push(((bar, e, n), note.pitch));
                    slice.legato.push(legato);
                }
            }
        }
    }

    // Candidates per slice, dropping notes that can't be placed.
    let mut all: Vec<Vec<Candidate>> = Vec::with_capacity(slices.len());
    for slice in &mut slices {
        let lock = options
            .hand_locks
            .iter()
            .find(|l| (l.first_bar..=l.last_bar).contains(&slice.bar))
            .map(|l| i32::from(l.fret));
        loop {
            let c = candidates(slice, &tuning, capo, options.max_fret, lock);
            if !c.is_empty() {
                all.push(c);
                break;
            }
            // Drop the note with the fewest places it could go.
            let worst = (0..slice.free.len())
                .min_by_key(|&i| places(slice.free[i].1, &tuning, capo, options.max_fret).len())
                .expect("an empty slice always has one candidate");
            report.unplayable.push(slice.free[worst].0);
            slice.free.remove(worst);
            slice.legato.remove(worst);
        }
    }

    // Cheapest path through the slices.
    let mut cost: Vec<Vec<f32>> = Vec::with_capacity(all.len());
    let mut back: Vec<Vec<usize>> = Vec::with_capacity(all.len());
    for (i, cands) in all.iter().enumerate() {
        if i == 0 {
            cost.push(cands.iter().map(|c| c.cost).collect());
            back.push(vec![0; cands.len()]);
            continue;
        }
        let prev = &all[i - 1];
        let prev_cost = &cost[i - 1];
        let mut row = Vec::with_capacity(cands.len());
        let mut brow = Vec::with_capacity(cands.len());
        for c in cands {
            let (best, arg) = prev
                .iter()
                .zip(prev_cost)
                .enumerate()
                .map(|(j, (p, pc))| (pc + transition(p, c), j))
                .fold((f32::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a });
            row.push(best + c.cost);
            brow.push(arg);
        }
        cost.push(row);
        back.push(brow);
    }
    let mut pick = vec![0; all.len()];
    if let Some(last) = cost.last() {
        let mut j = last
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(j, _)| j)
            .unwrap_or(0);
        for i in (0..all.len()).rev() {
            pick[i] = j;
            j = back[i][j];
        }
    }

    for (i, slice) in slices.iter().enumerate() {
        let chosen = &all[i][pick[i]];
        for (&((bar, e, n), _), &(string, fret)) in slice.free.iter().zip(&chosen.choice) {
            let note = &mut part.bars[bar].events[e].notes[n];
            note.string = Some(string);
            note.fret = Some(fret);
            report.assigned += 1;
        }
    }
    report
}

/// Every (string, fret) that plays a pitch.
fn places(pitch: u8, tuning: &Tuning, capo: u8, max_fret: u8) -> Vec<(u8, u8)> {
    (1..=tuning.string_count() as u8)
        .filter_map(|s| {
            let open = tuning.open_pitch(s, capo)?;
            let fret = pitch.checked_sub(open)?;
            (fret <= max_fret).then_some((s, fret))
        })
        .collect()
}

fn candidates(
    slice: &Slice,
    tuning: &Tuning,
    capo: u8,
    max_fret: u8,
    lock: Option<i32>,
) -> Vec<Candidate> {
    let options: Vec<Vec<(u8, u8)>> = slice
        .free
        .iter()
        .map(|&(_, p)| places(p, tuning, capo, max_fret))
        .collect();
    let mut out = Vec::new();
    let mut chosen = Vec::with_capacity(options.len());
    let used: Vec<u8> = slice.fixed.iter().map(|&(s, _)| s).collect();
    search(&options, &used, &mut chosen, &mut out);
    out.into_iter()
        .map(|choice| score_shape(slice, choice, lock))
        .collect()
}

fn search(
    options: &[Vec<(u8, u8)>],
    used: &[u8],
    chosen: &mut Vec<(u8, u8)>,
    out: &mut Vec<Vec<(u8, u8)>>,
) {
    if out.len() >= MAX_CANDIDATES {
        return;
    }
    let i = chosen.len();
    if i == options.len() {
        out.push(chosen.clone());
        return;
    }
    for &(s, f) in &options[i] {
        if used.contains(&s) || chosen.iter().any(|&(cs, _)| cs == s) {
            continue;
        }
        chosen.push((s, f));
        search(options, used, chosen, out);
        chosen.pop();
    }
}

fn score_shape(slice: &Slice, choice: Vec<(u8, u8)>, lock: Option<i32>) -> Candidate {
    let all: Vec<(u8, u8)> = choice.iter().chain(&slice.fixed).copied().collect();
    let fretted: Vec<i32> = all
        .iter()
        .filter(|&&(_, f)| f > 0)
        .map(|&(_, f)| i32::from(f))
        .collect();
    let mut cost = 0.0;
    let hand = fretted.iter().copied().min();
    if let (Some(lo), Some(hi)) = (hand, fretted.iter().copied().max()) {
        let stretch = hi - lo - COMFORT_SPAN;
        if stretch > 0 {
            cost += if stretch == 1 {
                2.0
            } else {
                100.0 * stretch as f32
            };
        }
        // A slight preference for lower positions, where tab is easiest to
        // read and most music sits.
        cost += 0.08 * fretted.iter().sum::<i32>() as f32 / fretted.len() as f32;
    }
    if let Some(l) = lock {
        // Open strings count as outside a locked position too.
        let outside = all
            .iter()
            .map(|&(_, f)| i32::from(f))
            .filter(|&f| f < l || f > l + COMFORT_SPAN)
            .count();
        cost += 50.0 * outside as f32;
    }
    let legato_strings = choice
        .iter()
        .zip(&slice.legato)
        .filter(|(_, l)| **l)
        .map(|(&(s, _), _)| s)
        .chain(slice.fixed_legato.iter().copied())
        .collect();
    let strings = all.iter().map(|&(s, _)| s).collect();
    Candidate {
        choice,
        cost,
        hand,
        legato_strings,
        strings,
    }
}

fn transition(prev: &Candidate, next: &Candidate) -> f32 {
    let movement = match (prev.hand, next.hand) {
        (Some(a), Some(b)) => (a - b).abs() as f32,
        // Open strings leave the hand free to move.
        _ => 0.3,
    };
    // A hammer-on, pull-off or slide continues on the same string.
    let broken = prev
        .legato_strings
        .iter()
        .filter(|s| !next.strings.contains(s))
        .count();
    movement + 4.0 * broken as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{Bar, Duration, Event, Note, NoteValue, TabSetup};

    fn tab_part(pitches: &[&[u8]]) -> Part {
        let q = Duration::new(NoteValue::Quarter);
        Part {
            name: "Guitar".into(),
            kind: PartKind::Tab(TabSetup {
                tuning: Tuning::guitar_standard(),
                capo: 0,
                show_staff: false,
                show_rhythm: true,
            }),
            bars: pitches
                .chunks(4)
                .map(|beats| Bar {
                    events: beats
                        .iter()
                        .map(|ps| Event::chord(1, q, ps.iter().map(|&p| Note::new(p)).collect()))
                        .collect(),
                    ..Bar::default()
                })
                .collect(),
        }
    }

    fn frets(part: &Part) -> Vec<Vec<(u8, u8)>> {
        part.bars
            .iter()
            .flat_map(|b| &b.events)
            .map(|e| {
                e.notes
                    .iter()
                    .map(|n| (n.string.unwrap_or(0), n.fret.unwrap_or(0)))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn open_chord_shape() {
        // A C major chord: C3 E3 G3 C4 E4.
        let mut part = tab_part(&[&[48, 52, 55, 60, 64]]);
        let report = assign(&mut part, &AssignOptions::default());
        assert_eq!(report.assigned, 5);
        assert!(report.unplayable.is_empty());
        let mut got = frets(&part)[0].clone();
        got.sort();
        assert_eq!(got, vec![(1, 0), (2, 1), (3, 0), (4, 2), (5, 3)]);
    }

    #[test]
    fn a_scale_stays_in_one_position() {
        // A minor pentatonic from A4 down, played around the fifth fret.
        let mut part = tab_part(&[&[69], &[67], &[64], &[62], &[60], &[57], &[55], &[52]]);
        let options = AssignOptions {
            hand_locks: vec![HandLock {
                first_bar: 0,
                last_bar: 1,
                fret: 5,
            }],
            ..AssignOptions::default()
        };
        assign(&mut part, &options);
        for slice in frets(&part) {
            let (_, fret) = slice[0];
            assert!((5..=8).contains(&fret), "fret {fret} left the position");
        }
        // Unlocked, the same line moves little: no jumps of more than a few
        // frets between neighbouring notes.
        let mut free = tab_part(&[&[69], &[67], &[64], &[62], &[60], &[57], &[55], &[52]]);
        assign(&mut free, &AssignOptions::default());
        let f: Vec<i32> = frets(&free).iter().map(|s| i32::from(s[0].1)).collect();
        for w in f.windows(2) {
            assert!((w[0] - w[1]).abs() <= 5, "jump in {f:?}");
        }
    }

    #[test]
    fn locked_notes_are_kept_and_out_of_range_reported() {
        let mut part = tab_part(&[&[64], &[30]]);
        part.bars[0].events[0].notes[0] = Note {
            locked: true,
            ..Note::at(64, 2, 5)
        };
        let report = assign(&mut part, &AssignOptions::default());
        assert_eq!(frets(&part)[0], vec![(2, 5)]);
        assert_eq!(report.unplayable, vec![(0, 1, 0)]);
        assert_eq!(part.bars[0].events[1].notes[0].string, None);
    }

    #[test]
    fn legato_stays_on_one_string() {
        // A hammer-on from G4 to A4: both could be open strings or frets,
        // but the hammer-on must stay on one string.
        let mut part = tab_part(&[&[67], &[69]]);
        part.bars[0].events[0].notes[0]
            .techniques
            .push(Technique::HammerOn);
        assign(&mut part, &AssignOptions::default());
        let f = frets(&part);
        assert_eq!(f[0][0].0, f[1][0].0);
        assert!(f[1][0].1 > f[0][0].1);
    }

    #[test]
    fn capo_moves_frets() {
        let mut part = tab_part(&[&[45 + 2]]);
        if let PartKind::Tab(setup) = &mut part.kind {
            setup.capo = 2;
        }
        assign(&mut part, &AssignOptions::default());
        assert_eq!(frets(&part)[0], vec![(5, 0)]);
    }
}
