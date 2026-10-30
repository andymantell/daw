// SPDX-License-Identifier: GPL-3.0-or-later
//! Page layout: turns a score into pages of drawing items (lines, music
//! font glyphs, text, curves) that a renderer draws. Units are points, y
//! grows down the page.
//!
//! Each part is laid out in turn, as a run of systems. A system is one line
//! of bars across the page; bars are spaced by the length of their notes
//! and stretched to fill the line, and systems are stacked into pages.

use std::collections::HashMap;

use crate::drums::{DrumMap, Notehead};
use crate::glyphs as g;
use crate::score::{
    Bar, ChordDiagram, Clef, Duration, NoteValue, Part, PartKind, Score, Strum, TICKS_PER_QUARTER,
    TabSetup, Technique, TimeSig, bar_positions,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Line {
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
        dashed: bool,
    },
    /// A music-font glyph. `size` is the font's em, which is four staff
    /// spaces; `at` is the glyph's origin.
    Glyph {
        at: (f32, f32),
        glyph: char,
        size: f32,
    },
    /// Text on a baseline. `knockout` clears the background behind it, for
    /// fret numbers sitting on tab lines.
    Text {
        at: (f32, f32),
        text: String,
        size: f32,
        anchor: Anchor,
        bold: bool,
        knockout: bool,
    },
    Curve {
        from: (f32, f32),
        c1: (f32, f32),
        c2: (f32, f32),
        to: (f32, f32),
        width: f32,
    },
    /// A filled shape, such as a beam or an arrowhead.
    Polygon { points: Vec<(f32, f32)> },
    Circle {
        centre: (f32, f32),
        radius: f32,
        fill: bool,
    },
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub page_width: f32,
    pub page_height: f32,
    pub margin: f32,
    /// Staff space: the distance between two staff lines.
    pub space: f32,
}

impl Default for LayoutOptions {
    /// A4 portrait with 15 mm margins and a 1.75 mm staff space.
    fn default() -> Self {
        Self {
            page_width: 595.28,
            page_height: 841.89,
            margin: 42.5,
            space: 4.96,
        }
    }
}

const HEAD_W: f32 = 1.18;
const STEM_W: f32 = 0.12;
const STAFF_LINE_W: f32 = 0.13;
const BEAM_T: f32 = 0.5;
const BEAM_GAP: f32 = 0.25;
/// Tab lines are further apart than staff lines, so numbers fit between.
const TAB_GAP: f32 = 1.5;

/// Lay a score out into pages.
pub fn layout(score: &Score, options: &LayoutOptions) -> Vec<Page> {
    let mut b = Builder::new(options);
    if !score.title.is_empty() {
        let sp = options.space;
        let size = 4.0 * sp;
        b.ensure(size * 2.0);
        let y = b.y + size;
        let x = options.page_width / 2.0;
        b.text(x, y, &score.title, size, Anchor::Middle, true);
        b.y = y + sp;
        if !score.composer.is_empty() {
            let right = options.page_width - options.margin;
            b.text(
                right,
                b.y + 2.0 * sp,
                &score.composer,
                2.2 * sp,
                Anchor::End,
                false,
            );
            b.y += 2.6 * sp;
        }
        b.y += 2.0 * sp;
    }
    for part in &score.parts {
        b.part(part);
    }
    b.finish()
}

/// One bar's horizontal plan: a column per moment where something starts.
struct BarPlan {
    ticks: Vec<u32>,
    widths: Vec<f32>,
    /// Space before the first column (and a time signature change).
    lead: f32,
}

/// Where an event was drawn, for ties, slurs and spans that look ahead.
#[derive(Clone, Copy)]
struct Placed {
    bar: usize,
    event: usize,
    x: f32,
}

struct Rows {
    chord_y: Option<f32>,
    diagram_y: Option<f32>,
    staff_top: Option<f32>,
    /// Tab: the row for spans (palm mute, let ring, vibrato) and the row
    /// for single labels (tapping, harmonics, accents), and the tab's top.
    span_y: f32,
    label_y: f32,
    tab_top: Option<f32>,
    bottom: f32,
}

struct Builder<'a> {
    o: &'a LayoutOptions,
    sp: f32,
    pages: Vec<Page>,
    items: Vec<Item>,
    y: f32,
}

impl<'a> Builder<'a> {
    fn new(o: &'a LayoutOptions) -> Self {
        Self {
            o,
            sp: o.space,
            pages: Vec::new(),
            items: Vec::new(),
            y: o.margin,
        }
    }

    fn finish(mut self) -> Vec<Page> {
        self.new_page();
        let count = self.pages.len();
        if count > 1 {
            for (i, page) in self.pages.iter_mut().enumerate() {
                page.items.push(Item::Text {
                    at: (page.width / 2.0, page.height - self.o.margin / 2.0),
                    text: format!("{} / {count}", i + 1),
                    size: 1.8 * self.sp,
                    anchor: Anchor::Middle,
                    bold: false,
                    knockout: false,
                });
            }
        }
        self.pages
    }

    fn new_page(&mut self) {
        if self.items.is_empty() && !self.pages.is_empty() {
            return;
        }
        self.pages.push(Page {
            width: self.o.page_width,
            height: self.o.page_height,
            items: std::mem::take(&mut self.items),
        });
        self.y = self.o.margin;
    }

    /// Start a new page unless `height` more fits on this one.
    fn ensure(&mut self, height: f32) {
        if self.y + height > self.o.page_height - self.o.margin && !self.items.is_empty() {
            self.new_page();
        }
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
        self.items.push(Item::Line {
            from: (x1, y1),
            to: (x2, y2),
            width,
            dashed: false,
        });
    }

    fn dashed(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
        self.items.push(Item::Line {
            from: (x1, y1),
            to: (x2, y2),
            width,
            dashed: true,
        });
    }

    fn glyph(&mut self, x: f32, y: f32, glyph: char) {
        self.glyph_scaled(x, y, glyph, 1.0);
    }

    fn glyph_scaled(&mut self, x: f32, y: f32, glyph: char, scale: f32) {
        self.items.push(Item::Glyph {
            at: (x, y),
            glyph,
            size: 4.0 * self.sp * scale,
        });
    }

    fn text(&mut self, x: f32, y: f32, text: &str, size: f32, anchor: Anchor, bold: bool) {
        self.items.push(Item::Text {
            at: (x, y),
            text: text.to_owned(),
            size,
            anchor,
            bold,
            knockout: false,
        });
    }

    fn curve(&mut self, from: (f32, f32), to: (f32, f32), lift: f32) {
        let dx = (to.0 - from.0) / 4.0;
        self.items.push(Item::Curve {
            from,
            c1: (from.0 + dx, from.1 - lift),
            c2: (to.0 - dx, to.1 - lift),
            to,
            width: 0.14 * self.sp,
        });
    }

    fn part(&mut self, part: &Part) {
        let sp = self.sp;
        // Part heading: name, and tuning and capo for tab.
        self.ensure(12.0 * sp);
        self.y += 2.4 * sp;
        let left = self.o.margin;
        self.text(left, self.y, &part.name, 2.4 * sp, Anchor::Start, true);
        if let PartKind::Tab(setup) = &part.kind {
            let mut info = format!(
                "Tuning: {}",
                setup
                    .tuning
                    .strings
                    .iter()
                    .rev()
                    .map(|&p| crate::tuning::pitch_class_name(p))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            if setup.capo > 0 {
                info.push_str(&format!("   Capo {}", setup.capo));
            }
            self.y += 2.2 * sp;
            self.text(left, self.y, &info, 1.8 * sp, Anchor::Start, false);
        }
        self.y += sp;
        if let PartKind::Drums(map) = &part.kind {
            self.legend(part, map);
        }

        let times = part.time_signatures();
        let keys = key_signatures(part);
        let plans: Vec<BarPlan> = part
            .bars
            .iter()
            .enumerate()
            .map(|(i, bar)| plan_bar(part, bar, times[i], keys[i], sp))
            .collect();
        let width = self.o.page_width - 2.0 * self.o.margin;

        let mut start = 0;
        while start < part.bars.len() {
            let head = self.system_head_width(part, keys[start], true);
            let mut used = head;
            let mut end = start;
            while end < part.bars.len() {
                let p = &plans[end];
                let w = p.lead + p.widths.iter().sum::<f32>() + sp;
                if end > start && used + w > width {
                    break;
                }
                used += w;
                end += 1;
            }
            let last = end == part.bars.len();
            self.system(part, start..end, &plans, &times, &keys, used, last);
            start = end;
        }
        self.y += 2.0 * sp;
    }

    fn system_head_width(&self, part: &Part, key: i8, with_time: bool) -> f32 {
        let sp = self.sp;
        let clef = 3.6 * sp;
        let key_w = match part.kind {
            PartKind::Drums(_) => 0.0,
            _ => f32::from(key.unsigned_abs()) * 1.1 * sp,
        };
        let time = if with_time { 2.6 * sp } else { 0.0 };
        clef + key_w + time + sp
    }

    #[allow(clippy::too_many_arguments)]
    fn system(
        &mut self,
        part: &Part,
        bars: std::ops::Range<usize>,
        plans: &[BarPlan],
        times: &[TimeSig],
        keys: &[i8],
        natural: f32,
        last: bool,
    ) {
        let sp = self.sp;
        let width = self.o.page_width - 2.0 * self.o.margin;
        let first = bars.start;
        let show_time = first == 0 || part.bars[first].time.is_some();
        let head = self.system_head_width(part, keys[first], show_time);

        // Stretch the columns to fill the line, except a short last line.
        let cols: f32 = bars
            .clone()
            .map(|i| plans[i].widths.iter().sum::<f32>())
            .sum();
        let fixed = natural - cols - self.system_head_width(part, keys[first], true) + head;
        let mut stretch = ((width - fixed) / cols.max(1.0)).max(0.5);
        if last && natural < width * 0.75 {
            stretch = 1.0;
        }

        let rows = self.rows(part, bars.clone());
        self.ensure(rows.bottom);
        let top = self.y;
        let rows = Rows {
            chord_y: rows.chord_y.map(|y| y + top),
            diagram_y: rows.diagram_y.map(|y| y + top),
            staff_top: rows.staff_top.map(|y| y + top),
            span_y: rows.span_y + top,
            label_y: rows.label_y + top,
            tab_top: rows.tab_top.map(|y| y + top),
            bottom: rows.bottom + top,
        };

        // Column x positions.
        let x0 = self.o.margin;
        let mut x = x0 + head;
        let mut col_x: HashMap<(usize, u32), f32> = HashMap::new();
        let mut bar_lines = Vec::new();
        for i in bars.clone() {
            let p = &plans[i];
            x += p.lead;
            for (t, w) in p.ticks.iter().zip(&p.widths) {
                col_x.insert((i, *t), x);
                x += w * stretch;
            }
            x += sp;
            bar_lines.push(x);
        }
        let x_end = if last && stretch == 1.0 {
            x
        } else {
            x0 + width
        };
        if let Some(l) = bar_lines.last_mut() {
            *l = x_end;
        }

        // Every event's x, per voice in time order.
        let mut voices: HashMap<u8, Vec<Placed>> = HashMap::new();
        for i in bars.clone() {
            for (t, e) in bar_positions(&part.bars[i]) {
                let x = col_x[&(i, t)];
                let v = part.bars[i].events[e].voice;
                voices.entry(v).or_default().push(Placed {
                    bar: i,
                    event: e,
                    x,
                });
            }
        }

        // Bar number at the start of the line.
        let number_y = rows.staff_top.or(rows.tab_top).unwrap_or(top) - 1.2 * sp;
        let number_y = rows
            .chord_y
            .map_or(number_y, |c| number_y.min(c - 2.0 * sp));
        self.text(
            x0,
            number_y,
            &format!("{}", first + 1),
            1.6 * sp,
            Anchor::Start,
            false,
        );

        // Chord names and boxes.
        if let Some(cy) = rows.chord_y {
            for i in bars.clone() {
                for chord in &part.bars[i].chords {
                    let x = col_x
                        .get(&(i, chord.tick))
                        .copied()
                        .unwrap_or_else(|| col_x_near(&col_x, i, chord.tick));
                    self.text(x, cy, &chord.name, 2.0 * sp, Anchor::Start, true);
                    if let (Some(dy), Some(d)) = (rows.diagram_y, &chord.diagram) {
                        self.diagram(x, dy, d);
                    }
                }
            }
        }

        match &part.kind {
            PartKind::Tab(setup) => {
                if let Some(staff_top) = rows.staff_top {
                    let clef = if setup.tuning.is_bass() {
                        Clef::Bass8vb
                    } else {
                        Clef::Treble8vb
                    };
                    self.staff_lines(x0, x_end, staff_top, 5, sp);
                    self.staff_head(
                        x0,
                        staff_top,
                        Some(clef),
                        keys[first],
                        show_time.then_some(times[first]),
                    );
                    self.pitched_notes(part, &voices, staff_top, clef, keys, times);
                    self.bar_lines(&bar_lines, staff_top, staff_top + 4.0 * sp, last);
                }
                let tab_top = rows.tab_top.expect("tab parts have a tab row");
                self.tab(
                    part, setup, &voices, &rows, tab_top, x0, x_end, show_time, times, first,
                );
                let n = setup.tuning.string_count().max(1) as f32;
                self.bar_lines(
                    &bar_lines,
                    tab_top,
                    tab_top + (n - 1.0) * TAB_GAP * sp,
                    last,
                );
            }
            PartKind::Staff { clef } => {
                let staff_top = rows.staff_top.expect("staff row");
                self.staff_lines(x0, x_end, staff_top, 5, sp);
                self.staff_head(
                    x0,
                    staff_top,
                    Some(*clef),
                    keys[first],
                    show_time.then_some(times[first]),
                );
                self.pitched_notes(part, &voices, staff_top, *clef, keys, times);
                self.bar_lines(&bar_lines, staff_top, staff_top + 4.0 * sp, last);
            }
            PartKind::Drums(map) => {
                let staff_top = rows.staff_top.expect("staff row");
                self.staff_lines(x0, x_end, staff_top, 5, sp);
                self.staff_head(x0, staff_top, None, 0, show_time.then_some(times[first]));
                self.drum_notes(part, map, &voices, staff_top, times);
                self.bar_lines(&bar_lines, staff_top, staff_top + 4.0 * sp, last);
            }
        }

        // Time signature changes inside the line.
        for i in bars.clone().skip(1) {
            if let Some(t) = part.bars[i].time {
                let x = plans[i].ticks.first().map_or(0.0, |t0| col_x[&(i, *t0)]) - 2.4 * sp;
                for top in [
                    rows.staff_top,
                    rows.tab_top.filter(|_| rows.staff_top.is_none()),
                ]
                .into_iter()
                .flatten()
                {
                    let h = if Some(top) == rows.tab_top {
                        tab_height(part, sp)
                    } else {
                        4.0 * sp
                    };
                    self.time_sig(x, top + h / 2.0, t);
                }
            }
        }

        self.y = rows.bottom;
    }

    /// Vertical rows of a system, relative to its top.
    fn rows(&self, part: &Part, bars: std::ops::Range<usize>) -> Rows {
        let sp = self.sp;
        let mut y = 0.0;
        let any_chords = bars.clone().any(|i| !part.bars[i].chords.is_empty());
        let any_diagrams = bars
            .clone()
            .any(|i| part.bars[i].chords.iter().any(|c| c.diagram.is_some()));
        let mut chord_y = None;
        let mut diagram_y = None;
        if any_chords {
            y += 2.4 * sp;
            chord_y = Some(y);
            y += 0.8 * sp;
            if any_diagrams {
                diagram_y = Some(y + 1.4 * sp);
                y += 8.0 * sp;
            }
        }
        let (mut staff_top, mut tab_top) = (None, None);
        let (mut span_y, mut label_y) = (0.0, 0.0);
        match &part.kind {
            PartKind::Tab(setup) => {
                if setup.show_staff {
                    y += 3.5 * sp;
                    staff_top = Some(y);
                    y += 4.0 * sp + 4.5 * sp;
                } else {
                    y += 0.5 * sp;
                }
                span_y = y + 1.2 * sp;
                label_y = y + 2.8 * sp;
                y += 3.8 * sp;
                tab_top = Some(y);
                y += tab_height(part, sp);
                y += if setup.show_rhythm {
                    5.0 * sp
                } else {
                    2.0 * sp
                };
            }
            PartKind::Staff { .. } | PartKind::Drums(_) => {
                y += 4.0 * sp;
                staff_top = Some(y);
                y += 4.0 * sp + 5.0 * sp;
            }
        }
        Rows {
            chord_y,
            diagram_y,
            staff_top,
            span_y,
            label_y,
            tab_top,
            bottom: y + 2.0 * sp,
        }
    }

    fn staff_lines(&mut self, x1: f32, x2: f32, top: f32, lines: usize, gap: f32) {
        for l in 0..lines {
            let y = top + l as f32 * gap;
            self.line(x1, y, x2, y, STAFF_LINE_W * self.sp);
        }
    }

    fn bar_lines(&mut self, xs: &[f32], top: f32, bottom: f32, last: bool) {
        let sp = self.sp;
        for (i, &x) in xs.iter().enumerate() {
            if last && i + 1 == xs.len() {
                self.line(x - 0.9 * sp, top, x - 0.9 * sp, bottom, 0.16 * sp);
                self.line(x - 0.25 * sp, top, x - 0.25 * sp, bottom, 0.5 * sp);
            } else {
                self.line(x, top, x, bottom, 0.16 * sp);
            }
        }
    }

    /// Clef, key signature and time signature at the start of a line.
    fn staff_head(
        &mut self,
        x0: f32,
        top: f32,
        clef: Option<Clef>,
        key: i8,
        time: Option<TimeSig>,
    ) {
        let sp = self.sp;
        let mut x = x0 + 0.8 * sp;
        match clef {
            Some(c @ (Clef::Treble | Clef::Treble8vb)) => {
                let glyph = if c == Clef::Treble {
                    g::G_CLEF
                } else {
                    g::G_CLEF_8VB
                };
                self.glyph(x, top + 3.0 * sp, glyph);
            }
            Some(c @ (Clef::Bass | Clef::Bass8vb)) => {
                let glyph = if c == Clef::Bass {
                    g::F_CLEF
                } else {
                    g::F_CLEF_8VB
                };
                self.glyph(x, top + sp, glyph);
            }
            None => self.glyph(x + 0.5 * sp, top + 2.0 * sp, g::PERCUSSION_CLEF),
        }
        x += 3.6 * sp - 0.8 * sp;
        if let Some(clef) = clef {
            let bass = matches!(clef, Clef::Bass | Clef::Bass8vb);
            for place in key_places(key) {
                let place = if bass { place - 2 } else { place };
                let y = top + 4.0 * sp - f32::from(place) * sp / 2.0;
                let glyph = if key > 0 {
                    g::ACCIDENTAL_SHARP
                } else {
                    g::ACCIDENTAL_FLAT
                };
                self.glyph(x, y, glyph);
                x += 1.1 * sp;
            }
        }
        if let Some(t) = time {
            self.time_sig(x + 0.3 * sp, top + 2.0 * sp, t);
        }
    }

    fn time_sig(&mut self, x: f32, middle: f32, t: TimeSig) {
        let sp = self.sp;
        for (n, y) in [(t.beats, middle - sp), (t.beat_type, middle + sp)] {
            let digits: Vec<u32> = n
                .to_string()
                .chars()
                .filter_map(|c| c.to_digit(10))
                .collect();
            let mut dx = x + if digits.len() > 1 { -0.9 * sp } else { 0.0 };
            for d in digits {
                self.glyph(dx, y, g::time_sig_digit(d));
                dx += 1.8 * sp;
            }
        }
    }

    fn diagram(&mut self, x: f32, top: f32, d: &ChordDiagram) {
        let sp = self.sp;
        let strings = d.frets.len().max(2);
        let gap = 0.9 * sp;
        let fret_h = 1.1 * sp;
        let frets = 4;
        let w = (strings - 1) as f32 * gap;
        let line_w = 0.1 * sp;
        for s in 0..strings {
            let sx = x + s as f32 * gap;
            self.line(sx, top, sx, top + frets as f32 * fret_h, line_w);
        }
        for f in 0..=frets {
            let fy = top + f as f32 * fret_h;
            self.line(x, fy, x + w, fy, line_w);
        }
        if d.first_fret <= 1 {
            self.line(x, top - 0.15 * sp, x + w, top - 0.15 * sp, 0.35 * sp);
        } else {
            self.text(
                x - 0.5 * sp,
                top + 0.9 * fret_h,
                &format!("{}", d.first_fret),
                1.3 * sp,
                Anchor::End,
                false,
            );
        }
        // Columns run from the lowest string on the left to string 1.
        for (i, fret) in d.frets.iter().enumerate() {
            let sx = x + (strings - 1 - i) as f32 * gap;
            match fret {
                None => self.text(sx, top - 0.5 * sp, "x", 1.2 * sp, Anchor::Middle, false),
                Some(0) => self.items.push(Item::Circle {
                    centre: (sx, top - 0.85 * sp),
                    radius: 0.3 * sp,
                    fill: false,
                }),
                Some(f) => {
                    let row = i32::from(*f) - i32::from(d.first_fret.max(1));
                    if (0..frets as i32).contains(&row) {
                        self.items.push(Item::Circle {
                            centre: (sx, top + (row as f32 + 0.5) * fret_h),
                            radius: 0.35 * sp,
                            fill: true,
                        });
                    }
                }
            }
        }
    }

    fn legend(&mut self, part: &Part, map: &DrumMap) {
        let sp = self.sp;
        let used = part
            .bars
            .iter()
            .flat_map(|b| &b.events)
            .flat_map(|e| &e.notes)
            .map(|n| n.pitch);
        let entries = map.legend(used);
        if entries.is_empty() {
            return;
        }
        let entry_w = 11.0 * sp;
        let per_row = ((self.o.page_width - 2.0 * self.o.margin) / entry_w)
            .floor()
            .max(1.0) as usize;
        for row in entries.chunks(per_row) {
            self.ensure(11.0 * sp);
            let top = self.y + 3.0 * sp;
            for (i, e) in row.iter().enumerate() {
                let x = self.o.margin + i as f32 * entry_w;
                self.staff_lines(x + 2.0 * sp, x + 7.0 * sp, top, 5, sp);
                let y = top + 4.0 * sp - f32::from(e.place) * sp / 2.0;
                let nx = x + 3.9 * sp;
                self.ledgers(nx, top, i32::from(e.place), HEAD_W);
                self.glyph(nx, y, notehead_glyph(e.notehead, NoteValue::Quarter));
                self.text(
                    x + 4.5 * sp,
                    top + 6.6 * sp,
                    &e.name,
                    1.4 * sp,
                    Anchor::Middle,
                    false,
                );
            }
            self.y = top + 8.0 * sp;
        }
    }

    fn ledgers(&mut self, x: f32, top: f32, place: i32, head_w: f32) {
        let sp = self.sp;
        let lines: Vec<i32> = if place <= -2 {
            (place..=-2).filter(|p| p % 2 == 0).collect()
        } else if place >= 10 {
            (10..=place).filter(|p| p % 2 == 0).collect()
        } else {
            Vec::new()
        };
        for p in lines {
            let y = top + 4.0 * sp - p as f32 * sp / 2.0;
            self.line(x - 0.4 * sp, y, x + (head_w + 0.4) * sp, y, 0.16 * sp);
        }
    }

    /// Notes on a pitched staff, for staff parts and the staff above tab.
    fn pitched_notes(
        &mut self,
        part: &Part,
        voices: &HashMap<u8, Vec<Placed>>,
        top: f32,
        clef: Clef,
        keys: &[i8],
        times: &[TimeSig],
    ) {
        let sp = self.sp;
        let mut acc_cache: HashMap<usize, Vec<Vec<Option<i8>>>> = HashMap::new();
        let mut voice_ids: Vec<u8> = voices.keys().copied().collect();
        voice_ids.sort_unstable();
        for v in voice_ids {
            let placed = &voices[&v];
            let mut heads: Vec<StemNote> = Vec::new();
            for (k, p) in placed.iter().enumerate() {
                let bar = &part.bars[p.bar];
                let e = &bar.events[p.event];
                let multi = bar_voice_count(bar) > 1;
                let acc = acc_cache
                    .entry(p.bar)
                    .or_insert_with(|| accidentals(bar, keys[p.bar], clef.transpose()));
                let middle = top + 2.0 * sp;
                if e.is_rest() {
                    let shift = if multi {
                        if v % 2 == 1 { -2.0 } else { 2.0 }
                    } else {
                        0.0
                    };
                    self.rest(p.x, middle + shift * sp, e.duration);
                    continue;
                }
                let places: Vec<i32> = e
                    .notes
                    .iter()
                    .map(|n| {
                        spell(i32::from(n.pitch) + clef.transpose(), keys[p.bar]).0
                            - clef.bottom_line_step()
                    })
                    .collect();
                let up = if multi {
                    v % 2 == 1
                } else {
                    places.iter().sum::<i32>() < 4 * places.len() as i32
                };
                for (n, note) in e.notes.iter().enumerate() {
                    let place = places[n];
                    let y = top + 4.0 * sp - place as f32 * sp / 2.0;
                    let head_w = if e.duration.value == NoteValue::Whole {
                        1.69
                    } else {
                        HEAD_W
                    };
                    self.ledgers(p.x, top, place, head_w);
                    let glyph = if note.has(&Technique::Dead) {
                        notehead_glyph(Notehead::X, e.duration.value)
                    } else {
                        notehead_glyph(Notehead::Normal, e.duration.value)
                    };
                    self.glyph(p.x, y, glyph);
                    if let Some(alter) = acc[p.event][n] {
                        let glyph = match alter {
                            1 => g::ACCIDENTAL_SHARP,
                            -1 => g::ACCIDENTAL_FLAT,
                            _ => g::ACCIDENTAL_NATURAL,
                        };
                        self.glyph(p.x - 1.3 * sp, y, glyph);
                    }
                    self.dots(p.x + head_w * sp, y, place, e.duration.dots);
                    if note.tie {
                        self.tie(placed, k, part, note.pitch, p.x, y, up);
                    }
                }
                let lo = *places.iter().min().expect("not a rest");
                let hi = *places.iter().max().expect("not a rest");
                let place_y = |pl: i32| top + 4.0 * sp - pl as f32 * sp / 2.0;
                let (far, near) = if up {
                    (place_y(lo), place_y(hi))
                } else {
                    (place_y(hi), place_y(lo))
                };
                let stem_x = if up {
                    p.x + (HEAD_W - STEM_W / 2.0) * sp
                } else {
                    p.x + STEM_W / 2.0 * sp
                };
                heads.push(StemNote {
                    x: stem_x,
                    far,
                    near,
                    up,
                    duration: e.duration,
                    group: beat_group(times[p.bar], bar_tick(bar, p.event), p.bar),
                    tremolo: false,
                });
                if e.notes.iter().any(|n| n.has(&Technique::Accent)) {
                    let y = if up {
                        place_y(lo) + 1.6 * sp
                    } else {
                        place_y(hi) - 1.6 * sp
                    };
                    let glyph = if up { g::ACCENT_BELOW } else { g::ACCENT_ABOVE };
                    self.glyph(p.x - 0.1 * sp, y, glyph);
                }
            }
            self.stems(&heads, 3.5, Some(top + 2.0 * sp));
        }
    }

    fn dots(&mut self, x: f32, y: f32, place: i32, dots: u8) {
        let sp = self.sp;
        let y = if place % 2 == 0 { y - sp / 2.0 } else { y };
        for d in 0..dots {
            self.glyph(x + (0.4 + f32::from(d) * 0.6) * sp, y, g::AUGMENTATION_DOT);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tie(
        &mut self,
        placed: &[Placed],
        k: usize,
        part: &Part,
        pitch: u8,
        x: f32,
        y: f32,
        up: bool,
    ) {
        let sp = self.sp;
        let lift = if up { -0.9 * sp } else { 0.9 * sp };
        let dy = if up { 0.7 * sp } else { -0.7 * sp };
        let to = match placed.get(k + 1) {
            Some(next)
                if part.bars[next.bar].events[next.event]
                    .notes
                    .iter()
                    .any(|n| n.pitch == pitch) =>
            {
                next.x
            }
            _ => x + 3.0 * sp,
        };
        self.curve((x + 1.0 * sp, y + dy), (to + 0.2 * sp, y + dy), lift);
    }

    fn rest(&mut self, x: f32, middle: f32, d: Duration) {
        let sp = self.sp;
        let (glyph, y) = match d.value {
            NoteValue::Whole => (g::REST_WHOLE, middle - sp),
            NoteValue::Half => (g::REST_HALF, middle),
            NoteValue::Quarter => (g::REST_QUARTER, middle),
            v => (g::RESTS_SHORT[v.beams() - 1], middle),
        };
        self.glyph(x, y, glyph);
        for i in 0..d.dots {
            self.glyph(
                x + (1.5 + f32::from(i) * 0.6) * sp,
                middle - 0.5 * sp,
                g::AUGMENTATION_DOT,
            );
        }
    }

    /// Stems, flags, beams and tuplet numbers for one voice's notes.
    /// With a `middle` line, stems of notes beyond the staff reach it.
    fn stems(&mut self, notes: &[StemNote], length: f32, middle: Option<f32>) {
        let sp = self.sp;
        let mut i = 0;
        while i < notes.len() {
            // Collect a beam group.
            let mut j = i + 1;
            if notes[i].duration.value.beams() > 0 {
                while j < notes.len()
                    && notes[j].duration.value.beams() > 0
                    && notes[j].group == notes[i].group
                {
                    j += 1;
                }
            }
            let group = &notes[i..j];
            let up = if group.len() > 1 {
                group.iter().filter(|n| n.up).count() * 2 >= group.len()
            } else {
                group[0].up
            };
            let dir = if up { -1.0 } else { 1.0 };
            if group.len() == 1 || group[0].duration.value.beams() == 0 {
                let n = &group[0];
                if n.duration.value != NoteValue::Whole {
                    let end = stem_end(n.near, dir, length * sp, middle);
                    self.line(n.x, n.far, n.x, end, STEM_W * sp);
                    let beams = n.duration.value.beams();
                    if beams > 0 {
                        let flag = if n.up {
                            g::FLAGS_UP[beams - 1]
                        } else {
                            g::FLAGS_DOWN[beams - 1]
                        };
                        self.glyph(n.x - STEM_W / 2.0 * sp, end, flag);
                    }
                    if n.tremolo {
                        self.glyph(n.x, (n.near + end) / 2.0, g::TREMOLO_3);
                    }
                }
            } else {
                // A straight beam from the first stem end to the last, kept
                // shallow and clear of every note.
                let ends: Vec<f32> = group
                    .iter()
                    .map(|n| stem_end(n.near, dir, length * sp, middle))
                    .collect();
                let (x1, x2) = (group[0].x, group[group.len() - 1].x);
                let mut rise = (ends[ends.len() - 1] - ends[0]).clamp(-sp, sp);
                if (x2 - x1).abs() < 1.0 {
                    rise = 0.0;
                }
                let slope = rise / (x2 - x1).max(1.0);
                let mut y1 = ends[0];
                for (n, e) in group.iter().zip(&ends) {
                    let at = y1 + slope * (n.x - x1);
                    let shift = if up {
                        (at - e).max(0.0)
                    } else {
                        (at - e).min(0.0)
                    };
                    y1 -= shift;
                }
                let beam_y = |x: f32| y1 + slope * (x - x1);
                for n in group {
                    self.line(n.x, n.far, n.x, beam_y(n.x), STEM_W * sp);
                    if n.tremolo {
                        self.glyph(n.x, (n.near + beam_y(n.x)) / 2.0, g::TREMOLO_3);
                    }
                }
                let max_beams = group
                    .iter()
                    .map(|n| n.duration.value.beams())
                    .max()
                    .unwrap_or(1);
                for level in 0..max_beams {
                    let off = -dir * level as f32 * (BEAM_T + BEAM_GAP) * sp;
                    let mut k = 0;
                    while k < group.len() {
                        if group[k].duration.value.beams() <= level {
                            k += 1;
                            continue;
                        }
                        let mut m = k;
                        while m + 1 < group.len() && group[m + 1].duration.value.beams() > level {
                            m += 1;
                        }
                        let (mut bx1, mut bx2) = (group[k].x, group[m].x);
                        if k == m {
                            // A stub towards the neighbour.
                            if k + 1 < group.len() {
                                bx2 = bx1 + 1.2 * sp;
                            } else {
                                bx1 = bx2 - 1.2 * sp;
                            }
                        }
                        self.beam(bx1, beam_y(bx1) + off, bx2, beam_y(bx2) + off, dir);
                        k = m + 1;
                    }
                }
            }
            // Tuplet number over a full tuplet group.
            if let Some(t) = group[0].duration.tuplet {
                let n = usize::from(t.actual).min(group.len());
                if group.iter().take(n).all(|g| g.duration.tuplet == Some(t)) && j - i >= n {
                    let mid = (group[0].x + group[n - 1].x) / 2.0;
                    let edge = group
                        .iter()
                        .take(n)
                        .map(|g| stem_end(g.near, dir, length * sp, middle))
                        .fold(
                            if up { f32::INFINITY } else { f32::NEG_INFINITY },
                            |a, b| {
                                if up { a.min(b) } else { a.max(b) }
                            },
                        );
                    let y = if up { edge - 0.8 * sp } else { edge + 2.0 * sp };
                    self.text(
                        mid,
                        y,
                        &t.actual.to_string(),
                        1.5 * sp,
                        Anchor::Middle,
                        false,
                    );
                }
            }
            i = j;
        }
    }

    fn beam(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, dir: f32) {
        let t = BEAM_T * self.sp * -dir;
        let half = STEM_W / 2.0 * self.sp;
        self.items.push(Item::Polygon {
            points: vec![
                (x1 - half, y1),
                (x2 + half, y2),
                (x2 + half, y2 + t),
                (x1 - half, y1 + t),
            ],
        });
    }

    fn drum_notes(
        &mut self,
        part: &Part,
        map: &DrumMap,
        voices: &HashMap<u8, Vec<Placed>>,
        top: f32,
        times: &[TimeSig],
    ) {
        let sp = self.sp;
        let mut voice_ids: Vec<u8> = voices.keys().copied().collect();
        voice_ids.sort_unstable();
        for v in voice_ids {
            let mut heads = Vec::new();
            for p in &voices[&v] {
                let bar = &part.bars[p.bar];
                let e = &bar.events[p.event];
                let multi = bar_voice_count(bar) > 1;
                let up = v % 2 == 1;
                if e.is_rest() {
                    let shift = if multi {
                        if up { -2.0 } else { 2.0 }
                    } else {
                        0.0
                    };
                    self.rest(p.x, top + 2.0 * sp + shift * sp, e.duration);
                    continue;
                }
                let mut places = Vec::new();
                let mut roll = false;
                for note in &e.notes {
                    let (place, mut head, _) = map.look_up(note.pitch);
                    if note.has(&Technique::CrossStick) {
                        head = Notehead::CircleX;
                    } else if note.has(&Technique::Rimshot) {
                        head = Notehead::Slashed;
                    }
                    let place = i32::from(place);
                    places.push(place);
                    let y = top + 4.0 * sp - place as f32 * sp / 2.0;
                    self.ledgers(p.x, top, place, HEAD_W);
                    self.glyph(p.x, y, notehead_glyph(head, e.duration.value));
                    if note.has(&Technique::Ghost) {
                        self.glyph(p.x - 0.4 * sp, y, g::NOTEHEAD_PAREN_LEFT);
                        self.glyph(p.x + (HEAD_W + 0.1) * sp, y, g::NOTEHEAD_PAREN_RIGHT);
                    }
                    if note.has(&Technique::Choke) {
                        self.glyph(p.x + (HEAD_W + 0.3) * sp, y - 0.2 * sp, g::CHOKE_CYMBAL);
                    }
                    let graces = if note.has(&Technique::Drag) {
                        2
                    } else if note.has(&Technique::Flam) {
                        1
                    } else {
                        0
                    };
                    for k in 0..graces {
                        let gx = p.x - (1.4 + 0.9 * k as f32) * sp;
                        self.glyph_scaled(gx, y, g::NOTEHEAD_BLACK, 0.6);
                        let sx = gx + (HEAD_W * 0.6 - 0.05) * sp;
                        self.line(sx, y, sx, y - 2.2 * sp, 0.09 * sp);
                        if graces == 1 {
                            self.glyph_scaled(
                                sx - 0.6 * sp,
                                y - 1.1 * sp,
                                g::GRACE_SLASH_STEM_UP,
                                0.6,
                            );
                        }
                    }
                    if graces == 2 {
                        let (a, b) = (p.x - 2.3 * sp + 0.65 * sp, p.x - 1.4 * sp + 0.65 * sp);
                        self.line(a, y - 2.2 * sp, b, y - 2.2 * sp, 0.25 * sp);
                    }
                    roll |= note.has(&Technique::Roll);
                }
                let lo = *places.iter().min().expect("not a rest");
                let hi = *places.iter().max().expect("not a rest");
                let place_y = |pl: i32| top + 4.0 * sp - pl as f32 * sp / 2.0;
                let (far, near) = if up {
                    (place_y(lo), place_y(hi))
                } else {
                    (place_y(hi), place_y(lo))
                };
                // Articulations go on the stem side, beyond the stem.
                let mut mark_y = if up {
                    (near - 4.8 * sp).min(top - 1.2 * sp)
                } else {
                    (near + 4.6 * sp).max(top + 5.2 * sp)
                };
                for note in &e.notes {
                    for (t, glyph) in [
                        (
                            Technique::Accent,
                            if up { g::ACCENT_ABOVE } else { g::ACCENT_BELOW },
                        ),
                        (Technique::Open, g::MUTE_OPEN),
                        (Technique::Closed, g::MUTE_CLOSED),
                    ] {
                        if note.has(&t)
                            || (t == Technique::Open && note.pitch == 46 && map.entry(46).is_some())
                        {
                            self.glyph(p.x, mark_y, glyph);
                            mark_y += if up { -1.5 * sp } else { 1.5 * sp };
                        }
                    }
                }
                let stem_x = if up {
                    p.x + (HEAD_W - STEM_W / 2.0) * sp
                } else {
                    p.x + STEM_W / 2.0 * sp
                };
                heads.push(StemNote {
                    x: stem_x,
                    far,
                    near,
                    up,
                    duration: e.duration,
                    group: beat_group(times[p.bar], bar_tick(bar, p.event), p.bar),
                    tremolo: roll,
                });
            }
            self.stems(&heads, 3.5, Some(top + 2.0 * sp));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tab(
        &mut self,
        part: &Part,
        setup: &TabSetup,
        voices: &HashMap<u8, Vec<Placed>>,
        rows: &Rows,
        top: f32,
        x0: f32,
        x_end: f32,
        show_time: bool,
        times: &[TimeSig],
        first: usize,
    ) {
        let sp = self.sp;
        let n = setup.tuning.string_count();
        let gap = TAB_GAP * sp;
        let height = (n.max(1) - 1) as f32 * gap;
        self.staff_lines(x0, x_end, top, n, gap);
        // The tab clef, scaled to the tab's height.
        let (clef, clef_h) = if n <= 4 {
            (g::TAB_CLEF_4, 4.5)
        } else {
            (g::TAB_CLEF_6, 6.05)
        };
        let scale = (height * 0.8 / (clef_h * sp)).max(0.4);
        self.glyph_scaled(x0 + 1.0 * sp, top + height / 2.0, clef, scale);
        if show_time && rows.staff_top.is_none() {
            self.time_sig(x0 + 3.6 * sp, top + height / 2.0, times[first]);
        }
        let string_y = |s: u8| top + f32::from(s.max(1) - 1) * gap;
        let num_size = 1.35 * sp;
        let centre = HEAD_W / 2.0 * sp;
        let bottom = top + height;

        // Spans across consecutive notes: palm mute and let ring.
        let mut order: Vec<Placed> = voices.values().flatten().copied().collect();
        order.sort_by(|a, b| a.x.total_cmp(&b.x));
        for (technique, label) in [
            (Technique::PalmMute, "P.M."),
            (Technique::LetRing, "let ring"),
        ] {
            let has = |p: &Placed| {
                part.bars[p.bar].events[p.event]
                    .notes
                    .iter()
                    .any(|n| n.has(&technique))
            };
            let mut k = 0;
            while k < order.len() {
                if !has(&order[k]) {
                    k += 1;
                    continue;
                }
                let mut m = k;
                while m + 1 < order.len() && has(&order[m + 1]) {
                    m += 1;
                }
                let x1 = order[k].x;
                let x2 = order[m].x + 2.0 * sp;
                let y = rows.span_y;
                self.text(x1, y, label, 1.4 * sp, Anchor::Start, false);
                let label_w = label.len() as f32 * 0.6 * sp + 0.5 * sp;
                if x2 > x1 + label_w {
                    self.dashed(x1 + label_w, y - 0.4 * sp, x2, y - 0.4 * sp, 0.1 * sp);
                    self.line(x2, y - 0.4 * sp, x2, y + 0.3 * sp, 0.1 * sp);
                }
                k = m + 1;
            }
        }

        let mut voice_ids: Vec<u8> = voices.keys().copied().collect();
        voice_ids.sort_unstable();
        for v in &voice_ids {
            let placed = &voices[v];
            let mut stems = Vec::new();
            for (k, p) in placed.iter().enumerate() {
                let bar = &part.bars[p.bar];
                let e = &bar.events[p.event];
                let cx = p.x + centre;
                if e.is_rest() {
                    if setup.show_rhythm {
                        self.rest(p.x, bottom + 3.0 * sp, e.duration);
                    }
                    continue;
                }
                if let Some(dir) = e.strum {
                    // A line the height of the chord's strings with the
                    // strum glyph as its arrowhead.
                    let strings: Vec<f32> = e
                        .notes
                        .iter()
                        .filter_map(|n| n.string)
                        .map(string_y)
                        .collect();
                    let hi = strings.iter().copied().fold(top, f32::max);
                    let lo = strings.iter().copied().fold(bottom, f32::min);
                    let ax = p.x - 1.1 * sp;
                    let tip_down = dir == Strum::Down;
                    let (from, to) = if tip_down { (lo, hi) } else { (hi, lo) };
                    self.line(ax, from, ax, to, 0.12 * sp);
                    self.arrow(
                        (ax, to + if tip_down { 0.1 * sp } else { -0.1 * sp }),
                        !tip_down,
                    );
                }
                let tied_in: Vec<u8> = k
                    .checked_sub(1)
                    .map(|j| {
                        let prev = &placed[j];
                        part.bars[prev.bar].events[prev.event]
                            .notes
                            .iter()
                            .filter(|n| n.tie)
                            .map(|n| n.pitch)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut labels: Vec<&str> = Vec::new();
                for note in &e.notes {
                    let (Some(s), Some(fret)) = (note.string, note.fret) else {
                        continue;
                    };
                    let y = string_y(s);
                    let tied = tied_in.contains(&note.pitch);
                    let text = if note.has(&Technique::Dead) {
                        "x".to_owned()
                    } else if note.has(&Technique::NaturalHarmonic) {
                        format!("<{fret}>")
                    } else if note.has(&Technique::Ghost) || tied {
                        format!("({fret})")
                    } else {
                        fret.to_string()
                    };
                    self.items.push(Item::Text {
                        at: (cx, y + 0.36 * num_size),
                        text,
                        size: num_size,
                        anchor: Anchor::Middle,
                        bold: false,
                        knockout: true,
                    });
                    let half_w = 0.45 * sp * (if fret >= 10 { 2.0 } else { 1.0 });
                    // Legato marks to the next note on the same string.
                    let next = placed.get(k + 1).and_then(|q| {
                        part.bars[q.bar].events[q.event]
                            .notes
                            .iter()
                            .find(|m| m.string == Some(s))
                            .map(|m| (q.x + centre, m.fret.unwrap_or(0)))
                    });
                    if note.has(&Technique::HammerOn) || note.has(&Technique::PullOff) {
                        if let Some((nx, _)) = next {
                            let y0 = y - 0.75 * sp;
                            self.curve((cx, y0), (nx, y0), 1.0 * sp);
                            let l = if note.has(&Technique::HammerOn) {
                                "H"
                            } else {
                                "P"
                            };
                            self.text(
                                (cx + nx) / 2.0,
                                y0 - 0.9 * sp,
                                l,
                                1.1 * sp,
                                Anchor::Middle,
                                false,
                            );
                        }
                    }
                    if note.has(&Technique::Slide) {
                        let (nx, nf) = next.unwrap_or((cx + 3.0 * sp, fret.saturating_sub(2)));
                        let rise = if nf > fret { -0.35 * sp } else { 0.35 * sp };
                        self.line(
                            cx + half_w + 0.2 * sp,
                            y - rise,
                            nx - 0.7 * sp,
                            y + rise,
                            0.12 * sp,
                        );
                    }
                    if let Some(Technique::Bend { semitones, release }) = note
                        .techniques
                        .iter()
                        .find(|t| matches!(t, Technique::Bend { .. }))
                    {
                        let from = (cx + half_w + 0.1 * sp, y - 0.2 * sp);
                        let peak = (cx + 2.0 * sp, rows.span_y + 1.2 * sp);
                        self.items.push(Item::Curve {
                            from,
                            c1: (peak.0, from.1),
                            c2: (peak.0, from.1),
                            to: peak,
                            width: 0.12 * sp,
                        });
                        self.arrow(peak, true);
                        self.text(
                            peak.0,
                            peak.1 - 0.9 * sp,
                            &bend_label(*semitones),
                            1.2 * sp,
                            Anchor::Middle,
                            false,
                        );
                        if *release {
                            let land = (cx + 3.6 * sp, y - 0.6 * sp);
                            self.items.push(Item::Curve {
                                from: peak,
                                c1: (land.0, peak.1),
                                c2: (land.0, peak.1),
                                to: land,
                                width: 0.12 * sp,
                            });
                            self.arrow(land, false);
                        }
                    }
                    if note.has(&Technique::Vibrato) {
                        for i in 0..4 {
                            self.glyph(
                                cx - 0.3 * sp + i as f32 * 0.61 * sp,
                                rows.span_y + 0.4 * sp,
                                g::VIBRATO_SEGMENT,
                            );
                        }
                    }
                    for (t, l) in [
                        (Technique::Tap, "T"),
                        (Technique::Slap, "S"),
                        (Technique::Pop, "P"),
                        (Technique::NaturalHarmonic, "N.H."),
                        (Technique::ArtificialHarmonic, "A.H."),
                    ] {
                        if note.has(&t) && !labels.contains(&l) {
                            labels.push(l);
                        }
                    }
                }
                let mut ly = rows.label_y;
                for l in labels {
                    self.text(cx, ly, l, 1.2 * sp, Anchor::Middle, false);
                    ly -= 1.3 * sp;
                }
                if e.notes.iter().any(|n| n.has(&Technique::Accent)) {
                    self.glyph(p.x, ly, g::ACCENT_ABOVE);
                }
                let tremolo = e.notes.iter().any(|n| n.has(&Technique::TremoloPicking));
                if tremolo && !setup.show_rhythm {
                    self.glyph(cx, rows.label_y - 0.4 * sp, g::TREMOLO_3);
                }
                if setup.show_rhythm && e.duration.value != NoteValue::Whole {
                    let start = bottom + 0.9 * sp;
                    let short = e.duration.value == NoteValue::Half;
                    stems.push(StemNote {
                        x: cx,
                        far: start,
                        near: if short { start + 1.5 * sp } else { start },
                        up: false,
                        duration: e.duration,
                        group: beat_group(times[p.bar], bar_tick(bar, p.event), p.bar),
                        tremolo,
                    });
                    if e.duration.dots > 0 {
                        self.dots(cx + 0.3 * sp, start + 3.0 * sp, 1, e.duration.dots);
                    }
                }
            }
            if setup.show_rhythm && v == voice_ids.first().unwrap_or(&1) {
                self.stems(&stems, 2.8, None);
            }
        }
    }

    fn arrow(&mut self, tip: (f32, f32), up: bool) {
        let sp = self.sp;
        let d = if up { 0.7 * sp } else { -0.7 * sp };
        self.items.push(Item::Polygon {
            points: vec![
                tip,
                (tip.0 - 0.35 * sp, tip.1 + d),
                (tip.0 + 0.35 * sp, tip.1 + d),
            ],
        });
    }
}

struct StemNote {
    x: f32,
    /// The notehead farthest from the stem's end, where the stem starts.
    far: f32,
    /// The notehead nearest the stem's end.
    near: f32,
    up: bool,
    duration: Duration,
    /// Beat group: notes beam together only within one.
    group: (usize, u32),
    tremolo: bool,
}

fn stem_end(near: f32, dir: f32, length: f32, middle: Option<f32>) -> f32 {
    let end = near + dir * length;
    match middle {
        Some(m) if dir < 0.0 => end.min(m),
        Some(m) => end.max(m),
        None => end,
    }
}

fn tab_height(part: &Part, sp: f32) -> f32 {
    match &part.kind {
        PartKind::Tab(s) => (s.tuning.string_count().max(1) - 1) as f32 * TAB_GAP * sp,
        _ => 4.0 * sp,
    }
}

fn bend_label(semitones: f32) -> String {
    let halves = (semitones.max(0.0) * 2.0).round() as u32;
    match halves {
        1 => "¼".into(),
        2 => "½".into(),
        3 => "¾".into(),
        4 => "full".into(),
        n if n % 4 == 0 => format!("{}", n / 4),
        n if n % 4 == 2 => format!("{}½", n / 4),
        n => format!("{:.1}", n as f32 / 4.0),
    }
}

fn notehead_glyph(head: Notehead, value: NoteValue) -> char {
    match head {
        Notehead::Normal => match value {
            NoteValue::Whole => g::NOTEHEAD_WHOLE,
            NoteValue::Half => g::NOTEHEAD_HALF,
            _ => g::NOTEHEAD_BLACK,
        },
        Notehead::X => match value {
            NoteValue::Whole => g::NOTEHEAD_X_WHOLE,
            NoteValue::Half => g::NOTEHEAD_X_HALF,
            _ => g::NOTEHEAD_X_BLACK,
        },
        Notehead::CircleX => g::NOTEHEAD_CIRCLE_X,
        Notehead::Diamond => g::NOTEHEAD_DIAMOND_BLACK,
        Notehead::Slashed => g::NOTEHEAD_SLASHED_BLACK,
    }
}

fn bar_voice_count(bar: &Bar) -> usize {
    let mut v: Vec<u8> = bar.events.iter().map(|e| e.voice).collect();
    v.sort_unstable();
    v.dedup();
    v.len()
}

fn bar_tick(bar: &Bar, event: usize) -> u32 {
    let voice = bar.events[event].voice;
    bar.events[..event]
        .iter()
        .filter(|e| e.voice == voice)
        .map(|e| e.duration.ticks())
        .sum()
}

/// Notes beam within a beat: a quarter in simple time, a dotted quarter in
/// compound time.
fn beat_group(time: TimeSig, tick: u32, bar: usize) -> (usize, u32) {
    let compound = time.beat_type == 8 && time.beats % 3 == 0 && time.beats > 3;
    let beat = if compound {
        TICKS_PER_QUARTER * 3 / 2
    } else {
        TICKS_PER_QUARTER * 4 / u32::from(time.beat_type.max(1))
    };
    (bar, tick / beat.max(1))
}

fn col_x_near(cols: &HashMap<(usize, u32), f32>, bar: usize, tick: u32) -> f32 {
    cols.iter()
        .filter(|((b, t), _)| *b == bar && *t <= tick)
        .max_by_key(|((_, t), _)| *t)
        .map(|(_, x)| *x)
        .unwrap_or(0.0)
}

fn key_signatures(part: &Part) -> Vec<i8> {
    let mut key = 0;
    part.bars
        .iter()
        .map(|b| {
            if let Some(k) = b.key {
                key = k.clamp(-7, 7);
            }
            key
        })
        .collect()
}

const SHARP_ORDER: [i32; 7] = [3, 0, 4, 1, 5, 2, 6];
const FLAT_ORDER: [i32; 7] = [6, 2, 5, 1, 4, 0, 3];

/// Treble-staff places (steps above the bottom line) of a key signature's
/// sharps or flats.
fn key_places(key: i8) -> Vec<i8> {
    const SHARPS: [i8; 7] = [8, 5, 9, 6, 3, 7, 4];
    const FLATS: [i8; 7] = [4, 7, 3, 6, 2, 5, 1];
    let n = usize::from(key.unsigned_abs()).min(7);
    if key > 0 {
        SHARPS[..n].to_vec()
    } else {
        FLATS[..n].to_vec()
    }
}

/// Alteration a key gives each step (C = 0).
fn key_alters(key: i8) -> [i8; 7] {
    let mut out = [0; 7];
    let n = usize::from(key.unsigned_abs()).min(7);
    if key > 0 {
        for &s in &SHARP_ORDER[..n] {
            out[s as usize] = 1;
        }
    } else {
        for &s in &FLAT_ORDER[..n] {
            out[s as usize] = -1;
        }
    }
    out
}

/// Spell a written pitch in a key: (diatonic step number, alteration).
/// Sharp keys and C use sharps, flat keys use flats.
pub(crate) fn spell(pitch: i32, key: i8) -> (i32, i8) {
    const SHARPS: [(i32, i8); 12] = [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (2, 0),
        (3, 0),
        (3, 1),
        (4, 0),
        (4, 1),
        (5, 0),
        (5, 1),
        (6, 0),
    ];
    const FLATS: [(i32, i8); 12] = [
        (0, 0),
        (1, -1),
        (1, 0),
        (2, -1),
        (2, 0),
        (3, 0),
        (4, -1),
        (4, 0),
        (5, -1),
        (5, 0),
        (6, -1),
        (6, 0),
    ];
    let pc = pitch.rem_euclid(12) as usize;
    let octave = pitch.div_euclid(12) - 1;
    let (step, alter) = if key < 0 { FLATS[pc] } else { SHARPS[pc] };
    (octave * 7 + step, alter)
}

/// Which notes in a bar need an accidental drawn, and which: per event,
/// per note, `Some(alter)` (0 = natural).
fn accidentals(bar: &Bar, key: i8, transpose: i32) -> Vec<Vec<Option<i8>>> {
    let base = key_alters(key);
    let mut current: HashMap<i32, i8> = HashMap::new();
    let mut out: Vec<Vec<Option<i8>>> = bar
        .events
        .iter()
        .map(|e| vec![None; e.notes.len()])
        .collect();
    for (_, e) in bar_positions(bar) {
        for (n, note) in bar.events[e].notes.iter().enumerate() {
            let (step, alter) = spell(i32::from(note.pitch) + transpose, key);
            let expected = *current
                .get(&step)
                .unwrap_or(&base[step.rem_euclid(7) as usize]);
            if alter != expected {
                out[e][n] = Some(alter);
                current.insert(step, alter);
            }
        }
    }
    out
}

/// Column widths for a bar, from the length of what starts in each column.
fn plan_bar(part: &Part, bar: &Bar, time: TimeSig, key: i8, sp: f32) -> BarPlan {
    let positions = bar_positions(bar);
    let mut ticks: Vec<u32> = positions.iter().map(|&(t, _)| t).collect();
    ticks.extend(bar.chords.iter().map(|c| c.tick));
    ticks.sort_unstable();
    ticks.dedup();
    if ticks.is_empty() {
        ticks.push(0);
    }
    let transpose = match &part.kind {
        PartKind::Staff { clef } => Some(clef.transpose()),
        PartKind::Tab(s) if s.show_staff => Some(12),
        _ => None,
    };
    let acc = transpose.map(|t| accidentals(bar, key, t));
    let bar_end = time.bar_ticks().max(ticks.last().copied().unwrap_or(0) + 1);
    let widths = ticks
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let next = ticks.get(i + 1).copied().unwrap_or(bar_end);
            let shortest = positions
                .iter()
                .filter(|&&(pt, _)| pt == t)
                .map(|&(_, e)| bar.events[e].duration.ticks())
                .min()
                .unwrap_or(next - t)
                .min(next.saturating_sub(t).max(1));
            let sixteenths = (shortest as f32 / (TICKS_PER_QUARTER as f32 / 4.0)).max(0.25);
            let mut w = (2.4 + 1.3 * sixteenths.log2().max(-1.0)) * sp;
            let here = positions.iter().filter(|&&(pt, _)| pt == t);
            for &(_, e) in here {
                let ev = &bar.events[e];
                if ev.duration.dots > 0 {
                    w += 0.6 * sp;
                }
                if ev.notes.iter().any(|n| n.fret.is_some_and(|f| f >= 10)) {
                    w += 0.5 * sp;
                }
                if ev.strum.is_some() {
                    w += 1.8 * sp;
                }
                if ev
                    .notes
                    .iter()
                    .any(|n| n.has(&Technique::Flam) || n.has(&Technique::Drag))
                {
                    w += 1.6 * sp;
                }
                if let Some(acc) = &acc {
                    if acc[e].iter().any(Option::is_some) {
                        w += 1.2 * sp;
                    }
                }
            }
            for c in bar.chords.iter().filter(|c| c.tick == t) {
                let name_w = c.name.chars().count() as f32 * 1.2 * sp + sp;
                let box_w = c
                    .diagram
                    .as_ref()
                    .map_or(0.0, |d| d.frets.len() as f32 * 0.9 * sp + sp);
                w = w.max(name_w).max(box_w);
            }
            w
        })
        .collect();
    let lead = if bar.time.is_some() {
        1.4 * sp + 2.6 * sp
    } else {
        1.4 * sp
    };
    BarPlan {
        ticks,
        widths,
        lead,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::drums::DrumMap;
    use crate::score::{ChordSymbol, Event, Note};
    use crate::tuning::Tuning;

    fn q() -> Duration {
        Duration::new(NoteValue::Quarter)
    }

    fn glyphs(pages: &[Page]) -> Vec<char> {
        pages
            .iter()
            .flat_map(|p| &p.items)
            .filter_map(|i| match i {
                Item::Glyph { glyph, .. } => Some(*glyph),
                _ => None,
            })
            .collect()
    }

    fn texts(pages: &[Page]) -> Vec<String> {
        pages
            .iter()
            .flat_map(|p| &p.items)
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn tab_score(bars: usize) -> Score {
        let bar = Bar {
            time: None,
            events: vec![
                Event::chord(1, q(), vec![Note::at(64, 1, 0)]),
                Event::chord(1, q(), vec![Note::at(69, 1, 5).with(Technique::HammerOn)]),
                Event::chord(1, q(), vec![Note::at(71, 1, 7)]),
                Event::chord(
                    1,
                    q(),
                    vec![Note::at(62, 2, 3).with(Technique::Bend {
                        semitones: 2.0,
                        release: false,
                    })],
                ),
            ],
            chords: vec![ChordSymbol {
                tick: 0,
                name: "Em".into(),
                diagram: Some(ChordDiagram {
                    frets: vec![Some(0), Some(0), Some(0), Some(2), Some(2), Some(0)],
                    first_fret: 1,
                }),
            }],
            ..Bar::default()
        };
        let mut bars_v = vec![bar; bars];
        bars_v[0].time = Some(TimeSig {
            beats: 4,
            beat_type: 4,
        });
        Score {
            title: "Test".into(),
            composer: String::new(),
            parts: vec![Part {
                name: "Guitar".into(),
                kind: PartKind::Tab(TabSetup {
                    tuning: Tuning::guitar_standard(),
                    capo: 2,
                    show_staff: true,
                    show_rhythm: true,
                }),
                bars: bars_v,
            }],
        }
    }

    #[test]
    fn tab_draws_numbers_marks_and_header() {
        let pages = layout(&tab_score(2), &LayoutOptions::default());
        assert_eq!(pages.len(), 1);
        let t = texts(&pages);
        for want in ["Test", "Guitar", "0", "5", "7", "3", "H", "full", "Em"] {
            assert!(t.iter().any(|s| s == want), "missing {want:?} in {t:?}");
        }
        assert!(t.iter().any(|s| s.contains("Capo 2")));
        let gl = glyphs(&pages);
        assert!(gl.contains(&g::TAB_CLEF_6));
        assert!(gl.contains(&g::G_CLEF_8VB));
        assert!(gl.contains(&g::NOTEHEAD_BLACK));
    }

    #[test]
    fn long_parts_wrap_and_paginate() {
        let pages = layout(&tab_score(60), &LayoutOptions::default());
        assert!(pages.len() > 1, "60 bars should not fit one page");
        for page in &pages {
            for item in &page.items {
                if let Item::Line { from, to, .. } = item {
                    for (x, y) in [from, to] {
                        assert!(*x >= 0.0 && *x <= page.width + 0.01, "x {x} off the page");
                        assert!(*y >= 0.0 && *y <= page.height, "y {y} off the page");
                    }
                }
            }
        }
        assert!(
            texts(&pages)
                .iter()
                .any(|s| s.ends_with(&format!("/ {}", pages.len())))
        );
    }

    #[test]
    fn drums_use_the_map_and_print_a_legend() {
        let e = q();
        let bar = Bar {
            time: Some(TimeSig {
                beats: 4,
                beat_type: 4,
            }),
            events: vec![
                Event::chord(
                    1,
                    e,
                    vec![Note::new(42), Note::new(38).with(Technique::Accent)],
                ),
                Event::chord(1, e, vec![Note::new(46)]),
                Event::chord(
                    1,
                    e,
                    vec![Note::new(42), Note::new(38).with(Technique::Flam)],
                ),
                Event::chord(1, e, vec![Note::new(49).with(Technique::Choke)]),
                Event::chord(2, Duration::new(NoteValue::Half), vec![Note::new(36)]),
                Event::chord(2, Duration::new(NoteValue::Half), vec![Note::new(36)]),
            ],
            ..Bar::default()
        };
        let score = Score {
            title: String::new(),
            composer: String::new(),
            parts: vec![Part {
                name: "Drums".into(),
                kind: PartKind::Drums(DrumMap::general_midi()),
                bars: vec![bar],
            }],
        };
        let pages = layout(&score, &LayoutOptions::default());
        let gl = glyphs(&pages);
        for want in [
            g::PERCUSSION_CLEF,
            g::NOTEHEAD_X_BLACK,
            g::MUTE_OPEN,
            g::ACCENT_ABOVE,
            g::CHOKE_CYMBAL,
            g::NOTEHEAD_HALF,
        ] {
            assert!(gl.contains(&want), "missing {:X}", want as u32);
        }
        let t = texts(&pages);
        for want in [
            "Closed hi-hat",
            "Snare",
            "Bass drum",
            "Crash cymbal",
            "Open hi-hat",
        ] {
            assert!(t.iter().any(|s| s == want), "legend lacks {want}");
        }
    }

    #[test]
    fn spelling_and_accidentals() {
        assert_eq!(spell(61, 0), (4 * 7, 1)); // C#4
        assert_eq!(spell(61, -2), (4 * 7 + 1, -1)); // Db4
        assert_eq!(key_alters(2), [1, 0, 0, 1, 0, 0, 0]); // F# C#
        // In D major, F# needs nothing, F natural needs a natural, and a
        // second F natural in the bar needs nothing more.
        let bar = Bar {
            events: vec![
                Event::chord(1, q(), vec![Note::new(66)]),
                Event::chord(1, q(), vec![Note::new(65)]),
                Event::chord(1, q(), vec![Note::new(65)]),
            ],
            ..Bar::default()
        };
        assert_eq!(
            accidentals(&bar, 2, 0),
            vec![vec![None], vec![Some(0)], vec![None]]
        );
        assert_eq!(bend_label(1.0), "½");
        assert_eq!(bend_label(2.0), "full");
        assert_eq!(bend_label(3.0), "1½");
    }
}
