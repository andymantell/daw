// SPDX-License-Identifier: GPL-3.0-or-later
//! The built-in effects' front panels (UI polish step 5): each effect
//! laid out on the panel kit as a unit in the Effects tab's rack.
//!
//! Every effect keeps the same grammar: modules left to right in the
//! order the sound goes through them, knobs with their values in units,
//! switches for small choices, and a small display drawn live where one
//! helps (an EQ's curve, a filter's response, a compressor's curve and
//! how far it is turning down, the echoes of a delay, a reverb's tail).
//! A simple effect is a half-width unit, two to a row of the rack; a big
//! one takes the full width. Every control is a setting of the effect,
//! so an assistant reaches each through `set_parameter`.

use daw_effects::{Effect, Kind, Unit};
use eframe::egui::{self, Pos2, RichText, Stroke, Ui, pos2, vec2};

use crate::fx::RoomPick;
use crate::panel_kit::{Knob, Size, Switch, module, screen, trace};
use crate::{spoken, theme};

/// One thing on a panel.
#[derive(Clone, Copy)]
enum It {
    /// A setting by its key, with a short label (its name when empty): a
    /// knob for a number, a switch for a choice or an on/off.
    P(&'static str, &'static str, Size),
    /// A live display, so wide.
    D(Display, f32),
    /// How far the level is being turned down now.
    Meter,
    /// The room, cabinet or capture file it uses.
    File,
}

/// What a display draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Display {
    /// The EQ's or filter's response across the frequencies.
    Response,
    /// The compressor's curve: level in to level out.
    Transfer,
    /// The delay's echoes, dying away.
    Echoes,
    /// The reverb's tail.
    Tail,
    /// A modulation effect's sweep.
    Sweep,
    /// The saturation's curve.
    Drive,
    /// The bit crusher's steps.
    Steps,
}

/// A module: a heading and its rows, each laid out left to right.
struct M {
    heading: &'static str,
    rows: &'static [&'static [It]],
}

/// An effect's panel: how many rack units tall, and its modules.
pub(crate) struct Face {
    pub units: u8,
    modules: &'static [M],
}

use It::{D, File, Meter, P};
use Size::{Large, Normal, Small};

macro_rules! m {
    ($heading:expr, $rows:expr $(,)?) => {
        M {
            heading: $heading,
            rows: $rows,
        }
    };
}

/// Each built-in effect's panel.
pub(crate) fn face(kind: Kind) -> Face {
    let (units, modules): (u8, &'static [M]) = match kind {
        Kind::Eq => (
            3,
            &[
                m!("Curve", &[&[D(Display::Response, 210.0)]]),
                m!(
                    "Cuts",
                    &[
                        &[P("low_cut", "Low cut", Normal)],
                        &[P("high_cut", "High cut", Normal)]
                    ],
                ),
                m!(
                    "Bass",
                    &[
                        &[P("low_gain", "Level", Normal)],
                        &[P("low_freq", "Freq", Normal)]
                    ],
                ),
                m!(
                    "Low middle",
                    &[
                        &[P("mid1_gain", "Level", Normal)],
                        &[P("mid1_freq", "Freq", Normal), P("mid1_q", "Width", Normal)],
                    ],
                ),
                m!(
                    "High middle",
                    &[
                        &[P("mid2_gain", "Level", Normal)],
                        &[P("mid2_freq", "Freq", Normal), P("mid2_q", "Width", Normal)],
                    ],
                ),
                m!(
                    "Treble",
                    &[
                        &[P("high_gain", "Level", Normal)],
                        &[P("high_freq", "Freq", Normal)]
                    ],
                ),
                m!("Output", &[&[P("output", "Level", Normal)]]),
            ],
        ),
        Kind::Filter => (
            2,
            &[
                m!(
                    "Filter",
                    &[&[P("mode", "", Small)], &[D(Display::Response, 200.0)]],
                ),
                m!(
                    "Shape",
                    &[&[
                        P("cutoff", "Cutoff", Large),
                        P("resonance", "Resonance", Normal),
                    ]],
                ),
                m!(
                    "Out",
                    &[&[P("slope", "", Small)], &[P("mix", "Mix", Small)]]
                ),
            ],
        ),
        Kind::Compressor => (
            2,
            &[
                m!("Curve", &[&[D(Display::Transfer, 84.0), Meter]]),
                m!(
                    "Level",
                    &[&[
                        P("threshold", "Threshold", Large),
                        P("ratio", "Ratio", Normal),
                        P("knee", "Knee", Normal),
                    ]],
                ),
                m!(
                    "Timing",
                    &[&[
                        P("attack", "Attack", Normal),
                        P("release", "Release", Normal)
                    ]],
                ),
                m!(
                    "Out",
                    &[&[P("makeup", "Make-up", Normal), P("mix", "Mix", Normal)]]
                ),
            ],
        ),
        Kind::Gate => (
            2,
            &[
                m!(
                    "Gate",
                    &[&[
                        Meter,
                        P("threshold", "Threshold", Large),
                        P("depth", "Depth", Normal)
                    ]],
                ),
                m!(
                    "Timing",
                    &[&[
                        P("attack", "Attack", Normal),
                        P("hold", "Hold", Normal),
                        P("release", "Release", Normal),
                    ]],
                ),
            ],
        ),
        Kind::Limiter => (
            2,
            &[m!(
                "Limiter",
                &[&[
                    Meter,
                    P("ceiling", "Ceiling", Large),
                    P("drive", "Drive", Normal),
                    P("release", "Release", Normal),
                ]],
            )],
        ),
        Kind::DeEsser => (
            2,
            &[
                m!(
                    "Listen",
                    &[&[
                        Meter,
                        P("frequency", "Frequency", Large),
                        P("threshold", "Threshold", Normal),
                        P("range", "Most down", Normal),
                    ]],
                ),
                m!(
                    "Mode",
                    &[&[P("mode", "", Small)], &[P("listen", "", Small)]]
                ),
            ],
        ),
        Kind::Multiband => (
            3,
            &[
                m!(
                    "Split",
                    &[
                        &[P("low_split", "Low", Normal)],
                        &[P("high_split", "High", Normal)],
                    ],
                ),
                m!(
                    "Low",
                    &[
                        &[
                            P("low_threshold", "Threshold", Normal),
                            P("low_ratio", "Ratio", Normal)
                        ],
                        &[P("low_gain", "Level", Normal)],
                    ],
                ),
                m!(
                    "Middle",
                    &[
                        &[
                            P("mid_threshold", "Threshold", Normal),
                            P("mid_ratio", "Ratio", Normal)
                        ],
                        &[P("mid_gain", "Level", Normal)],
                    ],
                ),
                m!(
                    "High",
                    &[
                        &[
                            P("high_threshold", "Threshold", Normal),
                            P("high_ratio", "Ratio", Normal),
                        ],
                        &[P("high_gain", "Level", Normal)],
                    ],
                ),
                m!(
                    "Timing",
                    &[
                        &[P("attack", "Attack", Normal), Meter],
                        &[P("release", "Release", Normal)],
                    ],
                ),
            ],
        ),
        Kind::Transient => (
            2,
            &[
                m!(
                    "Shape",
                    &[&[P("attack", "Attack", Large), P("sustain", "Sustain", Large)]],
                ),
                m!(
                    "Out",
                    &[&[P("speed", "Length", Normal), P("output", "Output", Normal)]],
                ),
            ],
        ),
        Kind::Saturation => (
            2,
            &[
                m!("Curve", &[&[D(Display::Drive, 84.0)]]),
                m!(
                    "Drive",
                    &[
                        &[P("stages", "", Small)],
                        &[P("drive", "Drive", Normal), P("character", "", Small)]
                    ],
                ),
                m!(
                    "Out",
                    &[&[P("tone", "Tone", Normal), P("output", "Output", Normal)]]
                ),
            ],
        ),
        Kind::Delay => (
            2,
            &[
                m!("Echoes", &[&[D(Display::Echoes, 150.0)]]),
                m!(
                    "Time",
                    &[&[P("time", "Time", Normal), P("sync", "", Small)]],
                ),
                m!(
                    "Repeats",
                    &[&[
                        P("feedback", "Repeats", Normal),
                        P("tone", "Tone", Normal),
                        P("mix", "Mix", Normal),
                    ]],
                ),
                m!("Stereo", &[&[P("ping_pong", "", Small)]]),
            ],
        ),
        Kind::Chorus => (
            2,
            &[
                m!("Sweep", &[&[D(Display::Sweep, 110.0)]]),
                m!(
                    "Movement",
                    &[&[
                        P("rate", "Speed", Normal),
                        P("depth", "Depth", Normal),
                        P("delay", "Delay", Normal),
                    ]],
                ),
                m!(
                    "Out",
                    &[
                        &[P("voices", "", Small)],
                        &[P("spread", "Spread", Small), P("mix", "Mix", Small)]
                    ],
                ),
            ],
        ),
        Kind::Flanger => (
            2,
            &[
                m!("Sweep", &[&[D(Display::Sweep, 110.0)]]),
                m!(
                    "Movement",
                    &[&[
                        P("rate", "Speed", Normal),
                        P("depth", "Depth", Normal),
                        P("delay", "Delay", Normal),
                        P("feedback", "Feedback", Normal),
                    ]],
                ),
                m!(
                    "Out",
                    &[&[P("spread", "Spread", Normal), P("mix", "Mix", Normal)]]
                ),
            ],
        ),
        Kind::Phaser => (
            2,
            &[
                m!("Sweep", &[&[D(Display::Sweep, 110.0)]]),
                m!(
                    "Movement",
                    &[
                        &[P("stages", "", Small)],
                        &[
                            P("rate", "Speed", Small),
                            P("depth", "Depth", Small),
                            P("centre", "Centre", Small),
                            P("feedback", "Feedback", Small),
                        ],
                    ],
                ),
                m!(
                    "Out",
                    &[&[P("spread", "Spread", Normal), P("mix", "Mix", Normal)]]
                ),
            ],
        ),
        Kind::Tremolo => (
            2,
            &[
                m!("Sweep", &[&[D(Display::Sweep, 110.0)]]),
                m!(
                    "Pulse",
                    &[&[P("rate", "Speed", Normal), P("depth", "Depth", Normal)]],
                ),
                m!(
                    "Shape",
                    &[&[P("shape", "", Small)], &[P("stereo", "", Small)]]
                ),
            ],
        ),
        Kind::Reverb => (
            2,
            &[
                m!("Tail", &[&[D(Display::Tail, 130.0)]]),
                m!(
                    "Room",
                    &[&[
                        P("decay", "Decay", Large),
                        P("predelay", "Pre-delay", Normal)
                    ]],
                ),
                m!(
                    "Out",
                    &[&[
                        P("tone", "Tone", Normal),
                        P("width", "Width", Normal),
                        P("mix", "Mix", Normal)
                    ]],
                ),
            ],
        ),
        Kind::Convolution => (
            2,
            &[
                m!("Room", &[&[File]]),
                m!(
                    "Out",
                    &[&[P("predelay", "Pre-delay", Normal), P("mix", "Mix", Normal)]],
                ),
            ],
        ),
        Kind::Amp => (
            2,
            &[
                m!(
                    "Amp",
                    &[&[P("voice", "", Small)], &[P("cabinet", "", Small)]]
                ),
                m!(
                    "Tone",
                    &[&[
                        P("gain", "Gain", Large),
                        P("bass", "Bass", Normal),
                        P("middle", "Middle", Normal),
                        P("treble", "Treble", Normal),
                        P("presence", "Presence", Normal),
                    ]],
                ),
                m!("Out", &[&[P("volume", "Volume", Normal)]]),
            ],
        ),
        Kind::Cabinet => (
            2,
            &[
                m!("Cabinet", &[&[File]]),
                m!(
                    "Out",
                    &[&[P("mix", "Mix", Normal), P("level", "Level", Normal)]]
                ),
            ],
        ),
        Kind::Capture => (
            2,
            &[
                m!("Capture", &[&[File], &[P("match", "", Small)]]),
                m!(
                    "Levels",
                    &[&[P("input", "Input", Normal), P("output", "Output", Normal)]],
                ),
            ],
        ),
        Kind::Vinyl => (
            2,
            &[
                m!(
                    "Noise",
                    &[&[
                        P("hiss", "Hiss", Normal),
                        P("crackle", "Crackle", Normal),
                        P("crackle-level", "Crackle lvl", Normal),
                        P("rumble", "Rumble", Normal),
                    ]],
                ),
                m!("Record", &[&[P("wear", "Wear", Large)]]),
            ],
        ),
        Kind::Tape => (
            2,
            &[
                m!(
                    "Wow and flutter",
                    &[&[
                        P("wow", "Wow", Normal),
                        P("wow-rate", "Speed", Normal),
                        P("flutter", "Flutter", Normal),
                        P("flutter-rate", "Speed", Normal),
                    ]],
                ),
                m!(
                    "Tape",
                    &[&[
                        P("saturation", "Saturation", Normal),
                        P("highs", "Highs", Normal),
                        P("hiss", "Hiss", Normal),
                    ]],
                ),
            ],
        ),
        Kind::BitCrusher => (
            2,
            &[
                m!("Steps", &[&[D(Display::Steps, 110.0)]]),
                m!(
                    "Crush",
                    &[&[
                        P("bits", "Bits", Large),
                        P("rate", "Rate", Normal),
                        P("drive", "Drive", Normal)
                    ]],
                ),
                m!(
                    "Out",
                    &[&[P("smooth", "", Small)], &[P("mix", "Mix", Small)]]
                ),
            ],
        ),
        Kind::Parallel => (
            2,
            &[
                m!(
                    "Chains",
                    &[&[P("chains", "", Small)], &[P("dry", "Dry", Small)]]
                ),
                m!(
                    "Levels",
                    &[&[
                        P("level1", "Chain 1", Normal),
                        P("level2", "Chain 2", Normal),
                        P("level3", "Chain 3", Normal),
                        P("level4", "Chain 4", Normal),
                    ]],
                ),
            ],
        ),
        Kind::Bands => (
            2,
            &[
                m!("Bands", &[&[P("bands", "", Small)]]),
                m!(
                    "Crossovers",
                    &[&[
                        P("cross1", "1", Normal),
                        P("cross2", "2", Normal),
                        P("cross3", "3", Normal),
                    ]],
                ),
                m!(
                    "Levels",
                    &[&[
                        P("level1", "Band 1", Normal),
                        P("level2", "Band 2", Normal),
                        P("level3", "Band 3", Normal),
                        P("level4", "Band 4", Normal),
                    ]],
                ),
            ],
        ),
        Kind::MidSide => (
            2,
            &[m!(
                "Levels",
                &[&[P("level1", "Mid", Normal), P("level2", "Side", Normal)]],
            )],
        ),
        Kind::LeftRight => (
            2,
            &[m!(
                "Levels",
                &[&[P("level1", "Left", Normal), P("level2", "Right", Normal)]],
            )],
        ),
        Kind::Switch => (
            2,
            &[
                // Which chain is heard is on the unit's header.
                m!("Chains", &[&[P("chains", "", Small)]]),
                m!(
                    "Levels",
                    &[&[
                        P("level1", "Chain 1", Normal),
                        P("level2", "Chain 2", Normal),
                        P("level3", "Chain 3", Normal),
                        P("level4", "Chain 4", Normal),
                    ]],
                ),
            ],
        ),
    };
    Face { units, modules }
}

/// Space between modules, and around a module's contents.
const GAP: f32 = 8.0;
const PAD: f32 = 16.0;
/// The width the room, cabinet and capture pickers take.
const FILE_WIDTH: f32 = 220.0;
/// The gain reduction meter's width.
const METER_WIDTH: f32 = 22.0;

/// The setting `key` of `kind`, by index.
fn index(kind: Kind, key: &str) -> Option<usize> {
    kind.params().iter().position(|p| p.key == key)
}

/// How wide one item draws.
fn item_width(ui: &Ui, kind: Kind, it: It) -> f32 {
    match it {
        P(key, _, size) => {
            let Some(p) = index(kind, key).map(|i| &kind.params()[i]) else {
                return 0.0;
            };
            match p.unit {
                Unit::Switch => Switch::width_of(ui, &["Off", "On"]).max(label_width(ui, p.name)),
                Unit::Choice if p.choices.len() <= SEGMENTS => {
                    Switch::width_of(ui, p.choices).max(label_width(ui, p.name))
                }
                Unit::Choice => 160.0,
                _ => size.cell(),
            }
        }
        D(_, w) => w,
        Meter => METER_WIDTH,
        File => FILE_WIDTH,
    }
}

/// How many choices show as segments; more go in a drop-down.
const SEGMENTS: usize = 5;

fn label_width(ui: &Ui, text: &str) -> f32 {
    let look = theme::look(ui.ctx());
    ui.fonts_mut(|f| {
        f.layout_no_wrap(text.into(), look.font(look.small), egui::Color32::WHITE)
            .size()
            .x
    })
}

fn module_width(ui: &Ui, kind: Kind, m: &M) -> f32 {
    let rows = (m.rows.iter())
        .map(|row| {
            let n = row.len().saturating_sub(1) as f32;
            row.iter().map(|&it| item_width(ui, kind, it)).sum::<f32>() + n * 6.0
        })
        .fold(0.0, f32::max);
    (rows + PAD).max(label_width(ui, &m.heading.to_uppercase()) + PAD)
}

/// How wide `kind`'s modules are side by side: whether it fits a
/// half-width unit.
pub(crate) fn width(ui: &Ui, kind: Kind) -> f32 {
    let f = face(kind);
    let n = f.modules.len().saturating_sub(1) as f32;
    (f.modules.iter())
        .map(|m| module_width(ui, kind, m))
        .sum::<f32>()
        + n * GAP
}

/// `b`'s modules, `height` tall, across the width given (in more
/// than one row when the unit is narrower). A room, cabinet or capture file
/// chosen, for the effects pane to load.
pub(crate) fn panel(ui: &mut Ui, id: u64, b: &Effect, height: f32) -> Option<RoomPick> {
    let kind = b.kind();
    let f = face(kind);
    let mut pick = None;
    crate::panel_kit::modules(ui, GAP, |ui| {
        for m in f.modules {
            let w = module_width(ui, kind, m);
            module(ui, m.heading, vec2(w, height), |ui| {
                for row in m.rows {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for &it in *row {
                            if let Some(p) = item(ui, id, b, it) {
                                pick = Some(p);
                            }
                        }
                    });
                }
            });
        }
    });
    if let Some(note) = crate::fx::room_note(ui, b) {
        ui.add(egui::Label::new(note).truncate());
    }
    pick
}

fn item(ui: &mut Ui, id: u64, b: &Effect, it: It) -> Option<RoomPick> {
    let kind = b.kind();
    match it {
        P(key, label, size) => {
            if let Some(i) = index(kind, key) {
                control(ui, id, b, i, label, size);
            }
            None
        }
        D(d, w) => {
            let h = (ui.available_height() - 2.0).max(40.0);
            display(ui, b, d, vec2(w, h));
            None
        }
        Meter => {
            let h = (ui.available_height() - 2.0).clamp(40.0, 120.0);
            meter(ui, b, vec2(METER_WIDTH, h));
            None
        }
        File => {
            ui.allocate_ui(vec2(FILE_WIDTH, ui.available_height()), |ui| {
                ui.vertical(|ui| match kind {
                    Kind::Convolution => crate::fx::room_picker(ui, id, b),
                    Kind::Cabinet => crate::fx::cabinet_picker(ui, id, b),
                    Kind::Capture => crate::fx::capture_picker(ui, b),
                    _ => None,
                })
                .inner
            })
            .inner
        }
    }
}

/// Setting `i` of `b`: a knob for a number, a switch for an on/off or a
/// short list, a drop-down for a long one. True if it changed.
pub(crate) fn control(ui: &mut Ui, id: u64, b: &Effect, i: usize, label: &str, size: Size) -> bool {
    let p = &b.kind().params()[i];
    let look = theme::look(ui.ctx());
    if !b.shown(p.key) {
        // A delay's time, set by the note length it follows.
        if let Some(n) = b.synced().filter(|_| p.key == "time") {
            ui.allocate_ui(vec2(size.cell(), size.height()), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(n).strong());
                    ui.label(
                        RichText::new("following the tempo")
                            .size(look.small)
                            .color(look.c().text_dim),
                    )
                    .on_hover_text("Set the note length to Off to set the time in ms");
                });
            });
        }
        return false;
    }
    let v = b.get(i);
    let salt = ("fx", id, i);
    let caption = |ui: &mut Ui| {
        ui.label(
            RichText::new(p.name)
                .size(look.small)
                .color(look.c().text_dim),
        );
    };
    let moved = match p.unit {
        Unit::Switch => {
            let mut moved = None;
            ui.vertical(|ui| {
                caption(ui);
                // ai: set_parameter
                moved = Switch::new(p.name, &["Off", "On"], usize::from(v >= 0.5))
                    .small()
                    .show(ui, salt)
                    .map(|k| k as f64);
            });
            moved
        }
        Unit::Choice if p.choices.len() <= SEGMENTS => {
            let mut moved = None;
            ui.vertical(|ui| {
                caption(ui);
                // ai: set_parameter
                moved = Switch::new(p.name, p.choices, v as usize)
                    .small()
                    .show(ui, salt)
                    .map(|k| k as f64);
            });
            moved
        }
        Unit::Choice => {
            let mut moved = None;
            ui.vertical(|ui| {
                caption(ui);
                let mut c = v as usize;
                let r = egui::ComboBox::from_id_salt(ui.scope_id().with(salt))
                    .selected_text(p.text(v))
                    .width(150.0)
                    .show_index(ui, &mut c, p.choices.len(), |k| p.choices[k])
                    .accessible_name(p.name);
                // ai: set_parameter
                if r.changed() {
                    moved = Some(c as f64);
                }
            });
            moved
        }
        _ => {
            let mut x = v;
            let first = spoken::start(ui);
            let label = if label.is_empty() { p.name } else { label };
            // ai: set_parameter
            let r = Knob::new(&mut x, p.min, p.max, p.name)
                .log(p.log)
                .stepped(p.stepped())
                .default(p.default)
                .label(label)
                .readout(p.text(v))
                .size(size)
                .show(ui, salt);
            let hear = match p.unit {
                Unit::Db => spoken::Hear::Level(x),
                _ => spoken::steady(x, p.min, p.max),
            };
            spoken::hearable(ui, first, p.text(x), hear);
            r.changed().then_some(x)
        }
    };
    if let Some(x) = moved {
        b.set(i, x);
        true
    } else {
        false
    }
}

/// How far the level is turned down now, as a bar falling from the top.
fn meter(ui: &mut Ui, b: &Effect, size: egui::Vec2) {
    let look = theme::look(ui.ctx());
    let c = look.c();
    let db = (-b.reduction_db()).max(0.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let bar = egui::Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 14.0));
    let p = ui.painter();
    p.rect_filled(bar, 2.0, c.meter_bg);
    let k = (db / 24.0).clamp(0.0, 1.0);
    if k > 0.0 {
        let lit = egui::Rect::from_min_size(bar.min, vec2(bar.width(), bar.height() * k));
        p.rect_filled(lit, 2.0, c.meter_high);
    }
    p.text(
        pos2(rect.center().x, rect.bottom()),
        egui::Align2::CENTER_BOTTOM,
        if db < 0.5 {
            "0".to_string()
        } else {
            format!("−{db:.0}")
        },
        look.numerals_font_at(look.small),
        c.text_dim,
    );
    let said = format!("Turning down {db:.1} dB");
    response
        .on_hover_text(format!("Turning the level down {db:.1} dB now"))
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &said));
    // The meter moves with the music.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));
}

/// The value of `key` of `b`, or `or` if it has none.
fn val(b: &Effect, key: &str, or: f64) -> f64 {
    index(b.kind(), key).map_or(or, |i| b.get(i))
}

/// `key`'s value from 0 to 1 across its range.
fn norm(b: &Effect, key: &str) -> f64 {
    let Some(i) = index(b.kind(), key) else {
        return 0.5;
    };
    let p = &b.kind().params()[i];
    ((b.get(i) - p.min) / (p.max - p.min)).clamp(0.0, 1.0)
}

fn display(ui: &mut Ui, b: &Effect, d: Display, size: egui::Vec2) {
    let (rect, look) = screen(ui, size, 4, 2);
    let inner = rect.shrink(3.0);
    let n = 80;
    let at = |t: f64, y: f64| -> Pos2 {
        pos2(
            inner.left() + inner.width() * t as f32,
            inner.bottom() - inner.height() * y.clamp(0.0, 1.0) as f32,
        )
    };
    let points: Vec<Pos2> = match d {
        Display::Response => {
            let (low, high) = if b.kind() == Kind::Eq {
                (-18.0, 18.0)
            } else {
                (-36.0, 18.0)
            };
            (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    let hz = 20.0 * 1000f64.powf(t);
                    let db = b.response_db(hz).unwrap_or(0.0);
                    at(t, (db - low) / (high - low))
                })
                .collect()
        }
        Display::Transfer => {
            // Level in from −60 to 0 dB across, out the same up.
            let threshold = val(b, "threshold", -18.0);
            let ratio = val(b, "ratio", 4.0).max(1.0);
            let knee = val(b, "knee", 0.0);
            let makeup = val(b, "makeup", 0.0);
            let out = |x: f64| {
                let over = x - threshold;
                let y = if 2.0 * over < -knee {
                    x
                } else if knee > 0.0 && 2.0 * over.abs() <= knee {
                    x + (1.0 / ratio - 1.0) * (over + knee / 2.0).powi(2) / (2.0 * knee)
                } else {
                    threshold + over / ratio
                };
                y + makeup
            };
            (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    let x = -60.0 + 60.0 * t;
                    at(t, (out(x) + 60.0) / 60.0)
                })
                .collect()
        }
        Display::Echoes => {
            // The first 2 s: a spike for the sound, then each echo.
            let time = val(b, "time", 375.0).max(1.0) / 1000.0;
            let feedback = val(b, "feedback", 35.0) / 100.0;
            let mix = val(b, "mix", 25.0) / 100.0;
            let span = 2.0;
            let stroke = Stroke::new(2.0, look.c().accent);
            let p = ui.painter();
            let dry = at(0.0, 1.0 - mix);
            p.line_segment(
                [pos2(dry.x + 2.0, inner.bottom()), pos2(dry.x + 2.0, dry.y)],
                stroke,
            );
            let mut level = mix;
            let mut t = time;
            while t < span && level > 0.02 {
                let top = at(t / span, level);
                p.line_segment([pos2(top.x, inner.bottom()), top], stroke);
                level *= feedback;
                t += time;
            }
            return;
        }
        Display::Tail => {
            let decay = val(b, "decay", 1.8).max(0.1);
            let predelay = val(b, "predelay", 10.0) / 1000.0;
            let span = (decay * 1.25).max(1.0);
            (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    let s = t * span - predelay;
                    let y = if s < 0.0 {
                        0.0
                    } else {
                        // −60 dB at the decay time, drawn in dB.
                        let db = -60.0 * s / decay;
                        (db + 60.0) / 60.0
                    };
                    at(t, y)
                })
                .collect()
        }
        Display::Sweep => {
            // One second across; the sweep at its speed and depth.
            let rate = val(b, "rate", 1.0);
            let depth = val(b, "depth", 50.0) / 100.0;
            let shape = if b.kind() == Kind::Tremolo {
                val(b, "shape", 0.0) as u32
            } else {
                0
            };
            (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    let phase = (t * rate.min(12.0)).fract();
                    let w = match shape {
                        1 => 1.0 - 2.0 * (phase - 0.5).abs(),
                        2 => {
                            if phase < 0.5 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        _ => 0.5 + 0.5 * (std::f64::consts::TAU * phase).sin(),
                    };
                    at(t, 0.5 + (w - 0.5) * depth)
                })
                .collect()
        }
        Display::Drive => {
            let k = 1.0 + 12.0 * norm(b, "drive");
            (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64;
                    let x = 2.0 * t - 1.0;
                    let y = (k * x).tanh() / k.tanh();
                    at(t, 0.5 + 0.5 * y)
                })
                .collect()
        }
        Display::Steps => {
            let bits = val(b, "bits", 8.0).clamp(1.0, 24.0);
            let levels = 2f64.powf(bits.min(6.0)) / 2.0;
            let hold = 1.0 - norm(b, "rate");
            let step = 1.0 / (8.0 + 120.0 * (1.0 - hold));
            (0..=n * 2)
                .map(|i| {
                    let t = i as f64 / (n * 2) as f64;
                    let held = (t / step).floor() * step;
                    let s = (std::f64::consts::TAU * held).sin();
                    let q = (s * levels).round() / levels;
                    at(t, 0.5 + 0.45 * q)
                })
                .collect()
        }
    };
    if d == Display::Steps || d == Display::Sweep || d == Display::Drive {
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(1.5, look.c().accent)));
    } else {
        trace(ui, rect, points, &look);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key a panel names is a setting of its effect, and every
    /// setting is on its panel: nothing is left out of reach.
    #[test]
    fn every_panel_shows_every_setting() {
        for kind in Kind::ALL {
            let f = face(kind);
            let mut keys: Vec<&str> = Vec::new();
            let mut files = 0;
            for m in f.modules {
                for row in m.rows {
                    for it in *row {
                        match it {
                            P(key, ..) => {
                                assert!(
                                    index(kind, key).is_some(),
                                    "{}: no setting {key}",
                                    kind.id()
                                );
                                keys.push(key);
                            }
                            File => files += 1,
                            _ => {}
                        }
                    }
                }
            }
            for (i, p) in kind.params().iter().enumerate() {
                // A room or cabinet's choice is its file picker.
                let by_file =
                    i == 0 && files > 0 && matches!(kind, Kind::Convolution | Kind::Cabinet);
                // The chain heard is picked on the header.
                let on_header = kind == Kind::Switch && p.key == "heard";
                assert!(
                    by_file || on_header || keys.contains(&p.key),
                    "{}: {} isn't on its panel",
                    kind.id(),
                    p.key
                );
            }
            assert!(
                (2..=3).contains(&f.units),
                "{}: a unit two or three high",
                kind.id()
            );
        }
    }

    #[test]
    fn a_metered_effect_shows_its_meter() {
        for kind in Kind::ALL.into_iter().filter(|k| k.has_meter()) {
            let has = (face(kind).modules.iter())
                .flat_map(|m| m.rows.iter())
                .flat_map(|r| r.iter())
                .any(|it| matches!(it, Meter));
            assert!(has, "{} has no meter", kind.id());
        }
    }
}

/// The Effects tab's rack, driven through the assistant interface and
/// clicked as a person would.
#[cfg(test)]
mod rack_tests {
    use egui_kittest::kittest::{NodeT, Queryable};
    use serde_json::{Value, json};

    use crate::agent::driver::Driver;
    use crate::workspace::Pane;

    fn param(d: &mut Driver, t: &str, name: &str) -> Value {
        let list = d.call("list_parameters", json!({"track": t})).unwrap();
        (list[0]["params"].as_array().unwrap().iter())
            .find(|p| p["param"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn effects_are_rack_units_an_assistant_sees_and_steers() {
        let mut d = Driver::new();
        let t = d
            .call("add_track", json!({"kind": "audio", "name": "Vocal"}))
            .unwrap()["track"]
            .as_str()
            .unwrap()
            .to_string();
        let mut ids = Vec::new();
        for kind in ["eq", "compressor", "delay"] {
            let fx = d
                .call("add_effect", json!({"track": t, "effect": kind}))
                .unwrap();
            ids.push(fx["effect"].as_str().unwrap().to_string());
        }
        let (eq, comp, delay) = (&ids[0], &ids[1], &ids[2]);
        d.app().effects.open_track = Some(0);
        d.app().workspace.full = Some(Pane::Effects);
        d.h.run();
        d.h.run();

        // Each effect shows in full, its controls on the panel kit; the
        // compressor's meter reads as its assistant field does.
        let fx = d.call("get_effects", json!({"track": t})).unwrap();
        for e in fx["effects"].as_array().unwrap() {
            assert_eq!(e["rack"], "shown", "{e}");
        }
        assert_eq!(fx["effects"][1]["turning_down_db"], json!(0.0), "{fx}");
        assert!(fx["effects"][0].get("turning_down_db").is_none());
        d.h.get_by_label("Show in rack");
        d.h.get_by_label("Threshold");
        d.h.get_by_label("Low middle frequency");
        d.h.get_by_label("Turning down 0.0 dB");

        // A switch on the delay sets the setting an assistant reads.
        d.h.get_by_label("Bounce left and right: On").click();
        d.h.run();
        assert_eq!(
            param(&mut d, &t, &format!("fx:{delay}:ping_pong"))["value"],
            json!(1.0)
        );

        // An assistant's change shows on the knob.
        d.call(
            "set_parameter",
            json!({"track": t, "param": format!("fx:{eq}:low_gain"), "value": 6.0}),
        )
        .unwrap();
        d.h.run();
        assert_eq!(
            param(&mut d, &t, &format!("fx:{eq}:low_gain"))["text"],
            "+6.0 dB"
        );
        let bass = d.h.get_by_label("Bass").accesskit_node().value();
        assert_eq!(bass.as_deref(), Some("+6.0 dB"));

        // Collapsed to a strip, the compressor keeps its on/off and
        // meter but not its knobs; hidden, it is left out, still on.
        d.call(
            "set_rack",
            json!({"track": t, "unit": comp, "view": "collapsed"}),
        )
        .unwrap();
        d.h.run();
        assert!(d.h.query_by_label("Threshold").is_none());
        let fx = d.call("get_effects", json!({"track": t})).unwrap();
        assert_eq!(fx["effects"][1]["rack"], "collapsed");
        d.call(
            "set_rack",
            json!({"track": t, "unit": eq, "view": "hidden"}),
        )
        .unwrap();
        d.h.run();
        assert!(d.h.query_by_label("Low middle frequency").is_none());
        d.h.get_by_label_contains("One effect is hidden from the rack");
        let fx = d.call("get_effects", json!({"track": t})).unwrap();
        assert_eq!(fx["effects"][0]["rack"], "hidden");
        assert_eq!(fx["effects"][0]["on"], true);

        // Show all brings every unit back.
        d.call(
            "set_rack",
            json!({"track": t, "unit": "all", "view": "shown"}),
        )
        .unwrap();
        d.h.run();
        d.h.get_by_label("Threshold");
        d.h.get_by_label("Low middle frequency");
    }
}
