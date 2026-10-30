// SPDX-License-Identifier: GPL-3.0-or-later
//! Lesson depth (roadmap 3.7): a practice log, lessons, and progress on
//! assignments.
//!
//! - `practice_log` holds, under each person's key, a `sessions` map from
//!   id to a stretch of playing (when, how long, which bars were looped,
//!   how many times round, and the top speed), and a `done` map from
//!   assignment id to when they said it was done. Each person writes only
//!   under their own key, so commenters may keep a log too (see
//!   `identity.rs`), and two people practising at once never clash.
//! - `lessons` holds, by id, a lesson's `title`, `author`, `created` and
//!   the `snapshot` taken when it started, so the project can be seen as
//!   it was at each lesson.
//!
//! Assignments themselves are part of the song ([`crate::Assignment`]),
//! set and changed like any edit. How far someone has got with one is
//! worked out from the log ([`progress`]), never typed in, so teacher and
//! student see the same thing.
//!
//! None of this is undoable: it records what happened, not edits.

use std::collections::BTreeMap;

use loro::{LoroMap, LoroValue};

use crate::doc::{QUIET, Writer, float, int, string};
use crate::{Assignment, ProjectDoc};

const LOG: &str = "practice_log";
const LESSONS: &str = "lessons";

/// A lesson's title is cut to this many characters.
pub const MAX_LESSON_TITLE: usize = 80;

/// A stretch of practice: playing from pressing play to stopping, or
/// until the loop moved to other bars.
#[derive(Clone, Debug, PartialEq)]
pub struct PracticeSession {
    pub id: String,
    /// Whose it is: the member's public key, as hex, or this computer's
    /// id in a project without members.
    pub who: String,
    /// When it began, in milliseconds since 1970.
    pub started: i64,
    /// Time spent playing, in seconds.
    pub seconds: f64,
    /// The bars looped, as the loop has them, or `None` when playing
    /// without the loop.
    pub bars: Option<(i64, i64)>,
    /// Times round the loop.
    pub loops: u64,
    /// The fastest speed played at, percent.
    pub top_speed: f64,
    /// The tempo that speed made at the start of the bars, in beats per
    /// minute.
    pub top_bpm: f64,
}

/// Someone saying they have done an assignment.
#[derive(Clone, Debug, PartialEq)]
pub struct DoneMark {
    pub who: String,
    pub assignment: String,
    /// When, in milliseconds since 1970.
    pub at: i64,
}

/// A lesson: a name and a date, and the snapshot of the project taken
/// when it began.
#[derive(Clone, Debug, PartialEq)]
pub struct Lesson {
    pub id: String,
    pub title: String,
    /// Who started it: the member's public key, as hex, or empty.
    pub author: String,
    /// When, in milliseconds since 1970.
    pub created: i64,
    /// The snapshot taken then.
    pub snapshot: String,
}

/// How one person is getting on with an assignment.
#[derive(Clone, Debug, PartialEq)]
pub struct AssignmentProgress {
    pub who: String,
    /// The fastest tempo they have played all the assignment's bars at,
    /// in beats per minute, or 0 if they haven't yet.
    pub best_bpm: f64,
    /// Time spent on any of its bars, in seconds.
    pub seconds: f64,
    /// Times round a loop that took in any of its bars.
    pub loops: u64,
    pub sessions: usize,
    /// When they said it was done, if they have.
    pub done: Option<i64>,
}

impl AssignmentProgress {
    /// How near the target tempo they are, 0 to 1.
    pub fn fraction(&self, a: &Assignment) -> f64 {
        if a.bpm <= 0.0 {
            return 0.0;
        }
        (self.best_bpm / a.bpm).clamp(0.0, 1.0)
    }

    /// Whether they have played it at the target tempo. Tempos are
    /// rounded to a tenth, so 89.96 bpm counts as 90.
    pub fn reached(&self, a: &Assignment) -> bool {
        (self.best_bpm * 10.0).round() >= (a.bpm * 10.0).round()
    }
}

/// Each person's progress on `a`, from the sessions since it was set,
/// in the order of their keys. Someone appears once they have played
/// any of its bars, or said it is done.
///
/// A session counts towards the time if its loop took in any of the
/// assignment's bars, and towards the tempo only if it looped all of
/// them: playing bars 9–12 fast is not playing 9–16 fast. Playing
/// without the loop counts towards neither.
pub fn progress(
    a: &Assignment,
    log: &[PracticeSession],
    done: &[DoneMark],
) -> Vec<AssignmentProgress> {
    let mut by: BTreeMap<&str, AssignmentProgress> = BTreeMap::new();
    for s in log.iter().filter(|s| s.started >= a.created) {
        let Some((from, to)) = s.bars else { continue };
        if to <= a.bars.0 || from >= a.bars.1 {
            continue;
        }
        let p = entry(&mut by, &s.who);
        p.seconds += s.seconds;
        p.loops += s.loops;
        p.sessions += 1;
        if from <= a.bars.0 && to >= a.bars.1 {
            p.best_bpm = p.best_bpm.max(s.top_bpm);
        }
    }
    for d in done.iter().filter(|d| d.assignment == a.id) {
        entry(&mut by, &d.who).done = Some(d.at);
    }
    by.into_values().collect()
}

fn entry<'a, 'b>(
    by: &'b mut BTreeMap<&'a str, AssignmentProgress>,
    who: &'a str,
) -> &'b mut AssignmentProgress {
    by.entry(who).or_insert_with(|| AssignmentProgress {
        who: who.to_string(),
        best_bpm: 0.0,
        seconds: 0.0,
        loops: 0,
        sessions: 0,
        done: None,
    })
}

impl ProjectDoc {
    /// Writes a practice session under `s.who`, or updates it if it is
    /// there already (a session still going, written at a save). True
    /// if anything changed.
    pub fn log_practice(&mut self, s: &PracticeSession) -> bool {
        let mut w = Writer::default();
        let mine = w.fixed(&self.doc.get_map(LOG), &s.who);
        let sessions = w.fixed(&mine, "sessions");
        let m = w.child(&sessions, &s.id);
        w.put(&m, "started", s.started);
        w.put(&m, "seconds", s.seconds);
        let (from, to) = match s.bars {
            Some((f, t)) => (LoroValue::I64(f), LoroValue::I64(t)),
            None => (LoroValue::Null, LoroValue::Null),
        };
        w.put(&m, "from", from);
        w.put(&m, "to", to);
        w.put(&m, "loops", s.loops as i64);
        w.put(&m, "top_speed", s.top_speed);
        w.put(&m, "top_bpm", s.top_bpm);
        self.commit_quiet(w.changed)
    }

    /// Takes a session out of `who`'s log. False if it wasn't there.
    pub fn forget_practice(&mut self, who: &str, id: &str) -> bool {
        let Some(sessions) = sub_map(&self.doc.get_map(LOG), &[who, "sessions"]) else {
            return false;
        };
        if sessions.get(id).is_none() {
            return false;
        }
        let _ = sessions.delete(id);
        self.commit_quiet(true)
    }

    /// Everyone's practice sessions, oldest first.
    pub fn practice_log(&self) -> Vec<PracticeSession> {
        let mut v = Vec::new();
        let LoroValue::Map(all) = self.doc.get_map(LOG).get_deep_value() else {
            return v;
        };
        for (who, mine) in all.iter() {
            let LoroValue::Map(mine) = mine else { continue };
            let Some(LoroValue::Map(sessions)) = mine.get("sessions") else {
                continue;
            };
            for (id, m) in sessions.iter() {
                let LoroValue::Map(m) = m else { continue };
                let bars = match (int(m, "from"), int(m, "to")) {
                    (Some(f), Some(t)) if t > f => Some((f, t)),
                    _ => None,
                };
                v.push(PracticeSession {
                    id: id.to_string(),
                    who: who.to_string(),
                    started: int(m, "started").unwrap_or(0),
                    seconds: float(m, "seconds").unwrap_or(0.0).max(0.0),
                    bars,
                    loops: int(m, "loops").unwrap_or(0).max(0) as u64,
                    top_speed: float(m, "top_speed").unwrap_or(100.0),
                    top_bpm: float(m, "top_bpm").unwrap_or(0.0),
                });
            }
        }
        v.sort_by(|a, b| a.started.cmp(&b.started).then(a.id.cmp(&b.id)));
        v
    }

    /// Says `who` has done an assignment, at `at`, or with `None` takes
    /// that back. True if anything changed.
    pub fn mark_done(&mut self, who: &str, assignment: &str, at: Option<i64>) -> bool {
        let mut w = Writer::default();
        let mine = w.fixed(&self.doc.get_map(LOG), who);
        let done = w.fixed(&mine, "done");
        match at {
            Some(t) => {
                w.put(&done, assignment, t);
            }
            None if done.get(assignment).is_some() => w.delete(&done, assignment),
            None => {}
        }
        self.commit_quiet(w.changed)
    }

    /// Everyone's done marks.
    pub fn done_marks(&self) -> Vec<DoneMark> {
        let mut v = Vec::new();
        let LoroValue::Map(all) = self.doc.get_map(LOG).get_deep_value() else {
            return v;
        };
        for (who, mine) in all.iter() {
            let LoroValue::Map(mine) = mine else { continue };
            let Some(LoroValue::Map(done)) = mine.get("done") else {
                continue;
            };
            for (a, at) in done.iter() {
                if let LoroValue::I64(at) = at {
                    v.push(DoneMark {
                        who: who.to_string(),
                        assignment: a.to_string(),
                        at: *at,
                    });
                }
            }
        }
        v
    }

    /// Starts a lesson: takes a snapshot named after it, and adds it to
    /// the list. Returns the lesson's id.
    pub fn start_lesson(&mut self, title: &str, author: &str, created: i64) -> String {
        let title = clean(title);
        let snapshot = self.take_snapshot(&title, author, created);
        let id = crate::new_id();
        let mut w = Writer::default();
        let m = w.child(&self.doc.get_map(LESSONS), &id);
        w.put(&m, "title", title.as_str());
        w.put(&m, "author", author);
        w.put(&m, "created", created);
        w.put(&m, "snapshot", snapshot.as_str());
        self.commit_quiet(true);
        id
    }

    /// Every lesson, oldest first.
    pub fn lessons(&self) -> Vec<Lesson> {
        let LoroValue::Map(all) = self.doc.get_map(LESSONS).get_deep_value() else {
            return Vec::new();
        };
        let mut v: Vec<Lesson> = (all.iter())
            .filter_map(|(id, m)| {
                let LoroValue::Map(m) = m else { return None };
                Some(Lesson {
                    id: id.to_string(),
                    title: string(m, "title").unwrap_or_default(),
                    author: string(m, "author").unwrap_or_default(),
                    created: int(m, "created").unwrap_or(0),
                    snapshot: string(m, "snapshot").unwrap_or_default(),
                })
            })
            .collect();
        v.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        v
    }

    /// Gives a lesson a new title. False if there is no such lesson.
    pub fn rename_lesson(&mut self, id: &str, title: &str) -> bool {
        let Some(m) = sub_map(&self.doc.get_map(LESSONS), &[id]) else {
            return false;
        };
        let mut w = Writer::default();
        w.put(&m, "title", clean(title).as_str());
        self.commit_quiet(w.changed);
        true
    }

    /// Takes a lesson off the list. Its snapshot stays, under the
    /// snapshots. False if there was no such lesson.
    pub fn delete_lesson(&mut self, id: &str) -> bool {
        let all = self.doc.get_map(LESSONS);
        if all.get(id).is_none() {
            return false;
        }
        let _ = all.delete(id);
        // Assignments set in it stay, and show as set in no lesson.
        self.commit_quiet(true)
    }

    fn commit_quiet(&mut self, changed: bool) -> bool {
        if changed {
            self.doc.set_next_commit_origin(QUIET);
            self.doc.commit();
        }
        changed
    }
}

/// The map at `path` under `m`, if it is there.
fn sub_map(m: &LoroMap, path: &[&str]) -> Option<LoroMap> {
    let mut m = m.clone();
    for k in path {
        m = m.get(k)?.into_container().ok()?.into_map().ok()?;
    }
    Some(m)
}

/// A title with no line breaks or surrounding space, not too long.
fn clean(title: &str) -> String {
    let one_line: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.trim().chars().take(MAX_LESSON_TITLE).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    fn session(who: &str, started: i64, bars: Option<(i64, i64)>, bpm: f64) -> PracticeSession {
        PracticeSession {
            id: crate::new_id(),
            who: who.into(),
            started,
            seconds: 60.0,
            bars,
            loops: 3,
            top_speed: 75.0,
            top_bpm: bpm,
        }
    }

    fn assignment() -> Assignment {
        Assignment {
            id: "a1".into(),
            bars: (9, 17),
            bpm: 90.0,
            due: 0,
            note: String::new(),
            lesson: None,
            author: String::new(),
            created: 100,
        }
    }

    #[test]
    fn progress_counts_sessions_on_its_bars_since_it_was_set() {
        let a = assignment();
        let log = vec![
            // Before it was set.
            session("sam", 50, Some((9, 17)), 120.0),
            // All its bars, then part of them faster, then elsewhere.
            session("sam", 200, Some((9, 17)), 72.0),
            session("sam", 300, Some((9, 13)), 95.0),
            session("sam", 400, Some((1, 9)), 120.0),
            // Without the loop.
            session("sam", 500, None, 120.0),
            // A wider loop counts for both.
            session("jo", 600, Some((5, 21)), 91.0),
        ];
        let done = vec![DoneMark {
            who: "jo".into(),
            assignment: "a1".into(),
            at: 700,
        }];
        let p = progress(&a, &log, &done);
        assert_eq!(p.len(), 2);
        let (jo, sam) = (&p[0], &p[1]);
        assert_eq!(sam.who, "sam");
        assert_eq!(sam.best_bpm, 72.0);
        assert_eq!(sam.sessions, 2);
        assert_eq!(sam.seconds, 120.0);
        assert_eq!(sam.loops, 6);
        assert_eq!(sam.done, None);
        assert!((sam.fraction(&a) - 0.8).abs() < 1e-9);
        assert!(!sam.reached(&a));
        assert_eq!(jo.best_bpm, 91.0);
        assert!(jo.reached(&a));
        assert_eq!(jo.fraction(&a), 1.0);
        assert_eq!(jo.done, Some(700));
    }

    #[test]
    fn the_log_and_lessons_survive_saving_and_merge() {
        let mut d = ProjectDoc::new(1);
        let mut s = session("me", 1_000, Some((1, 5)), 100.0);
        assert!(d.log_practice(&s));
        assert!(!d.log_practice(&s), "nothing new to write");
        // A session still going is updated in place.
        s.seconds = 90.0;
        s.loops = 4;
        assert!(d.log_practice(&s));
        let free = session("me", 2_000, None, 120.0);
        d.log_practice(&free);
        d.mark_done("me", "a1", Some(3_000));
        let lesson = d.start_lesson("  Lesson 1\n", "me", 4_000);

        let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
        assert_eq!(back.practice_log(), vec![s.clone(), free.clone()]);
        assert_eq!(
            back.done_marks(),
            vec![DoneMark {
                who: "me".into(),
                assignment: "a1".into(),
                at: 3_000
            }]
        );
        let lessons = back.lessons();
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].id, lesson);
        assert_eq!(lessons[0].title, "Lesson 1");
        assert_eq!(lessons[0].created, 4_000);
        let snaps = back.snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].id, lessons[0].snapshot);
        assert_eq!(snaps[0].name, "Lesson 1");

        // Two people practising at once keep both.
        let mut other = ProjectDoc::from_bytes(&d.to_bytes(), 3).unwrap();
        other.log_practice(&session("them", 5_000, Some((1, 5)), 90.0));
        d.log_practice(&session("me", 5_500, Some((1, 5)), 95.0));
        d.merge(&other.to_bytes()).unwrap();
        assert_eq!(d.practice_log().len(), 4);

        assert!(d.mark_done("me", "a1", None));
        assert!(d.done_marks().is_empty());
        assert!(d.forget_practice("me", &free.id));
        assert_eq!(d.practice_log().len(), 3);
        assert!(d.rename_lesson(&lesson, "First lesson"));
        assert_eq!(d.lessons()[0].title, "First lesson");
        assert!(d.delete_lesson(&lesson));
        assert!(d.lessons().is_empty());
        assert_eq!(d.snapshots().len(), 1, "the snapshot stays");
    }

    #[test]
    fn none_of_it_is_undone() {
        let mut d = ProjectDoc::new(1);
        let p = Project {
            tempo_bpm: 100.0,
            ..Project::default()
        };
        d.set(&p);
        d.log_practice(&session("me", 1_000, Some((1, 5)), 100.0));
        d.start_lesson("Lesson 1", "me", 2_000);
        assert!(d.undo());
        assert_eq!(d.get().tempo_bpm, 120.0);
        assert_eq!(d.practice_log().len(), 1);
        assert_eq!(d.lessons().len(), 1);
    }

    #[test]
    fn assignments_are_kept_and_undoable_and_survive_a_restore() {
        let mut d = ProjectDoc::new(1);
        let lesson = d.start_lesson("Lesson 1", "me", 1_000);
        let mut p = d.get();
        p.assignments.push(Assignment {
            lesson: Some(lesson.clone()),
            note: "Keep the strumming even".into(),
            due: 9_000,
            ..assignment()
        });
        d.set(&p);
        let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap();
        assert_eq!(back.get().assignments, p.assignments);

        // Restoring the lesson's snapshot keeps what was set since.
        let snap = d.lessons()[0].snapshot.clone();
        let restored = d.restore_snapshot(&snap).unwrap();
        assert_eq!(restored.assignments, p.assignments);

        // Nothing else had changed, so undo takes back the assignment.
        assert!(d.undo());
        assert!(d.get().assignments.is_empty());
    }
}
