// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawing laid-out pages. Glyphs and text are both drawn as outlines taken
//! from the fonts, so SVG and PDF output need no installed fonts and look
//! the same everywhere.

pub mod pdf;
pub mod svg;

use ttf_parser::{Face, OutlineBuilder};

use crate::layout::Anchor;

/// The fonts pages are drawn with: a music font in the standard
/// music-font layout, and a text font for fret numbers, names and labels.
pub struct Fonts<'a> {
    music: Face<'a>,
    text: Face<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontError(pub &'static str);

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for FontError {}

/// One step of an outline, in page coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Cubic((f32, f32), (f32, f32), (f32, f32)),
    Close,
}

impl<'a> Fonts<'a> {
    pub fn new(music: &'a [u8], text: &'a [u8]) -> Result<Self, FontError> {
        Ok(Self {
            music: Face::parse(music, 0).map_err(|_| FontError("the music font can't be read"))?,
            text: Face::parse(text, 0).map_err(|_| FontError("the text font can't be read"))?,
        })
    }

    /// Glyphs from [`crate::glyphs::ALL`] the music font lacks.
    pub fn missing_glyphs(&self) -> Vec<char> {
        crate::glyphs::ALL
            .iter()
            .copied()
            .chain((0..10).map(crate::glyphs::time_sig_digit))
            .filter(|&c| self.music.glyph_index(c).is_none())
            .collect()
    }

    /// Outline of a music glyph whose origin is at `at`; `size` is the em.
    pub fn glyph(&self, glyph: char, at: (f32, f32), size: f32) -> Vec<Seg> {
        outline(&self.music, glyph, at, size)
    }

    /// Width of a line of text.
    pub fn text_width(&self, text: &str, size: f32) -> f32 {
        let scale = size / f32::from(self.text.units_per_em());
        text.chars()
            .map(|c| {
                self.text
                    .glyph_index(c)
                    .and_then(|g| self.text.glyph_hor_advance(g))
                    .map_or(0.5 * size, |a| f32::from(a) * scale)
            })
            .sum()
    }

    /// Outlines of a line of text on a baseline, and its left edge.
    pub fn text(&self, text: &str, at: (f32, f32), size: f32, anchor: Anchor) -> (Vec<Seg>, f32) {
        let width = self.text_width(text, size);
        let mut x = match anchor {
            Anchor::Start => at.0,
            Anchor::Middle => at.0 - width / 2.0,
            Anchor::End => at.0 - width,
        };
        let left = x;
        let scale = size / f32::from(self.text.units_per_em());
        let mut out = Vec::new();
        for c in text.chars() {
            out.extend(outline(&self.text, c, (x, at.1), size));
            x += self
                .text
                .glyph_index(c)
                .and_then(|g| self.text.glyph_hor_advance(g))
                .map_or(0.5 * size, |a| f32::from(a) * scale);
        }
        (out, left)
    }
}

fn outline(face: &Face<'_>, c: char, at: (f32, f32), size: f32) -> Vec<Seg> {
    let Some(id) = face.glyph_index(c) else {
        return Vec::new();
    };
    let mut b = Builder {
        at,
        scale: size / f32::from(face.units_per_em()),
        last: (0.0, 0.0),
        out: Vec::new(),
    };
    face.outline_glyph(id, &mut b);
    b.out
}

struct Builder {
    at: (f32, f32),
    scale: f32,
    last: (f32, f32),
    out: Vec<Seg>,
}

impl Builder {
    /// Font units are y-up; pages are y-down.
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.at.0 + x * self.scale, self.at.1 - y * self.scale)
    }
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.last = p;
        self.out.push(Seg::Move(p.0, p.1));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.last = p;
        self.out.push(Seg::Line(p.0, p.1));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let q = self.p(x1, y1);
        let p = self.p(x, y);
        let l = self.last;
        let c1 = (l.0 + 2.0 / 3.0 * (q.0 - l.0), l.1 + 2.0 / 3.0 * (q.1 - l.1));
        let c2 = (p.0 + 2.0 / 3.0 * (q.0 - p.0), p.1 + 2.0 / 3.0 * (q.1 - p.1));
        self.last = p;
        self.out.push(Seg::Cubic(c1, c2, p));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.p(x1, y1);
        let c2 = self.p(x2, y2);
        let p = self.p(x, y);
        self.last = p;
        self.out.push(Seg::Cubic(c1, c2, p));
    }

    fn close(&mut self) {
        self.out.push(Seg::Close);
    }
}

/// The box cleared behind knocked-out text: (x, y, width, height).
pub(crate) fn knockout_box(left: f32, width: f32, baseline: f32, size: f32) -> (f32, f32, f32, f32) {
    let pad = 0.12 * size;
    (left - pad, baseline - 0.8 * size, width + 2.0 * pad, 0.95 * size)
}

/// A circle as four cubic curves.
pub(crate) fn circle(centre: (f32, f32), r: f32) -> Vec<Seg> {
    const K: f32 = 0.552_284_8;
    let (x, y) = centre;
    let k = K * r;
    vec![
        Seg::Move(x + r, y),
        Seg::Cubic((x + r, y + k), (x + k, y + r), (x, y + r)),
        Seg::Cubic((x - k, y + r), (x - r, y + k), (x - r, y)),
        Seg::Cubic((x - r, y - k), (x - k, y - r), (x, y - r)),
        Seg::Cubic((x + k, y - r), (x + r, y - k), (x + r, y)),
        Seg::Close,
    ]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const MUSIC: &[u8] = include_bytes!("../../../app/fonts/bravura/Bravura.otf");
    pub(crate) const TEXT: &[u8] =
        include_bytes!("../../../app/fonts/ibm-plex-sans-condensed/IBMPlexSansCondensed-Regular.ttf");

    #[test]
    fn the_bundled_music_font_has_every_glyph() {
        let fonts = Fonts::new(MUSIC, TEXT).expect("fonts load");
        assert_eq!(fonts.missing_glyphs(), Vec::<char>::new());
        // A black notehead is 1.18 staff spaces wide and one space tall.
        let segs = fonts.glyph(crate::glyphs::NOTEHEAD_BLACK, (100.0, 100.0), 20.0);
        let ys: Vec<f32> = segs
            .iter()
            .filter_map(|s| match s {
                Seg::Move(_, y) | Seg::Line(_, y) | Seg::Cubic(_, _, (_, y)) => Some(*y),
                Seg::Close => None,
            })
            .collect();
        let (lo, hi) = ys.iter().fold((f32::MAX, f32::MIN), |(a, b), &y| (a.min(y), b.max(y)));
        assert!((hi - lo - 5.0).abs() < 0.3, "notehead height {}", hi - lo);
        assert!(fonts.text_width("12", 10.0) > 5.0);
    }
}
