// SPDX-License-Identifier: GPL-3.0-or-later
//! The top bar (UI polish plan, step two): one row in place of the menu
//! bar, the status line and the four transport rows.
//!
//! From the left: the menus; the transport (back to the start, play,
//! record, loop and the metronome, each of the last three with an arrow
//! for its options); the position; and the song's tempo, time signature
//! and key. On the right: Find a command, one status
//! pill for the audio device and background work, and the assistants.
//!
//! What was always on screen before and is used once a session or never
//! (punch bars, pre-roll and post-roll, what to keep, the click's sound
//! and level, the loop's bars, transpose and the speed trainer) is in
//! small option panels that open from the button they belong to. The
//! practice speed and pitch have no button: Practice → Speed and pitch…
//! opens them under the tempo until the Practice panel is built. Each
//! panel can be opened by a command too, and `get_selection` says which
//! is open (`top_bar`), so an assistant can show one to the person.

use daw_project::{ROOTS, Scale, ScaleKind};
use eframe::egui::{self, RichText};

use crate::commands::Cmd;
use crate::icons::{Icon, IconButton};
use crate::{App, Audio};

/// The option panels that open from the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Options {
    Record,
    Metronome,
    Loop,
    /// The time signature, and the way to tempo changes.
    Signature,
    Key,
    /// Practice speed and pitch, and the speed trainer.
    Speed,
    /// Everything the status pill has to say.
    Status,
}

impl Options {
    #[cfg(test)]
    pub(crate) const ALL: [Options; 7] = [
        Options::Record,
        Options::Metronome,
        Options::Loop,
        Options::Signature,
        Options::Key,
        Options::Speed,
        Options::Status,
    ];

    /// Its name for assistants, in `get_selection`.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Options::Record => "record",
            Options::Metronome => "metronome",
            Options::Loop => "loop",
            Options::Signature => "time-signature",
            Options::Key => "key",
            Options::Speed => "speed",
            Options::Status => "status",
        }
    }

    /// The panel's heading, which a screen reader says too.
    fn title(self) -> &'static str {
        match self {
            Options::Record => "Record options",
            Options::Metronome => "Metronome options",
            Options::Loop => "Loop options",
            Options::Signature => "Time signature",
            Options::Key => "Key",
            Options::Speed => "Practice speed and pitch",
            Options::Status => "Status",
        }
    }
}

/// Which option panel is open, if any: one at a time.
#[derive(Default)]
pub(crate) struct TopBar {
    pub(crate) open: Option<Options>,
}

impl TopBar {
    pub(crate) fn toggle(&mut self, o: Options) {
        self.open = if self.open == Some(o) { None } else { Some(o) };
    }
}

/// How much a status line matters: problems first, then work going on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Problem,
    Busy,
    Fine,
}

/// The status pill's width, kept the same whatever it says so the
/// buttons beside it stay put.
const PILL_WIDTH: f32 = 230.0;

impl App {
    /// The top bar, one row.
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        self.practice_tick();
        let ctx = ui.ctx().clone();
        let simple = self.simple.on;
        // Bigger controls in the simpler mode.
        let tall = if simple {
            egui::vec2(0.0, ui.spacing().interact_size.y * 1.6)
        } else {
            egui::Vec2::ZERO
        };
        // The menu bar takes the whole row; what follows the menus is
        // drawn in the app's own style rather than the menus'.
        let style = ui.style().clone();
        let width = ui.available_width();
        egui::MenuBar::new().ui(ui, |ui| {
            self.menu_bar(ui);
            self.simple_badge(ui);
            ui.set_style(style);
            ui.separator();
            self.transport_buttons(ui, &ctx, tall);
            ui.separator();
            self.clock(ui);
            self.song_buttons(ui, tall);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.offered(Cmd::Assistants)
                    && ui
                        // ai: assistants
                        .add(
                            IconButton::new(Icon::Assistant, "AI assistants")
                                .tip(self.commands.hover(Cmd::Assistants)),
                        )
                        .clicked()
                {
                    self.run(&ctx, Cmd::Assistants);
                }
                self.status_pill(ui);
                let keys = self.commands.keymap.text(Cmd::FindCommand);
                // From the bar's whole width, which is the same every
                // frame: what is left over moves as the clock counts, and
                // the button would flip between its two forms.
                let narrow = width < 1600.0;
                let find = if narrow {
                    // ai: find-command
                    IconButton::new(Icon::Search, "Find a command")
                } else {
                    // ai: find-command
                    IconButton::labelled(
                        Icon::Search,
                        RichText::new(format!("Find a command   {keys}")),
                    )
                    .spoken("Find a command")
                };
                if ui
                    // ai: find-command
                    .add(find.tip(self.commands.hover(Cmd::FindCommand)))
                    .clicked()
                {
                    self.run(&ctx, Cmd::FindCommand);
                }
            });
        });
    }

    /// Back to the start, play, record, loop and the metronome.
    fn transport_buttons(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, tall: egui::Vec2) {
        let simple = self.simple.on;
        if ui
            // ai: to-start
            .add(
                IconButton::new(Icon::ToStart, "Back to the start")
                    .tip(self.commands.hover(Cmd::ToStart))
                    .min_size(tall),
            )
            .clicked()
        {
            self.run(ctx, Cmd::ToStart);
        }
        let playing = self.ctl.status().playing();
        // As wide playing or not, so the controls after it stay put.
        let (icon, word) = if playing {
            (Icon::Stop, "Stop")
        } else {
            (Icon::Play, "Play")
        };
        // ai: play
        let play = IconButton::labelled(icon, word).min_size(egui::vec2(72.0, tall.y));
        if ui
            // ai: play
            .add(play)
            .on_hover_text(self.commands.hover(Cmd::PlayStop))
            .clicked()
        {
            self.run(ctx, Cmd::PlayStop);
        }
        // The dot is always the theme's record colour; the word turns
        // that colour too while recording.
        let record = crate::theme::look(ui.ctx()).c().record;
        let rec = RichText::new("Record").color(if self.recording {
            record
        } else {
            ui.visuals().text_color()
        });
        if ui
            // ai: record
            .add(
                IconButton::labelled(Icon::Record, rec)
                    .selected(self.recording)
                    .frame(self.recording)
                    .colour(record)
                    .min_size(tall),
            )
            .on_hover_text(self.commands.hover(Cmd::Record))
            .clicked()
        {
            self.run(ctx, Cmd::Record);
        }
        self.options_arrow(ui, Options::Record, Cmd::RecordOptions, tall);

        let loop_button = if simple {
            // ai: loop
            IconButton::labelled(Icon::Loop, "Loop").spoken("Loop")
        } else {
            // ai: loop
            IconButton::new(Icon::Loop, "Loop")
        };
        if ui
            // ai: loop
            .add(
                loop_button
                    .selected(self.looping)
                    .tip(self.commands.hover(Cmd::Loop))
                    .min_size(tall),
            )
            .clicked()
        {
            self.run(ctx, Cmd::Loop);
        }
        self.options_arrow(ui, Options::Loop, Cmd::LoopOptions, tall);

        let click = if simple {
            // ai: metronome
            IconButton::labelled(Icon::Metronome, "Click").spoken("Metronome")
        } else {
            // ai: metronome
            IconButton::new(Icon::Metronome, "Metronome")
        };
        if ui
            // ai: metronome
            .add(
                click
                    .selected(self.metronome.enabled)
                    .tip(self.commands.hover(Cmd::Metronome))
                    .min_size(tall),
            )
            .clicked()
        {
            self.run(ctx, Cmd::Metronome);
        }
        self.options_arrow(ui, Options::Metronome, Cmd::MetronomeOptions, tall);
    }

    /// The small arrow after a button that opens its options, and the
    /// panel itself while it is open.
    fn options_arrow(&mut self, ui: &mut egui::Ui, which: Options, cmd: Cmd, tall: egui::Vec2) {
        let r = ui
            // ai: view
            .add(
                IconButton::new(Icon::Options, which.title())
                    .tip(self.commands.hover(cmd))
                    .selected(self.top.open == Some(which))
                    .small()
                    .min_size(egui::vec2(0.0, tall.y)),
            );
        self.options_panel(&r, which);
    }

    /// Opens or closes `which` when `anchor` is clicked, and shows it
    /// below `anchor` while it is open. Clicking elsewhere or Escape
    /// closes it.
    fn options_panel(&mut self, anchor: &egui::Response, which: Options) {
        if anchor.clicked() {
            self.top.toggle(which);
        }
        self.show_panel(anchor, which);
    }

    /// Shows `which` below `anchor` while it is open.
    fn show_panel(&mut self, anchor: &egui::Response, which: Options) {
        if self.top.open != Some(which) {
            return;
        }
        let mut open = true;
        egui::Popup::from_response(anchor)
            .id(egui::Id::unique(("top bar options", which.id())))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .gap(4.0)
            .accessible_name(which.title())
            .show(|ui| {
                ui.set_min_width(240.0);
                ui.label(RichText::new(which.title()).strong());
                ui.add_space(ui.spacing().item_spacing.y);
                match which {
                    Options::Record => self.record_options(ui),
                    Options::Metronome => self.metronome_options(ui),
                    Options::Loop => self.loop_options(ui),
                    Options::Signature => self.signature_options(ui),
                    Options::Key => self.key_options(ui),
                    Options::Speed => self.speed_options(ui),
                    Options::Status => self.status_details(ui),
                }
            });
        if !open && self.top.open == Some(which) {
            self.top.open = None;
        }
    }

    /// The position, in bars and in time.
    fn clock(&mut self, ui: &mut egui::Ui) {
        let map = self.tempo_map();
        let pos = self.ctl.status().position();
        let bb = map.bar_beat(map.ticks_at(pos));
        let font = crate::theme::look(ui.ctx()).numerals_font();
        let bars = if self.ctl.status().counting_in() {
            "count-in".to_string()
        } else {
            bb.to_string()
        };
        let secs = map.seconds(pos);
        let time = match self.shown_clock() {
            Some(c) => c.timecode(pos.0, self.rate).to_string(),
            None => format!("{}:{:05.2}", (secs / 60.0) as u64, secs % 60.0),
        };
        let spoken = format!("Position: bar {bars}, {time}");
        let frame = egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(ui.visuals().widgets.inactive.corner_radius)
            .inner_margin(egui::Margin::symmetric(8, 2));
        let r = frame
            .show(ui, |ui| {
                // Room for the clock as it counts, so the controls after
                // it stay put.
                let bar = ui.add_sized(
                    [font.size * 5.5, ui.spacing().interact_size.y],
                    egui::Label::new(RichText::new(bars).font(font.clone())),
                );
                let width = if self.shown_clock().is_some() {
                    8.5
                } else {
                    5.0
                };
                ui.add_sized(
                    [font.size * width, ui.spacing().interact_size.y],
                    egui::Label::new(RichText::new(time).font(font.clone()).weak()),
                );
                bar
            })
            .response;
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &spoken));
    }

    /// The tempo, time signature, key and practice speed.
    fn song_buttons(&mut self, ui: &mut egui::Ui, tall: egui::Vec2) {
        let simple = self.simple.on;
        let t = ui
            .add_sized(
                [72.0, tall.y.max(ui.spacing().interact_size.y)],
                // ai: set_tempo
                egui::DragValue::new(&mut self.tempo_bpm)
                    .range(20.0..=300.0)
                    .speed(0.2)
                    .max_decimals(1)
                    .suffix(" BPM"),
            )
            .accessible_name("Tempo")
            .on_hover_text("The song's tempo: drag, or double-click to type");
        if t.changed() {
            self.time_changed();
        }
        if !simple {
            let sig = format!("{}/{}", self.beats_per_bar, self.time.beat_unit);
            let r = ui
                // ai: view
                .add(egui::Button::selectable(
                    self.top.open == Some(Options::Signature),
                    &sig,
                ))
                .on_hover_text("Time signature, and tempo changes later in the song")
                .accessible_name(format!("Time signature, {sig}"));
            self.options_panel(&r, Options::Signature);
            let key = match self.scale {
                Some(s) => s.label(),
                None => "No key".into(),
            };
            let r = ui
                // ai: view
                .add(egui::Button::selectable(
                    self.top.open == Some(Options::Key),
                    &key,
                ))
                .on_hover_text("The song's key: the note editor and fretboard show it")
                .accessible_name(format!("Key, {key}"));
            self.options_panel(&r, Options::Key);
        }
        // Speed and pitch have no button of their own (Practice → Speed
        // and pitch… opens them, until the Practice panel is built); they
        // show under the tempo.
        if self.top.open == Some(Options::Speed) {
            self.show_panel(&t, Options::Speed);
        }
    }

    /// Punch bars, pre-roll and post-roll, what is kept, and how notes
    /// are recorded.
    fn record_options(&mut self, ui: &mut egui::Ui) {
        let o = &mut self.record;
        let mut punch = false;
        egui::Grid::new("record options")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                // ai: set_record_options
                punch |= ui
                    .checkbox(&mut o.punch, "Punch in")
                    .on_hover_text(
                        "With Record on, record only from the start of the first bar \
                         to the start of the second",
                    )
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("bar");
                    punch |= ui
                        // ai: set_record_options
                        .add(egui::DragValue::new(&mut o.punch_bars.0).range(1..=999))
                        .accessible_name("Punch in at bar")
                        .changed();
                    ui.label("to");
                    punch |= ui
                        // ai: set_record_options
                        .add(egui::DragValue::new(&mut o.punch_bars.1).range(2..=1000))
                        .accessible_name("Punch out at bar")
                        .changed();
                });
                if o.punch_bars.1 <= o.punch_bars.0 {
                    o.punch_bars.1 = o.punch_bars.0 + 1;
                }
                ui.end_row();
                ui.label("Pre-roll / post-roll");
                ui.horizontal(|ui| {
                    // ai: set_record_options
                    ui.add(crate::recording::bars(&mut o.pre_roll))
                        .accessible_name("Pre-roll bars")
                        .on_hover_text("Bars played before the punch-in");
                    punch |= ui
                        // ai: set_record_options
                        .add(crate::recording::bars(&mut o.post_roll))
                        .accessible_name("Post-roll bars")
                        .on_hover_text("Bars played after the punch-out, then playback stops")
                        .changed();
                });
                ui.end_row();
            });
        if punch && self.ctl.status().playing() {
            self.send_punch();
        }
        ui.separator();

        // Retrospective capture: what armed tracks heard, kept anyway.
        let before = self.record.capture_secs;
        ui.label("Always keep the last of armed tracks' input")
            .on_hover_text(
                "Armed tracks' input is kept in memory while playing without \
                 recording, so a good part played without Record on can still be kept",
            );
        ui.horizontal(|ui| {
            for &(s, name) in crate::recording::CAPTURE_CHOICES {
                // ai: set_record_options
                ui.selectable_value(&mut self.record.capture_secs, s, name);
            }
        });
        let held = self.ctl.captured_frames();
        let secs = held as f64 / self.rate.max(1) as f64;
        let keep = ui
            .add_enabled(
                held > 0,
                // ai: keep-last
                egui::Button::new(format!(
                    "Keep last {}:{:02}",
                    secs as u64 / 60,
                    secs as u64 % 60
                )),
            )
            .on_hover_text("Make takes from armed tracks' input that wasn't recorded")
            .on_disabled_hover_text(if self.record.capture_secs == 0 {
                "Choose how much to keep first"
            } else {
                "Nothing kept yet: arm a track and play"
            });
        if keep.clicked() {
            self.ctl.keep_capture();
        }
        if self.record.capture_secs != before {
            self.send_capture();
        }
        if self.record.capture_secs > 0 && self.tracks.iter().any(|t| t.arm) {
            // The kept time counts up.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        ui.separator();

        ui.label("Notes on instrument tracks").on_hover_text(
            "Each recording pass on an instrument track makes a new note clip, or \
             merges its notes into the clip already there (to build up a part)",
        );
        ui.horizontal(|ui| {
            for m in [crate::midi::Mode::NewClip, crate::midi::Mode::Merge] {
                // ai: set_record_options
                ui.selectable_value(&mut self.midi.mode, m, m.label());
            }
        });
        let keep_notes = ui
            // ai: keep-notes
            .add_enabled(self.midi.has_recent(), egui::Button::new("Keep notes"))
            .on_hover_text("Make clips from notes played on instrument tracks without Record on")
            .on_disabled_hover_text(
                "Nothing played yet: monitor or arm an instrument track and play",
            );
        if keep_notes.clicked() && self.keep_notes() {
            self.rebuild();
        }
    }

    /// The click's sound and level, and the count-in.
    fn metronome_options(&mut self, ui: &mut egui::Ui) {
        let mut m = self.metronome;
        egui::Grid::new("metronome options")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Clicks");
                ui.horizontal(|ui| {
                    for (n, label) in [
                        (1, "Beats"),
                        (2, "Eighths"),
                        (3, "Triplets"),
                        (4, "Sixteenths"),
                    ] {
                        // ai: set_practice
                        ui.selectable_value(&mut m.subdivision, n, label);
                    }
                });
                ui.end_row();
                ui.label("Level");
                let first = crate::spoken::start(ui);
                // ai: set_practice
                ui.add(egui::Slider::new(&mut m.level_db, -40.0..=0.0).suffix(" dB"))
                    .accessible_name("Click level");
                crate::spoken::level(ui, first, m.level_db as f64);
                ui.end_row();
                ui.label("");
                // ai: set_practice
                ui.checkbox(&mut m.accent, "Higher click on the first beat");
                ui.end_row();
                ui.label("");
                // ai: count-in
                ui.checkbox(&mut self.count_in, "Count in one bar")
                    .on_hover_text(self.commands.hover(Cmd::CountIn));
                ui.end_row();
            });
        if m != self.metronome {
            self.metronome = m;
            self.send(daw_engine::engine::Command::SetMetronome(m));
        }
    }

    /// The loop's bars. They can be dragged on the ruler too.
    fn loop_options(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("From bar");
            changed |= ui
                // ai: set_loop
                .add(egui::DragValue::new(&mut self.loop_bars.0).range(1..=999))
                .accessible_name("Loop from bar")
                .changed();
            ui.label("to");
            changed |= ui
                // ai: set_loop
                .add(egui::DragValue::new(&mut self.loop_bars.1).range(2..=1000))
                .accessible_name("Loop to bar")
                .changed();
        });
        if self.loop_bars.1 <= self.loop_bars.0 {
            self.loop_bars.1 = self.loop_bars.0 + 1;
        }
        ui.label(RichText::new("Or drag the loop's ends on the ruler.").weak());
        if changed {
            self.send_loop();
        }
    }

    /// The song's time signature at the start, and the tempo window.
    fn signature_options(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Beats per bar");
            changed |= ui
                // ai: set_tempo
                .add(egui::DragValue::new(&mut self.beats_per_bar).range(1..=12))
                .accessible_name("Beats per bar")
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Beat");
            for u in crate::tempo::UNITS {
                changed |= ui
                    // ai: set_tempo
                    .selectable_value(&mut self.time.beat_unit, u, u.to_string())
                    .changed();
            }
        });
        if changed {
            self.time_changed();
        }
        ui.separator();
        if ui
            // ai: view
            .button("Tempo and signature changes…")
            .on_hover_text("Tempo and time signature changes later in the song")
            .clicked()
        {
            self.tempo_ui.open = true;
            self.top.open = None;
        }
    }

    /// The song's key: a root and a scale, or none.
    fn key_options(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        let now = self.scale;
        ui.label("Root");
        egui::Grid::new("key roots").num_columns(6).show(ui, |ui| {
            for (i, name) in ROOTS.iter().enumerate() {
                let on = now.is_some_and(|s| s.root as usize == i);
                // ai: set_key
                if ui.selectable_label(on, *name).clicked() {
                    let kind = now.map_or(ScaleKind::Major, |s| s.kind);
                    pick = Some(Some(Scale::new(i as u8, kind)));
                }
                if i % 6 == 5 {
                    ui.end_row();
                }
            }
        });
        ui.label("Scale");
        egui::Grid::new("key scales").num_columns(2).show(ui, |ui| {
            for (i, kind) in ScaleKind::ALL.into_iter().enumerate() {
                let on = now.is_some_and(|s| s.kind == kind);
                // ai: set_key
                if ui.selectable_label(on, kind.label()).clicked() {
                    let root = now.map_or(0, |s| s.root);
                    pick = Some(Some(Scale::new(root, kind)));
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.separator();
        // ai: set_key
        if ui
            .add_enabled(now.is_some(), egui::Button::new("No key"))
            .clicked()
        {
            pick = Some(None);
        }
        if let Some(s) = pick {
            self.scale = s;
        }
    }

    /// Speed, transpose, tuning presets and the speed trainer.
    fn speed_options(&mut self, ui: &mut egui::Ui) {
        let before = (self.speed_pct, self.semitones, self.cents);
        egui::Grid::new("speed options")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Speed");
                ui.horizontal(|ui| {
                    ui.add(
                        // ai: set_practice
                        egui::DragValue::new(&mut self.speed_pct)
                            .range(50.0..=150.0)
                            .speed(0.5)
                            .fixed_decimals(0)
                            .suffix(" %"),
                    )
                    .accessible_name("Speed")
                    .on_hover_text("Slow down or speed up without changing pitch");
                    for pct in [50.0, 75.0, 100.0] {
                        if ui
                            // ai: set_practice
                            .selectable_label(self.speed_pct == pct, format!("{pct:.0}%"))
                            .clicked()
                        {
                            self.speed_pct = pct;
                        }
                    }
                });
                ui.end_row();
                ui.label("Transpose");
                ui.horizontal(|ui| {
                    ui.add(
                        // ai: set_practice
                        egui::DragValue::new(&mut self.semitones)
                            .range(-12..=12)
                            .suffix(" st"),
                    )
                    .accessible_name("Transpose, semitones")
                    .on_hover_text("Semitones, without changing speed");
                    ui.add(
                        // ai: set_practice
                        egui::DragValue::new(&mut self.cents)
                            .range(-50..=50)
                            .suffix(" ¢"),
                    )
                    .accessible_name("Fine pitch, cents")
                    .on_hover_text("Fine pitch, in cents");
                });
                ui.end_row();
            });
        ui.separator();
        ui.label("Retune a recording to play along");
        for (label, st) in [
            ("As recorded", 0),
            ("Half step down to standard (+1)", 1),
            ("Whole step down to standard (+2)", 2),
            ("Standard to half step down (−1)", -1),
            ("Standard to whole step down (−2)", -2),
        ] {
            let on = (self.semitones, self.cents) == (st, 0);
            // ai: set_practice
            if ui.selectable_label(on, label).clicked() {
                self.semitones = st;
                self.cents = 0;
            }
        }
        self.detect_tuning(ui);
        ui.separator();
        ui.horizontal(|ui| self.trainer.controls(ui));
        if (self.speed_pct, self.semitones, self.cents) != (100.0, 0, 0)
            && ui
                // ai: normal-speed
                .button("Back to normal")
                .on_hover_text(self.commands.hover(Cmd::NormalSpeed))
                .clicked()
        {
            (self.speed_pct, self.semitones, self.cents) = (100.0, 0, 0);
        }
        if (self.speed_pct, self.semitones, self.cents) != before {
            let c = self.practice();
            self.send(c);
        }
    }

    /// Measuring how far a recording is from concert pitch.
    fn detect_tuning(&mut self, ui: &mut egui::Ui) {
        for i in 0..self.tracks.len() {
            let t = &self.tracks[i];
            // The newest clip that plays.
            let source = t.clips.iter().rev().find(|c| !c.muted);
            let Some(src) = source.map(|c| c.source.clone()) else {
                continue;
            };
            let name = t.name.clone();
            if ui
                // ai: set_practice
                .selectable_label(false, format!("Detect tuning of {name}"))
                .on_hover_text(
                    "Measures how far the recording is from concert pitch \
                     (A = the tuner's reference) and suggests a fine-pitch shift. \
                     Whole-step tunings like E♭ read as in tune.",
                )
                .clicked()
            {
                let (tx, rx) = std::sync::mpsc::channel();
                let a4 = self.tuner.a4;
                std::thread::spawn(move || {
                    // The first minute and a half is plenty.
                    let r = src
                        .read_mono(90 * src.rate as usize)
                        .ok()
                        .and_then(|x| daw_engine::tuner::tuning_offset(&x, src.rate, a4));
                    let _ = tx.send(r);
                });
                self.detecting = Some((name, rx));
                self.detected = None;
            }
        }
        if self.detecting.is_some() {
            ui.spinner();
        }
        if let Some((name, r)) = self.detected.clone() {
            ui.horizontal(|ui| {
                match r {
                    Some(c) if c.abs() >= 2.0 => {
                        ui.label(format!("{name} is {c:+.0} ¢ off"));
                        // ai: set_practice
                        if ui.button(format!("Shift {:+.0} ¢", -c)).clicked() {
                            self.cents = (-c).round() as i32;
                            self.detected = None;
                        }
                    }
                    Some(_) => {
                        ui.label(format!("{name} is at concert pitch"));
                    }
                    None => {
                        ui.label(format!("{name}: no clear pitch to measure"));
                    }
                }
                // ai: view
                if ui
                    .add(IconButton::new(Icon::Close, "Dismiss").small())
                    .clicked()
                {
                    self.detected = None;
                }
            });
        }
    }

    /// What practice needs each frame, whether its panel is open or not:
    /// the trainer's steps and a finished tuning measurement.
    fn practice_tick(&mut self) {
        if let Some((name, rx)) = &self.detecting
            && let Ok(r) = rx.try_recv()
        {
            self.detected = Some((name.clone(), r));
            self.detecting = None;
        }
        let loops = self.ctl.status().loops();
        if self.looping && self.ctl.status().playing() {
            let before = self.speed_pct;
            self.trainer.update(loops, &mut self.speed_pct);
            if self.speed_pct != before {
                let c = self.practice();
                self.send(c);
            }
        }
    }

    /// Everything the status pill could say, most important first.
    fn status_lines(&self, ui: &egui::Ui) -> Vec<(Level, String)> {
        let mut out = Vec::new();
        let _ = ui;
        match &self.audio {
            Audio::Running(r) => {
                let load = self.perf.load();
                let device = self.profile.as_ref().map_or("", |p| &p.device);
                let line = format!("{device} · {} Hz · DSP {:.0}%", r.rate, load * 100.0);
                if load > 0.8 {
                    out.push((
                        Level::Problem,
                        format!("DSP {:.0}%: close to the limit", load * 100.0),
                    ));
                }
                out.push((Level::Fine, line));
            }
            Audio::Silent(..) | Audio::Stopped => {
                out.push((Level::Problem, "No audio device: playing silently".into()));
            }
        }
        let lost = self.ctl.status().rec_dropped();
        if lost > 0 {
            out.push((
                Level::Problem,
                format!("Recording fell behind: {lost} frames lost"),
            ));
        }
        let under = self.ctl.status().underruns();
        if under > 0 {
            out.push((
                Level::Problem,
                format!("Disk too slow: {under} frames missed"),
            ));
        }
        if self.recording && (self.speed_pct, self.semitones, self.cents) != (100.0, 0, 0) {
            out.push((Level::Problem, self.slowed_note()));
        }
        if let Some(s) = self.effects.scanning() {
            out.push((Level::Busy, s.to_string()));
        }
        if self.proj.loading() {
            out.push((
                Level::Busy,
                match self.proj.progress.get() {
                    (_, 0) => "Opening project…".to_string(),
                    (done, total) => format!("Opening project: {done} of {total} files…"),
                },
            ));
        }
        if !self.imports.is_empty() {
            out.push((
                Level::Busy,
                match self.imports.len() {
                    1 => "Opening 1 file…".to_string(),
                    n => format!("Opening {n} files…"),
                },
            ));
        }
        if self.renders.busy() > 0 {
            let n = self.renders.busy();
            out.push((
                Level::Busy,
                format!(
                    "Rendering{}… {:.0}%",
                    if n > 1 {
                        format!(" {n}")
                    } else {
                        String::new()
                    },
                    100.0 * self.renders.progress()
                ),
            ));
        }
        if self.processing.busy() > 0 {
            out.push((
                Level::Busy,
                match self.processing.busy() {
                    1 => "Processing 1 clip…".to_string(),
                    n => format!("Processing {n} clips…"),
                },
            ));
        }
        out.sort_by_key(|(l, _)| *l);
        out
    }

    /// One pill for the audio device, the DSP load and work going on:
    /// the most important line, and the rest when clicked.
    fn status_pill(&mut self, ui: &mut egui::Ui) {
        let lines = self.status_lines(ui);
        let Some((level, first)) = lines.first().cloned() else {
            return;
        };
        let colour = match level {
            Level::Problem => ui.visuals().error_fg_color,
            Level::Busy => ui.visuals().text_color(),
            Level::Fine => ui.visuals().weak_text_color(),
        };
        let more = lines.len() - 1;
        let icon = match level {
            Level::Problem => Icon::Warning,
            Level::Busy => Icon::More,
            Level::Fine => Icon::Speaker,
        };
        let text = RichText::new(&first).color(colour).small();
        let spoken = if more > 0 {
            format!("Status: {first}, and {more} more")
        } else {
            format!("Status: {first}")
        };
        let r = ui
            .add_sized(
                [PILL_WIDTH, ui.spacing().interact_size.y],
                // ai: view
                egui::Button::new(text)
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .selected(self.top.open == Some(Options::Status))
                    .corner_radius(ui.spacing().interact_size.y / 2.0),
            )
            .on_hover_text("Click for the audio device, DSP load and work going on")
            .accessible_name(spoken);
        // Its icon, at the start, in the line's colour.
        let side = ui.text_style_height(&egui::TextStyle::Small);
        let at = egui::Rect::from_min_size(
            egui::pos2(r.rect.left() + 6.0, r.rect.center().y - side / 2.0),
            egui::Vec2::splat(side),
        );
        if level == Level::Busy {
            egui::Spinner::new().size(side).paint_at(ui, at);
        } else {
            crate::icons::paint(ui.painter(), at, icon, colour);
        }
        if level == Level::Busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.options_panel(&r, Options::Status);
    }

    /// The status pill's panel: every line, and where to go about them.
    fn status_details(&mut self, ui: &mut egui::Ui) {
        for (level, line) in self.status_lines(ui) {
            match level {
                Level::Problem => ui.colored_label(ui.visuals().error_fg_color, line),
                _ => ui.label(line),
            };
        }
        ui.separator();
        ui.horizontal(|ui| {
            let ctx = ui.ctx().clone();
            // ai: audio-device
            if ui.button("Audio settings…").clicked() {
                self.run(&ctx, Cmd::AudioDevice);
                self.top.open = None;
            }
            // ai: performance-check
            if ui.button("Performance…").clicked() {
                self.run(&ctx, Cmd::PerformanceCheck);
                self.top.open = None;
            }
            if self.renders.busy() > 0
                // ai: view
                && ui.button("Cancel rendering").clicked()
            {
                self.renders.cancel();
            }
        });
    }

    /// The option panel open, by its id, for `get_selection`.
    pub(crate) fn top_bar_open(&self) -> Option<&'static str> {
        self.top.open.map(Options::id)
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;
    use serde_json::json;

    use super::*;
    use crate::agent::driver::Driver;

    /// Through the assistant interface: the option panels open for the
    /// person to see and `get_selection` says which; the record options
    /// are read and set by value; and the top bar shows what the tools
    /// set (UI polish plan, step two).
    #[test]
    fn the_top_bar_through_the_assistant_interface() {
        let mut d = Driver::new();
        let open = |d: &mut Driver| d.call("get_selection", json!({})).unwrap()["top_bar"].clone();
        assert_eq!(open(&mut d), json!(null));
        for (id, panel, inside) in [
            ("record-options", "record", "Punch in at bar"),
            ("loop-options", "loop", "Loop from bar"),
            ("metronome-options", "metronome", "Click level"),
            ("speed-options", "speed", "Transpose, semitones"),
        ] {
            d.command(id);
            d.h.run();
            assert_eq!(open(&mut d), json!(panel), "{id}");
            assert!(d.h.query_by_label(inside).is_some(), "{inside} shown");
        }
        // Shortcuts still work with a panel open: Space plays.
        d.h.key_press(egui::Key::Space);
        for _ in 0..50 {
            d.h.step();
            if d.app().ctl.status().playing() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(d.app().ctl.status().playing(), "Space played");
        assert_eq!(open(&mut d), json!("speed"), "and the panel stayed open");
        d.h.key_press(egui::Key::Space);
        // Escape closes it, as clicking elsewhere does.
        d.h.key_press(egui::Key::Escape);
        d.h.run();
        assert_eq!(open(&mut d), json!(null));

        let r = d
            .call(
                "set_record_options",
                json!({"punch": true, "punch_in_bar": 5, "punch_out_bar": 9,
                       "pre_roll_bars": 2, "post_roll_bars": 0, "keep_seconds": 60,
                       "notes": "merge"}),
            )
            .unwrap();
        assert_eq!(r["punch_in_bar"], 5);
        let t = d.call("get_transport", json!({})).unwrap();
        let o = &t["record_options"];
        assert_eq!(
            (
                &o["punch"],
                &o["punch_out_bar"],
                &o["pre_roll_bars"],
                &o["post_roll_bars"]
            ),
            (&json!(true), &json!(9), &json!(2), &json!(0))
        );
        assert_eq!(
            (&o["keep_seconds"], &o["notes"]),
            (&json!(60), &json!("merge"))
        );
        for bad in [
            json!({"punch_in_bar": 9, "punch_out_bar": 9}),
            json!({"pre_roll_bars": 9}),
            json!({"keep_seconds": 45}),
            json!({"notes": "both"}),
        ] {
            assert!(d.call("set_record_options", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(
            d.call("get_transport", json!({})).unwrap()["record_options"]["punch_in_bar"],
            5,
            "a refused call changes nothing"
        );
        // The panel shows what was set.
        d.command("record-options");
        d.h.run();
        assert!(d.h.query_by_label("Merge into the clip there").is_some());

        // Keep last with nothing kept says so, rather than failing.
        let r = d.command("keep-last");
        assert!(
            r["message"].as_str().unwrap().contains("Nothing to keep"),
            "{r}"
        );

        // The song's key and the practice speed show on the bar.
        d.call("set_key", json!({"key": "D dorian"})).unwrap();
        d.call(
            "set_practice",
            json!({"speed_pct": 75, "metronome": {"accent": false}}),
        )
        .unwrap();
        d.h.run();
        assert!(d.h.query_by_label("Key, D Dorian").is_some());
        assert!(!d.app().metronome.accent);
    }

    /// The loop's end drags along the foot of the ruler to another bar.
    #[test]
    fn the_loop_end_drags_on_the_ruler() {
        let mut d = Driver::new();
        let app = d.app();
        crate::timeline::add_marker(&mut app.markers, 3);
        crate::timeline::add_marker(&mut app.markers, 5);
        d.call("add_track", json!({"kind": "audio"})).unwrap();
        d.call("set_loop", json!({"from_bar": 3, "to_bar": 5, "on": true}))
            .unwrap();
        d.h.run();
        let at = |d: &Driver, bar: i64| {
            let r =
                d.h.get_by_label_contains(&format!("marker at bar {bar}"))
                    .rect();
            egui::pos2(r.left(), r.top() + crate::timeline::RULER - 5.0)
        };
        let (from, three) = (at(&d, 5), at(&d, 3));
        let to = egui::pos2(from.x + (from.x - three.x), from.y);
        d.h.event(egui::Event::PointerMoved(from));
        d.h.run();
        d.h.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        d.h.run();
        for i in 1..=10 {
            let p = from + (to - from) * (i as f32 / 10.0);
            d.h.event(egui::Event::PointerMoved(p));
            d.h.run();
        }
        d.h.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        d.h.run();
        let t = d.call("get_transport", json!({})).unwrap();
        assert_eq!(t["loop_bars"], json!([3, 7]));
        assert_eq!(t["loop"], true);
    }

    #[test]
    fn option_panels_have_distinct_ids() {
        let ids: std::collections::HashSet<_> = Options::ALL.iter().map(|o| o.id()).collect();
        assert_eq!(ids.len(), Options::ALL.len());
    }

    #[test]
    fn one_panel_at_a_time() {
        let mut t = TopBar::default();
        t.toggle(Options::Record);
        t.toggle(Options::Metronome);
        assert_eq!(t.open, Some(Options::Metronome));
        t.toggle(Options::Metronome);
        assert_eq!(t.open, None);
    }
}
