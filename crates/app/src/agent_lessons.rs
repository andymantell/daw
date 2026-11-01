// SPDX-License-Identifier: GPL-3.0-or-later
//! Assistants' tools for lessons (roadmap 2.11, 3.7): the lesson notes
//! and their tab blocks, assignments, and lessons themselves, as the
//! Notes and Lessons panes change them.

use daw_project::{Assignment, TabBlock};
use serde_json::{Value, json};

use crate::App;
use crate::agent::{Access, Tool};

/// What the tools here answer: a value, and the undo step's name.
type Done = Result<(Value, Option<String>), String>;

pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        name: "get_lessons",
        description: "The lesson notes and their tab blocks (each pinned to a section's \
                      marker, or the whole song), the assignments (bars to play at a \
                      tempo by a date) with who has done them, and the lessons started.",
        access: Access::Read,
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "edit_lesson_notes",
        description: "Changes the lesson notes (text: # for a heading, - for a list, \
                      **bold**), and adds, changes or removes tab blocks. One undo step.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "text": {"type": "string", "description": "The notes' whole new text."},
                "add_tabs": {"type": "array", "items": {"type": "object", "properties": {
                    "section": {"type": "string", "description": "A marker's id; the whole song if left out."},
                    "title": {"type": "string"},
                    "text": {"type": "string", "description": "The tab, as text."}
                }, "required": ["text"]}},
                "change_tabs": {"type": "array", "items": {"type": "object", "properties": {
                    "id": {"type": "string"},
                    "section": {"type": "string", "description": "A marker's id, or \"none\" for the whole song."},
                    "title": {"type": "string"},
                    "text": {"type": "string"}
                }, "required": ["id"]}},
                "remove_tabs": {"type": "array", "items": {"type": "string"}}
            }})
        },
    },
    Tool {
        name: "edit_assignments",
        description: "Sets, changes or removes assignments (bars to play at a tempo by a \
                      date), says this user has done one (or takes it back), or practises \
                      one: loops its bars with the tempo trainer aiming at its tempo.",
        access: Access::Edit,
        schema: || {
            let fields = json!({
                "from_bar": {"type": "integer", "description": "From the start of this bar (from 1)."},
                "to_bar": {"type": "integer", "description": "To the end of this bar."},
                "bpm": {"type": "number", "description": "The tempo to reach."},
                "due_days": {"type": "integer", "description": "Days from today; no date if left out or 0."},
                "note": {"type": "string"}
            });
            let mut change = fields.clone();
            change["id"] = json!({"type": "string"});
            json!({"type": "object", "properties": {
                "add": {"type": "array", "items": {"type": "object", "properties": fields,
                        "required": ["from_bar", "to_bar", "bpm"]}},
                "change": {"type": "array", "items": {"type": "object", "properties": change,
                           "required": ["id"]}},
                "remove": {"type": "array", "items": {"type": "string"}},
                "done": {"type": "object", "properties": {
                    "id": {"type": "string"}, "done": {"type": "boolean"}
                }, "required": ["id", "done"]},
                "practise": {"type": "string", "description": "An assignment's id."}
            }})
        },
    },
    Tool {
        name: "edit_lessons",
        description: "Starts a lesson (a snapshot named after it is taken, and practice \
                      from then on counts towards it), renames one, or takes one off the \
                      list (its snapshot stays), or takes a practice session out of this \
                      user's log.",
        access: Access::Edit,
        schema: || {
            json!({"type": "object", "properties": {
                "start": {"type": "string", "description": "The new lesson's title."},
                "rename": {"type": "object", "properties": {
                    "id": {"type": "string"}, "title": {"type": "string"}
                }, "required": ["id", "title"]},
                "remove": {"type": "string", "description": "A lesson's id."},
                "forget_practice": {"type": "string",
                    "description": "One of this user's practice sessions, to take out of their log."}
            }})
        },
    },
];

fn now_ms() -> i64 {
    crate::lessons::now_ms()
}

impl App {
    pub(crate) fn lessons_read(&mut self, name: &str) -> Option<Result<Value, String>> {
        (name == "get_lessons").then(|| Ok(self.lessons_value()))
    }

    pub(crate) fn lessons_tool(
        &mut self,
        ctx: &eframe::egui::Context,
        name: &str,
        args: &Value,
    ) -> Option<Done> {
        Some(match name {
            "edit_lesson_notes" => self.edit_lesson_notes(args),
            "edit_assignments" => self.edit_assignments(ctx, args),
            "edit_lessons" => self.edit_lessons(ctx, args),
            _ => return None,
        })
    }

    fn lessons_value(&self) -> Value {
        let p = self.project();
        let done = self.proj.doc.done_marks();
        let tabs: Vec<Value> = (p.tabs.iter())
            .map(|t| json!({"id": t.id, "section": t.marker, "title": t.title, "text": t.text}))
            .collect();
        let assignments: Vec<Value> = (p.assignments.iter())
            .map(|a| {
                let by: Vec<Value> = (done.iter())
                    .filter(|d| d.assignment == a.id)
                    .map(|d| json!({"who": d.who, "at_ms": d.at}))
                    .collect();
                json!({
                    "id": a.id, "from_bar": a.bars.0, "to_bar": a.bars.1 - 1, "bpm": a.bpm,
                    "due_ms": (a.due > 0).then_some(a.due), "note": a.note, "done_by": by,
                })
            })
            .collect();
        let lessons: Vec<Value> = (self.proj.doc.lessons().iter())
            .map(|l| json!({"id": l.id, "title": l.title, "started_ms": l.created}))
            .collect();
        let practice: Vec<Value> = (self.proj.doc.practice_log().iter())
            .map(|s| {
                json!({"id": s.id, "who": s.who, "started_ms": s.started, "seconds": s.seconds,
                       "bars": s.bars.map(|(f, t)| json!({"from_bar": f, "to_bar": t - 1})),
                       "loops": s.loops, "top_speed_percent": s.top_speed})
            })
            .collect();
        json!({"notes": p.notes, "tabs": tabs, "assignments": assignments, "lessons": lessons,
               "practice": practice})
    }

    fn edit_lesson_notes(&mut self, args: &Value) -> Done {
        let author = self.my_author_id();
        self.edit_model(|p| {
            if let Some(t) = args["text"].as_str() {
                p.notes = t.to_string();
            }
            let marker = |p: &daw_project::Project, m: &str| -> Result<Option<String>, String> {
                match m {
                    "none" => Ok(None),
                    m if p.markers.iter().any(|x| x.id == m) => Ok(Some(m.into())),
                    m => Err(format!("There is no marker {m}.")),
                }
            };
            let mut added = Vec::new();
            for a in args["add_tabs"].as_array().into_iter().flatten() {
                let t = TabBlock {
                    id: daw_project::new_id(),
                    marker: marker(p, a["section"].as_str().unwrap_or("none"))?,
                    title: a["title"].as_str().unwrap_or("").into(),
                    text: a["text"]
                        .as_str()
                        .ok_or("Give each tab block its text.")?
                        .into(),
                    author: author.clone(),
                    created: now_ms(),
                };
                added.push(t.id.clone());
                p.tabs.push(t);
            }
            for c in args["change_tabs"].as_array().into_iter().flatten() {
                let id = c["id"].as_str().ok_or("Give the tab block's id.")?;
                let m = c["section"].as_str().map(|m| marker(p, m)).transpose()?;
                let t = (p.tabs.iter_mut())
                    .find(|t| t.id == id)
                    .ok_or_else(|| format!("There is no tab block {id}."))?;
                if let Some(m) = m {
                    t.marker = m;
                }
                if let Some(s) = c["title"].as_str() {
                    t.title = s.into();
                }
                if let Some(s) = c["text"].as_str() {
                    t.text = s.into();
                }
            }
            for id in args["remove_tabs"].as_array().into_iter().flatten() {
                let id = id.as_str().unwrap_or("");
                if !p.tabs.iter().any(|t| t.id == id) {
                    return Err(format!("There is no tab block {id}."));
                }
                p.tabs.retain(|t| t.id != id);
            }
            Ok(json!({"added_tabs": added}))
        })
        .map(|v| (v, Some("Change the lesson notes".into())))
    }

    fn edit_assignments(&mut self, ctx: &eframe::egui::Context, args: &Value) -> Done {
        let author = self.my_author_id();
        let now = now_ms();
        let fill = |a: &mut Assignment, v: &Value| -> Result<(), String> {
            if let Some(f) = v["from_bar"].as_i64() {
                a.bars.0 = f.max(1);
            }
            if let Some(t) = v["to_bar"].as_i64() {
                a.bars.1 = t + 1;
            }
            if a.bars.1 <= a.bars.0 {
                return Err("The last bar can't come before the first.".into());
            }
            if let Some(b) = v["bpm"].as_f64() {
                if !(20.0..=400.0).contains(&b) {
                    return Err("The tempo goes from 20 to 400 bpm.".into());
                }
                a.bpm = b;
            }
            if let Some(d) = v["due_days"].as_i64() {
                a.due = if d > 0 {
                    now + d * crate::lessons::DAY_MS
                } else {
                    0
                };
            }
            if let Some(n) = v["note"].as_str() {
                a.note = n.into();
            }
            Ok(())
        };
        let mut v = self.edit_model(|p| {
            let mut added = Vec::new();
            for x in args["add"].as_array().into_iter().flatten() {
                let mut a = Assignment {
                    id: daw_project::new_id(),
                    bars: (1, 2),
                    bpm: 100.0,
                    due: 0,
                    note: String::new(),
                    lesson: None,
                    author: author.clone(),
                    created: now,
                };
                fill(&mut a, x)?;
                added.push(a.id.clone());
                p.assignments.push(a);
            }
            for x in args["change"].as_array().into_iter().flatten() {
                let id = x["id"].as_str().ok_or("Give the assignment's id.")?;
                let a = (p.assignments.iter_mut())
                    .find(|a| a.id == id)
                    .ok_or_else(|| format!("There is no assignment {id}."))?;
                fill(a, x)?;
            }
            for id in args["remove"].as_array().into_iter().flatten() {
                let id = id.as_str().unwrap_or("");
                if !p.assignments.iter().any(|a| a.id == id) {
                    return Err(format!("There is no assignment {id}."));
                }
                p.assignments.retain(|a| a.id != id);
            }
            Ok(json!({"added": added}))
        })?;
        let known = |app: &App, id: &str| -> Result<Assignment, String> {
            (app.assignments.iter())
                .find(|a| a.id == id)
                .cloned()
                .ok_or_else(|| format!("There is no assignment {id}."))
        };
        let mut out = crate::lessons::Outcome::default();
        if let Some(d) = args["done"].as_object() {
            let id = d.get("id").and_then(Value::as_str).unwrap_or("");
            known(self, id)?;
            out.done = Some((
                id.into(),
                d.get("done").and_then(Value::as_bool).unwrap_or(true),
            ));
        }
        if let Some(id) = args["practise"].as_str() {
            out.practise = Some(known(self, id)?);
            v["practising"] = json!(id);
        }
        let (me, author) = (self.my_log_id(), self.my_author_id());
        self.lessons_outcome(ctx, out, &me, &author);
        Ok((v, Some("Change assignments".into())))
    }

    fn edit_lessons(&mut self, ctx: &eframe::egui::Context, args: &Value) -> Done {
        let lessons = self.proj.doc.lessons();
        let known = |id: &str| -> Result<(), String> {
            lessons
                .iter()
                .any(|l| l.id == id)
                .then_some(())
                .ok_or_else(|| format!("There is no lesson {id}."))
        };
        let mut out = crate::lessons::Outcome::default();
        if let Some(t) = args["start"].as_str() {
            let t = t.trim();
            if t.is_empty() || t.chars().count() > daw_project::MAX_LESSON_TITLE {
                return Err(format!(
                    "Give a title of 1 to {} characters.",
                    daw_project::MAX_LESSON_TITLE
                ));
            }
            out.start_lesson = Some(t.into());
        }
        if let Some(r) = args["rename"].as_object() {
            let id = r.get("id").and_then(Value::as_str).unwrap_or("");
            known(id)?;
            let title = r.get("title").and_then(Value::as_str).unwrap_or("").trim();
            if title.is_empty() {
                return Err("Give the lesson's new title.".into());
            }
            out.rename_lesson = Some((id.into(), title.into()));
        }
        if let Some(id) = args["remove"].as_str() {
            known(id)?;
            out.delete_lesson = Some(id.into());
        }
        if let Some(id) = args["forget_practice"].as_str() {
            let me = self.my_log_id();
            if !(self.proj.doc.practice_log().iter()).any(|s| s.id == id && s.who == me) {
                return Err(format!("There is no practice session {id} of yours."));
            }
            out.forget = Some(id.into());
        }
        let (me, author) = (self.my_log_id(), self.my_author_id());
        self.lessons_outcome(ctx, out, &me, &author);
        Ok((
            json!({"lessons": self.proj.doc.lessons().len()}),
            Some("Change lessons".into()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::driver::Driver;

    #[test]
    fn lesson_notes_tabs_assignments_and_lessons() {
        let mut d = Driver::new();
        d.call(
            "edit_markers",
            json!({"add": [{"bar": 1, "name": "Intro"}]}),
        )
        .unwrap();
        let m = d.project()["markers"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = d
            .call(
                "edit_lesson_notes",
                json!({"text": "# Riff\n- slow first",
                       "add_tabs": [{"section": m, "title": "Intro riff", "text": "e|-0-3-|"}]}),
            )
            .unwrap();
        let tab = r["added_tabs"][0].as_str().unwrap().to_string();
        let a = d
            .call(
                "edit_assignments",
                json!({"add": [{"from_bar": 1, "to_bar": 4, "bpm": 90, "due_days": 7}]}),
            )
            .unwrap();
        let a = a["added"][0].as_str().unwrap().to_string();
        d.call("edit_assignments", json!({"done": {"id": a, "done": true}}))
            .unwrap();
        d.call("edit_lessons", json!({"start": "Week 1"})).unwrap();
        let l = d.call("get_lessons", json!({})).unwrap();
        assert_eq!(l["notes"], "# Riff\n- slow first");
        assert_eq!(l["tabs"][0]["id"], tab.as_str());
        assert_eq!(l["tabs"][0]["section"], m.as_str());
        assert_eq!(l["assignments"][0]["from_bar"], 1);
        assert_eq!(l["assignments"][0]["to_bar"], 4);
        assert_eq!(l["assignments"][0]["done_by"].as_array().unwrap().len(), 1);
        assert_eq!(l["lessons"][0]["title"], "Week 1");

        d.call("edit_assignments", json!({"practise": a})).unwrap();
        assert_eq!(d.call("get_transport", json!({})).unwrap()["loop"], true);

        assert!(
            d.call("edit_lesson_notes", json!({"remove_tabs": ["nope"]}))
                .is_err()
        );
        assert!(
            d.call(
                "edit_assignments",
                json!({"add": [{"from_bar": 4, "to_bar": 2, "bpm": 90}]})
            )
            .is_err()
        );
        d.call("edit_lesson_notes", json!({"remove_tabs": [tab]}))
            .unwrap();
        d.command("undo");
        let l = d.call("get_lessons", json!({})).unwrap();
        assert_eq!(
            l["tabs"].as_array().unwrap().len(),
            1,
            "undo brings it back"
        );
    }
}
