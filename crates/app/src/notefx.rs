// SPDX-License-Identifier: GPL-3.0-or-later
//! Note effects on instrument tracks (Phase 4 plan A6): the kinds there
//! are and their settings, the list in the instrument window, what the
//! engine is given to play them, and rendering them into the notes.
//!
//! The project keeps each effect as its kind and its settings by name
//! (see [`daw_project::NoteFx`]); this module knows each kind's settings,
//! their ranges and what they mean. The table below is also what the
//! commands and anything driving the app from outside read, so a
//! setting has one name everywhere.

use crate::icons::{Icon, IconButton};
use daw_engine::notefx::{
    self, ArpMode, ChordShape, LengthMode, NoteFxKind, NoteFxSpec, RenderNote, ScaleSpec, Snap,
};
use daw_project::{NoteFx, PPQ, Scale};
use eframe::egui;

use crate::commands::Cmd;

/// How a setting is shown and changed.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Shape {
    /// One of these values, each with its name.
    Choice(&'static [(f64, &'static str)]),
    /// On (1) or off (0).
    Toggle,
    /// A whole number from `min` to `max`, in this unit.
    Number(&'static str),
}

/// One setting of a kind of note effect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Param {
    pub id: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub shape: Shape,
    pub hint: &'static str,
}

/// A kind of note effect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Kind {
    /// As the project names it.
    pub id: &'static str,
    pub name: &'static str,
    pub hint: &'static str,
    pub params: &'static [Param],
}

const T: f64 = PPQ as f64;

/// Note values, in ticks.
const RATES: &[(f64, &str)] = &[
    (T, "1/4"),
    (T / 2.0, "1/8"),
    (T / 3.0, "1/8 triplet"),
    (T / 4.0, "1/16"),
    (T / 6.0, "1/16 triplet"),
    (T / 8.0, "1/32"),
];

const LENGTHS: &[(f64, &str)] = &[
    (T / 8.0, "1/32"),
    (T / 4.0, "1/16"),
    (T / 2.0, "1/8"),
    (T, "1/4"),
    (T * 2.0, "1/2"),
    (T * 4.0, "1 bar"),
];

const fn param(
    id: &'static str,
    label: &'static str,
    min: f64,
    max: f64,
    default: f64,
    shape: Shape,
    hint: &'static str,
) -> Param {
    Param {
        id,
        label,
        min,
        max,
        default,
        shape,
        hint,
    }
}

/// The chord shapes, in the order shown: semitones above the note, or
/// thirds up the song's scale.
const SHAPES: &[(f64, &str)] = &[
    (0.0, "Major"),
    (1.0, "Minor"),
    (2.0, "Fifth (power chord)"),
    (3.0, "Octave"),
    (4.0, "Sus2"),
    (5.0, "Sus4"),
    (6.0, "Major 7"),
    (7.0, "Minor 7"),
    (8.0, "Dominant 7"),
    (9.0, "Triad in the key"),
    (10.0, "Seventh in the key"),
];

fn shape(k: usize) -> ChordShape {
    let fixed = |i: &[i8]| {
        let mut intervals = [0; 4];
        intervals[..i.len()].copy_from_slice(i);
        ChordShape::Fixed {
            intervals,
            count: i.len() as u8,
        }
    };
    match k {
        1 => fixed(&[3, 7]),
        2 => fixed(&[7]),
        3 => fixed(&[12]),
        4 => fixed(&[2, 7]),
        5 => fixed(&[5, 7]),
        6 => fixed(&[4, 7, 11]),
        7 => fixed(&[3, 7, 10]),
        8 => fixed(&[4, 7, 10]),
        9 => ChordShape::InKey { count: 2 },
        10 => ChordShape::InKey { count: 3 },
        _ => fixed(&[4, 7]),
    }
}

/// Every kind of note effect, in the order offered.
pub(crate) const KINDS: &[Kind] = &[
    Kind {
        id: "arpeggiator",
        name: "Arpeggiator",
        hint: "Plays the keys held down one after another, in time with the song",
        params: &[
            param(
                "mode",
                "Order",
                0.0,
                6.0,
                0.0,
                Shape::Choice(&[
                    (0.0, "Up"),
                    (1.0, "Down"),
                    (2.0, "Up and down"),
                    (3.0, "Down and up"),
                    (4.0, "As played"),
                    (5.0, "Random"),
                    (6.0, "Chord"),
                ]),
                "The order the held notes play in; Chord plays them all on each step",
            ),
            param(
                "rate",
                "Rate",
                T / 8.0,
                T,
                T / 4.0,
                Shape::Choice(RATES),
                "How often a step plays",
            ),
            param(
                "octaves",
                "Octaves",
                1.0,
                4.0,
                1.0,
                Shape::Number(""),
                "How many octaves up the pattern climbs before it starts again",
            ),
            param(
                "gate",
                "Gate",
                5.0,
                100.0,
                50.0,
                Shape::Number("%"),
                "How long each step sounds, as a share of the step; 100% joins them up",
            ),
            param(
                "latch",
                "Latch",
                0.0,
                1.0,
                0.0,
                Shape::Toggle,
                "Keep playing after the keys are let go, until new ones are pressed",
            ),
            param(
                "velocity",
                "Velocity",
                0.0,
                127.0,
                0.0,
                Shape::Number(""),
                "0 plays each note as hard as it was played; above, every step at that velocity",
            ),
        ],
    },
    Kind {
        id: "chord",
        name: "Chord",
        hint: "Plays a chord on each note: a fixed shape, or one from the song's key",
        params: &[param(
            "shape",
            "Chord",
            0.0,
            10.0,
            0.0,
            Shape::Choice(SHAPES),
            "The notes added above each note played",
        )],
    },
    Kind {
        id: "length",
        name: "Note length",
        hint: "Makes notes a set length, at least a length, or longer",
        params: &[
            param(
                "mode",
                "Make notes",
                0.0,
                2.0,
                0.0,
                Shape::Choice(&[
                    (0.0, "This long"),
                    (1.0, "At least this long"),
                    (2.0, "Longer by"),
                ]),
                "This long ignores when the key is let go",
            ),
            param(
                "length",
                "Length",
                T / 8.0,
                T * 4.0,
                T / 4.0,
                Shape::Choice(LENGTHS),
                "",
            ),
        ],
    },
    Kind {
        id: "velocity",
        name: "Velocity",
        hint: "Makes notes softer, harder or more even",
        params: &[
            param(
                "scale",
                "Scale",
                0.0,
                200.0,
                100.0,
                Shape::Number("%"),
                "Below 100% evens notes out; above, spreads them apart",
            ),
            param(
                "offset",
                "Add",
                -64.0,
                64.0,
                0.0,
                Shape::Number(""),
                "Added to every note's velocity",
            ),
            param("min", "Softest", 1.0, 127.0, 1.0, Shape::Number(""), ""),
            param("max", "Hardest", 1.0, 127.0, 127.0, Shape::Number(""), ""),
        ],
    },
    Kind {
        id: "randomise",
        name: "Randomise",
        hint: "Moves each note's pitch and velocity a little at random, and can leave some out",
        params: &[
            param(
                "pitch",
                "Pitch",
                0.0,
                12.0,
                0.0,
                Shape::Number(" semitones"),
                "Up to this far up or down",
            ),
            param(
                "velocity",
                "Velocity",
                0.0,
                64.0,
                10.0,
                Shape::Number(""),
                "Up to this much softer or harder",
            ),
            param(
                "chance",
                "Chance",
                0.0,
                100.0,
                100.0,
                Shape::Number("%"),
                "The chance each note plays",
            ),
            param(
                "in_scale",
                "Stay in the key",
                0.0,
                1.0,
                1.0,
                Shape::Toggle,
                "Moved notes land on the song's key and scale",
            ),
        ],
    },
    Kind {
        id: "scale",
        name: "Scale",
        hint: "Moves notes onto the song's key and scale, or leaves out the ones off it",
        params: &[param(
            "snap",
            "Notes off the scale",
            0.0,
            3.0,
            0.0,
            Shape::Choice(&[
                (0.0, "Move to the nearest"),
                (1.0, "Move up"),
                (2.0, "Move down"),
                (3.0, "Leave out"),
            ]),
            "",
        )],
    },
];

pub(crate) fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

/// A new note effect of the kind at `k` in [`KINDS`], at its defaults.
pub(crate) fn new_fx(k: usize) -> Option<NoteFx> {
    let kind = KINDS.get(k)?;
    Some(NoteFx {
        id: daw_project::new_id(),
        kind: kind.id.into(),
        bypass: false,
        params: (kind.params.iter())
            .map(|p| (p.id.to_string(), p.default))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .collect(),
    })
}

/// A setting's value, or its default when the project has none, kept in
/// its range.
pub(crate) fn get(fx: &NoteFx, id: &str) -> f64 {
    let p = kind(&fx.kind).and_then(|k| k.params.iter().find(|p| p.id == id));
    let v = fx.params.iter().find(|(k, _)| k == id).map(|(_, v)| *v);
    match p {
        Some(p) => v
            .filter(|v| v.is_finite())
            .unwrap_or(p.default)
            .clamp(p.min, p.max),
        None => v.unwrap_or(0.0),
    }
}

/// Sets a setting, kept in its range (whole numbers, or one of the
/// choices). Errs for a setting the effect doesn't have.
pub(crate) fn set(fx: &mut NoteFx, id: &str, value: f64) -> Result<(), String> {
    let k = kind(&fx.kind).ok_or_else(|| format!("unknown note effect “{}”", fx.kind))?;
    let p = (k.params.iter())
        .find(|p| p.id == id)
        .ok_or_else(|| format!("{} has no setting “{id}”", k.name))?;
    if !value.is_finite() {
        return Err(format!("{} must be a number", p.label));
    }
    let v = match p.shape {
        Shape::Choice(c) => c
            .iter()
            .map(|c| c.0)
            .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
            .unwrap_or(p.default),
        Shape::Toggle => (value >= 0.5) as u8 as f64,
        Shape::Number(_) => value.round().clamp(p.min, p.max),
    };
    match fx.params.iter_mut().find(|(k, _)| k == id) {
        Some(e) => e.1 = v,
        None => {
            fx.params.push((id.to_string(), v));
            fx.params.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
    Ok(())
}

/// How the assistant tools name a note effect's setting:
/// `notefx:` and the effect's id, `:` and the setting.
pub(crate) const PARAM_PREFIX: &str = "notefx:";

/// The settings of a track's note effects, as `list_parameters` gives
/// them: each with its name, value, range and, for a choice, the values
/// it can take.
pub(crate) fn parameters(list: &[NoteFx]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for fx in list {
        let Some(k) = kind(&fx.kind) else { continue };
        let id = format!("{PARAM_PREFIX}{}", fx.id);
        out.push(serde_json::json!({
            "param": format!("{id}:on"),
            "name": format!("{}: on", k.name),
            "value": (!fx.bypass) as u8,
            "min": 0, "max": 1,
        }));
        for p in k.params {
            let v = get(fx, p.id);
            let mut row = serde_json::json!({
                "param": format!("{id}:{}", p.id),
                "name": format!("{}: {}", k.name, p.label),
                "value": v,
                "text": value_text(p, v),
                "min": p.min,
                "max": p.max,
            });
            if let Shape::Choice(c) = p.shape {
                row["choices"] = (c.iter())
                    .map(|(v, name)| serde_json::json!({"value": v, "name": name}))
                    .collect();
            }
            out.push(row);
        }
    }
    out
}

/// Sets a note effect's setting by its `list_parameters` name. The undo
/// step's name, or why it can't.
pub(crate) fn set_parameter(
    track: &str,
    list: &mut [NoteFx],
    param: &str,
    value: f64,
) -> Result<String, String> {
    let rest = param.strip_prefix(PARAM_PREFIX).unwrap_or(param);
    let (id, name) = rest
        .rsplit_once(':')
        .ok_or_else(|| format!("{track} has no setting {param}."))?;
    let fx = (list.iter_mut())
        .find(|f| f.id == id)
        .ok_or_else(|| format!("{track} has no note effect {id}."))?;
    let k = kind(&fx.kind).ok_or_else(|| format!("{} isn't known here.", fx.kind))?;
    if name == "on" {
        fx.bypass = value < 0.5;
        let state = if fx.bypass { "off" } else { "on" };
        return Ok(format!("{} on {track} {state}", k.name));
    }
    set(fx, name, value)?;
    let p = k
        .params
        .iter()
        .find(|p| p.id == name)
        .expect("set checked it");
    Ok(format!(
        "Set {track} {} {} to {}",
        k.name,
        p.label,
        value_text(p, get(fx, name))
    ))
}

/// The key the effects follow: the song's, or C major when it has none.
pub(crate) fn scale_spec(scale: Option<Scale>) -> ScaleSpec {
    let s = scale.unwrap_or(Scale::new(0, daw_project::ScaleKind::Major));
    ScaleSpec::new(s.root, s.kind.steps())
}

/// An effect id the engine can match from one graph to the next.
fn engine_id(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What the engine plays for one effect; `None` for a kind this version
/// doesn't know.
pub(crate) fn spec(fx: &NoteFx) -> Option<NoteFxSpec> {
    let g = |id| get(fx, id);
    let kind = match fx.kind.as_str() {
        "arpeggiator" => NoteFxKind::Arpeggiator {
            mode: match g("mode") as u8 {
                1 => ArpMode::Down,
                2 => ArpMode::UpDown,
                3 => ArpMode::DownUp,
                4 => ArpMode::AsPlayed,
                5 => ArpMode::Random,
                6 => ArpMode::Chord,
                _ => ArpMode::Up,
            },
            rate: g("rate"),
            octaves: g("octaves") as u8,
            gate: (g("gate") / 100.0) as f32,
            latch: g("latch") >= 0.5,
            velocity: g("velocity") as u8,
        },
        "chord" => NoteFxKind::Chord {
            shape: shape(g("shape") as usize),
        },
        "length" => NoteFxKind::Length {
            mode: match g("mode") as u8 {
                1 => LengthMode::AtLeast,
                2 => LengthMode::LongerBy,
                _ => LengthMode::Fixed,
            },
            ticks: g("length"),
        },
        "velocity" => NoteFxKind::Velocity {
            scale: (g("scale") / 100.0) as f32,
            offset: g("offset") as i32,
            min: g("min") as u8,
            max: g("max") as u8,
        },
        "randomise" => NoteFxKind::Randomise {
            pitch: g("pitch") as u8,
            velocity: g("velocity") as u8,
            chance: (g("chance") / 100.0) as f32,
            in_scale: g("in_scale") >= 0.5,
        },
        "scale" => NoteFxKind::Scale {
            snap: match g("snap") as u8 {
                1 => Snap::Up,
                2 => Snap::Down,
                3 => Snap::Leave,
                _ => Snap::Nearest,
            },
        },
        _ => return None,
    };
    Some(NoteFxSpec {
        id: engine_id(&fx.id),
        bypass: fx.bypass,
        kind,
    })
}

/// What the engine plays for a track's effects.
pub(crate) fn specs(list: &[NoteFx]) -> Vec<NoteFxSpec> {
    list.iter().filter_map(spec).collect()
}

/// How far past a clip's end notes it started may still sound when
/// rendered: a long note length's worth.
const TAIL: i64 = PPQ * 8;

/// The notes of `clip` through the effects, as the clip's own notes:
/// plain notes, in step with the timeline. Notes outside the clip are
/// dropped, as they would not play.
pub(crate) fn render_clip(
    list: &[NoteFx],
    scale: Option<Scale>,
    clip: &daw_project::NoteClip,
) -> Vec<daw_project::Note> {
    let origin = clip.start - clip.offset;
    let notes: Vec<RenderNote> = (clip.sounding().into_iter())
        .map(|(_, n)| RenderNote {
            start: n.start,
            len: n.len,
            pitch: n.pitch,
            velocity: n.velocity,
            channel: n.channel,
        })
        .collect();
    let end = clip.start + clip.len;
    let made = notefx::render(
        &specs(list),
        scale_spec(scale),
        &notes,
        clip.start,
        end + TAIL,
    );
    (made.into_iter())
        .filter(|n| n.start < end)
        .map(|n| {
            let mut note = daw_project::Note::new(n.start - origin, n.len, n.pitch, n.velocity);
            note.channel = n.channel;
            note
        })
        .collect()
}

/// What a value is called, for a choice, or as a number in its unit.
pub(crate) fn value_text(p: &Param, v: f64) -> String {
    match p.shape {
        Shape::Choice(c) => (c.iter())
            .find(|c| (c.0 - v).abs() < 1e-6)
            .map_or_else(|| v.to_string(), |c| c.1.to_string()),
        Shape::Toggle => if v >= 0.5 { "On" } else { "Off" }.into(),
        Shape::Number(unit) => format!("{}{unit}", v.round()),
    }
}

/// What changed in the note effects section this frame.
#[derive(Default)]
pub(crate) struct Edited {
    pub changed: bool,
    pub cmd: Option<Cmd>,
}

/// The track's note effects, in the instrument window: each with its
/// settings, switched on or off, moved or removed, and a menu to add
/// more.
pub(crate) fn controls(
    ui: &mut egui::Ui,
    key: &str,
    list: &mut Vec<NoteFx>,
    scale: Option<Scale>,
) -> Edited {
    let mut out = Edited::default();
    ui.horizontal(|ui| {
        ui.strong("Note effects");
        ui.menu_button("Add…", |ui| {
            for (k, kind) in KINDS.iter().enumerate() {
                if ui.button(kind.name).on_hover_text(kind.hint).clicked() {
                    out.cmd = Some(Cmd::AddNoteFx(k as u8));
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Effects on the notes before they reach the instrument");
        if !list.is_empty()
            && ui
                .button("Render into notes")
                .on_hover_text(
                    "Make the track's note clips the notes the effects play, and switch the \
                     effects off so they don't play twice",
                )
                .clicked()
        {
            out.cmd = Some(Cmd::RenderNoteFx);
        }
    });
    if list.is_empty() {
        ui.label(
            egui::RichText::new("None: notes go straight to the instrument")
                .weak()
                .small(),
        );
        return out;
    }
    let uses_key = list
        .iter()
        .any(|f| f.kind == "scale" || f.kind == "chord" || f.kind == "randomise");
    if uses_key {
        let text = match scale {
            Some(s) => format!("Key: {}", s.label()),
            None => "Key: C major (the song has no key set; set one in the note editor)".into(),
        };
        ui.label(egui::RichText::new(text).weak().small());
    }
    let mut remove = None;
    let mut swap = None;
    let n = list.len();
    for (i, fx) in list.iter_mut().enumerate() {
        let Some(kind) = kind(&fx.kind) else {
            ui.label(format!("{} (not in this version: off)", fx.kind));
            continue;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut on = !fx.bypass;
                if ui
                    .checkbox(&mut on, kind.name)
                    .on_hover_text(kind.hint)
                    .changed()
                {
                    fx.bypass = !on;
                    out.changed = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(IconButton::new(Icon::Close, "Remove").small())
                        .clicked()
                    {
                        remove = Some(i);
                    }
                    if i + 1 < n
                        && ui
                            .add(IconButton::new(Icon::MoveDown, "Move down").small())
                            .clicked()
                    {
                        swap = Some(i);
                    }
                    if i > 0
                        && ui
                            .add(IconButton::new(Icon::MoveUp, "Move up").small())
                            .clicked()
                    {
                        swap = Some(i - 1);
                    }
                });
            });
            ui.add_enabled_ui(!fx.bypass, |ui| {
                egui::Grid::new(("note-fx", key, &fx.id))
                    .num_columns(2)
                    .show(ui, |ui| {
                        for p in kind.params {
                            let r = ui.label(p.label);
                            if !p.hint.is_empty() {
                                r.on_hover_text(p.hint);
                            }
                            let now = get(fx, p.id);
                            let mut v = now;
                            match p.shape {
                                Shape::Choice(c) => {
                                    egui::ComboBox::from_id_salt(("note-fx", &fx.id, p.id))
                                        .selected_text(value_text(p, now))
                                        .show_ui(ui, |ui| {
                                            for &(cv, name) in c {
                                                ui.selectable_value(&mut v, cv, name);
                                            }
                                        })
                                        .response
                                        .accessible_name(p.label);
                                }
                                Shape::Toggle => {
                                    let mut b = now >= 0.5;
                                    ui.checkbox(&mut b, "").accessible_name(p.label);
                                    v = b as u8 as f64;
                                }
                                Shape::Number(unit) => {
                                    ui.add(
                                        egui::Slider::new(&mut v, p.min..=p.max)
                                            .step_by(1.0)
                                            .fixed_decimals(0)
                                            .suffix(unit),
                                    )
                                    .accessible_name(p.label);
                                }
                            }
                            if v != now && set(fx, p.id, v).is_ok() {
                                out.changed = true;
                            }
                            ui.end_row();
                        }
                    });
            });
        });
    }
    if let Some(i) = remove {
        list.remove(i);
        out.changed = true;
    }
    if let Some(i) = swap {
        list.swap(i, i + 1);
        out.changed = true;
    }
    out
}

impl crate::App {
    /// The instrument track note effect commands act on: the one whose
    /// instrument window is open, else the selected note clip's, else
    /// the first.
    pub(crate) fn note_fx_track(&self) -> Option<usize> {
        self.layer_track()
    }

    /// Adds a note effect of the kind at `k` in [`KINDS`] to the end of
    /// the instrument track's list, and opens its window.
    pub(crate) fn add_note_fx(&mut self, k: usize) -> Result<(), String> {
        let i = self
            .note_fx_track()
            .ok_or("Add an instrument track first.")?;
        let fx = new_fx(k).ok_or("No such note effect.")?;
        self.tracks[i].note_fx.push(fx);
        self.instrument_open = Some(i);
        self.rebuild();
        Ok(())
    }

    /// Switches the instrument track's note effects all off, or all on
    /// again if they are all off.
    pub(crate) fn toggle_note_fx(&mut self) {
        let Some(i) = self.note_fx_track() else {
            return;
        };
        let list = &mut self.tracks[i].note_fx;
        let off = list.iter().any(|f| !f.bypass);
        for f in list.iter_mut() {
            f.bypass = off;
        }
        self.rebuild();
    }

    /// Takes the note effects off the instrument track.
    pub(crate) fn clear_note_fx(&mut self) {
        if let Some(i) = self.note_fx_track() {
            self.tracks[i].note_fx.clear();
            self.rebuild();
        }
    }

    /// Makes the instrument track's note clips play the notes its note
    /// effects make, as plain notes, and switches the effects off. Pattern
    /// clips become plain note clips first.
    pub(crate) fn render_note_fx(&mut self) -> Result<usize, String> {
        let i = self
            .note_fx_track()
            .ok_or("Add an instrument track first.")?;
        if !self.tracks[i]
            .note_fx
            .iter()
            .any(|f| !f.bypass && spec(f).is_some())
        {
            return Err("The track has no note effects switched on.".into());
        }
        self.fill_patterns();
        let scale = self.scale;
        let t = &mut self.tracks[i];
        let list = t.note_fx.clone();
        let mut made = 0;
        for c in t.note_clips.iter_mut().filter(|c| !c.muted) {
            c.pattern = None;
            c.notes = render_clip(&list, scale, c);
            // The rendered notes run the clip's whole length.
            c.loop_len = None;
            made += c.notes.len();
        }
        for f in &mut t.note_fx {
            f.bypass = true;
        }
        self.rebuild();
        Ok(made)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_makes_an_engine_effect_at_its_defaults() {
        for (k, kind) in KINDS.iter().enumerate() {
            let fx = new_fx(k).unwrap();
            assert_eq!(fx.kind, kind.id);
            assert!(spec(&fx).is_some(), "{}", kind.id);
            for p in kind.params {
                assert_eq!(get(&fx, p.id), p.default, "{}.{}", kind.id, p.id);
                assert!(p.min <= p.default && p.default <= p.max);
            }
        }
        let unknown = NoteFx {
            kind: "from-the-future".into(),
            ..new_fx(0).unwrap()
        };
        assert!(spec(&unknown).is_none());
    }

    #[test]
    fn settings_stay_in_range_and_on_their_choices() {
        let mut arp = new_fx(0).unwrap();
        set(&mut arp, "rate", 250.0).unwrap();
        assert_eq!(get(&arp, "rate"), 240.0, "the nearest note value");
        set(&mut arp, "octaves", 9.0).unwrap();
        assert_eq!(get(&arp, "octaves"), 4.0);
        set(&mut arp, "latch", 0.9).unwrap();
        assert_eq!(get(&arp, "latch"), 1.0);
        assert!(set(&mut arp, "nothing", 1.0).is_err());
        assert!(set(&mut arp, "gate", f64::NAN).is_err());
        // A value from a damaged project is kept in range when read.
        arp.params.retain(|p| p.0 != "gate");
        arp.params.push(("gate".into(), 1000.0));
        assert_eq!(get(&arp, "gate"), 100.0);
    }

    #[test]
    fn rendering_keeps_the_notes_the_effects_play() {
        // An arpeggiator over a held C major chord, in a clip starting a
        // beat in, with its content offset by a beat too.
        let mut clip = daw_project::NoteClip::new("c".into(), "Keys".into(), PPQ, PPQ * 2);
        clip.offset = PPQ;
        for p in [60, 64, 67] {
            clip.notes.push(daw_project::Note::new(PPQ, PPQ, p, 100));
        }
        // Before the clip's content start: not played, not kept.
        clip.notes.push(daw_project::Note::new(0, PPQ, 72, 100));
        let arp = new_fx(0).unwrap();
        let notes = render_clip(&[arp], None, &clip);
        let got: Vec<(i64, u8)> = notes.iter().map(|n| (n.start, n.pitch)).collect();
        let q = PPQ / 4;
        assert_eq!(
            got,
            [
                (PPQ, 60),
                (PPQ + q, 64),
                (PPQ + 2 * q, 67),
                (PPQ + 3 * q, 60)
            ]
        );
        assert!(notes.iter().all(|n| n.len == q / 2));
        // A chord in the key: A minor's notes from C major's A.
        let mut chord = new_fx(1).unwrap();
        set(&mut chord, "shape", 9.0).unwrap();
        let mut one = daw_project::NoteClip::new("d".into(), "Keys".into(), 0, PPQ);
        one.notes.push(daw_project::Note::new(0, PPQ, 69, 100));
        let p: Vec<u8> = render_clip(&[chord], None, &one)
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(p, [69, 72, 76]);
    }

    #[test]
    fn an_assistant_adds_sets_and_renders_note_effects() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        d.command("add-instrument-track");
        let mut clip = daw_project::NoteClip::new("c".into(), "Keys".into(), 0, 1920);
        for p in [60, 64, 67] {
            clip.notes.push(daw_project::Note::new(0, 960, p, 100));
        }
        d.app().tracks[0].note_clips.push(clip);
        d.command("add-arpeggiator");
        let p = d.project();
        let track = p["tracks"][0]["id"].as_str().unwrap().to_string();
        let fx = &p["tracks"][0]["note_effects"][0];
        assert_eq!(fx["kind"], "arpeggiator");
        let id = fx["id"].as_str().unwrap().to_string();
        // Its settings are listed, with their choices, and set by name.
        let list = d.call("list_parameters", json!({"track": track})).unwrap();
        let rate = format!("notefx:{id}:rate");
        let row = (list[0]["params"].as_array().unwrap().iter())
            .find(|r| r["param"] == rate.as_str())
            .unwrap()
            .clone();
        assert_eq!(row["text"], "1/16");
        assert!(row["choices"].as_array().unwrap().len() >= 6);
        d.call(
            "set_parameter",
            json!({"track": track, "param": rate, "value": 480.0}),
        )
        .unwrap();
        d.call(
            "set_parameter",
            json!({"track": track, "param": format!("notefx:{id}:mode"), "value": 1.0}),
        )
        .unwrap();
        assert!(
            d.call(
                "set_parameter",
                json!({"track": track, "param": format!("notefx:{id}:nothing"), "value": 1.0}),
            )
            .is_err()
        );
        // Rendered: eighths down the chord for the beat it's held.
        d.command("render-note-effects");
        let p = d.call("get_project", json!({"notes": true})).unwrap();
        let notes = p["tracks"][0]["note_clips"][0]["notes"].as_array().unwrap();
        let pitches: Vec<u64> = notes.iter().map(|n| n["pitch"].as_u64().unwrap()).collect();
        assert_eq!(pitches, [67, 64]);
        assert_eq!(p["tracks"][0]["note_effects"][0]["bypass"], true);
    }

    mod app {
        use daw_audio_io::SilentInput;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        use crate::commands::Cmd;
        use crate::{App, Audio};

        fn harness() -> Harness<'static, App> {
            let mut h = Harness::builder()
                .with_size([1400.0, 1000.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            let app = h.state_mut();
            let eng = app.replace_engine();
            app.audio = Audio::Silent(
                daw_audio_io::start_silent_with(app.rate, 256, eng, SilentInput::None),
                String::new(),
            );
            h.run();
            h
        }

        #[test]
        fn note_effects_are_added_shown_switched_saved_and_rendered_by_command() {
            let mut h = harness();
            let ctx = h.ctx.clone();
            assert!(
                !h.state().can_run(Cmd::AddNoteFx(0)),
                "needs an instrument track"
            );
            h.state_mut().run(&ctx, Cmd::AddInstrumentTrack);
            let mut clip = daw_project::NoteClip::new("c".into(), "Keys".into(), 0, 3840);
            for p in [60, 64, 67] {
                clip.notes.push(daw_project::Note::new(0, 1920, p, 100));
            }
            h.state_mut().tracks[0].note_clips.push(clip);
            assert!(
                !h.state().can_run(Cmd::RenderNoteFx),
                "nothing to render yet"
            );
            h.state_mut().run(&ctx, Cmd::AddNoteFx(0));
            h.state_mut().run(&ctx, Cmd::AddNoteFx(3));
            h.run();
            let app = h.state();
            let kinds: Vec<&str> = app.tracks[0]
                .note_fx
                .iter()
                .map(|f| f.kind.as_str())
                .collect();
            assert_eq!(kinds, ["arpeggiator", "velocity"]);
            assert_eq!(app.instrument_open, Some(0), "the window shows them");
            // In the window, with their settings.
            h.get_by_label("Arpeggiator");
            assert!(h.query_all_by_label("Octaves").next().is_some());
            // Saved with the project.
            let saved = h.state().project();
            assert_eq!(saved.tracks[0].note_fx, h.state().tracks[0].note_fx);
            // Off and on again.
            h.state_mut().run(&ctx, Cmd::NoteFxOnOff);
            assert!(h.state().tracks[0].note_fx.iter().all(|f| f.bypass));
            h.state_mut().run(&ctx, Cmd::NoteFxOnOff);
            assert!(h.state().tracks[0].note_fx.iter().all(|f| !f.bypass));
            // Rendered: the held chord becomes eight sixteenth steps over
            // the two beats it is held, and the effects go off.
            h.state_mut().run(&ctx, Cmd::RenderNoteFx);
            let t = &h.state().tracks[0];
            let notes = &t.note_clips[0].notes;
            assert_eq!(notes.len(), 8);
            let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
            assert_eq!(pitches, [60, 64, 67, 60, 64, 67, 60, 64]);
            assert!(t.note_fx.iter().all(|f| f.bypass));
            h.state_mut().run(&ctx, Cmd::ClearNoteFx);
            assert!(h.state().tracks[0].note_fx.is_empty());
        }
    }
}
