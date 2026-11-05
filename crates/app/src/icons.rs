// SPDX-License-Identifier: GPL-3.0-or-later
//! The app's icons, drawn as vector shapes in our own code (UI polish
//! plan, section 3).
//!
//! They used to be Unicode symbols, which depend on what fonts a computer
//! has and showed as empty boxes when a font lacked them. Drawn icons look
//! the same everywhere, scale with the interface size and take their
//! colour from the theme: a button's text colour, or a token such as
//! `record`.
//!
//! Each icon is drawn in a unit square (0 to 1 across and down) mapped to
//! the rect it is given. [`IconButton`] puts one on an egui button, with
//! or without text; an icon-only button always has a tooltip and an
//! accessible name, so a screen reader and the assistant interface know
//! what it does.

use eframe::egui::{
    self, Atom, Color32, IdSalt, Painter, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2, Widget,
    WidgetText, pos2, vec2,
};

/// Every icon the app draws. Some wait for the polish threads that
/// follow (the edit toolbar, track headers, the top bar).
#[allow(
    dead_code,
    reason = "the set is drawn ahead of the controls that use it"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Icon {
    // Transport.
    Play,
    Stop,
    Pause,
    Record,
    ToStart,
    ToEnd,
    FrameBack,
    FrameForward,
    PrevHit,
    NextHit,
    Loop,
    Metronome,
    // Editing.
    Undo,
    Redo,
    Split,
    Delete,
    Marker,
    Pointer,
    Pencil,
    Snap,
    Ripple,
    ZoomIn,
    ZoomOut,
    Fit,
    // Panels and lists.
    Close,
    Plus,
    Minus,
    Check,
    PopOut,
    Restore,
    FullSize,
    More,
    Expand,
    Collapse,
    /// Chevrons left and right: the previous and next of a list.
    Previous,
    Next,
    /// The small arrow beside a button that opens its options.
    Options,
    MoveUp,
    MoveDown,
    Folder,
    Search,
    Settings,
    Assistant,
    Warning,
    Eye,
    Lock,
    // Tracks and sound.
    Mic,
    Note,
    Speaker,
    Mute,
    Headphones,
    Arm,
    Wave,
    Keys,
}

#[allow(dead_code, reason = "for the icon sheet and tests")]
impl Icon {
    pub(crate) const ALL: [Icon; 54] = [
        Icon::Play,
        Icon::Stop,
        Icon::Pause,
        Icon::Record,
        Icon::ToStart,
        Icon::ToEnd,
        Icon::FrameBack,
        Icon::FrameForward,
        Icon::PrevHit,
        Icon::NextHit,
        Icon::Loop,
        Icon::Metronome,
        Icon::Undo,
        Icon::Redo,
        Icon::Split,
        Icon::Delete,
        Icon::Marker,
        Icon::Pointer,
        Icon::Pencil,
        Icon::Snap,
        Icon::Ripple,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::Fit,
        Icon::Close,
        Icon::Plus,
        Icon::Minus,
        Icon::Check,
        Icon::PopOut,
        Icon::Restore,
        Icon::FullSize,
        Icon::More,
        Icon::Expand,
        Icon::Collapse,
        Icon::Previous,
        Icon::Next,
        Icon::Options,
        Icon::MoveUp,
        Icon::MoveDown,
        Icon::Folder,
        Icon::Search,
        Icon::Settings,
        Icon::Assistant,
        Icon::Warning,
        Icon::Eye,
        Icon::Lock,
        Icon::Mic,
        Icon::Note,
        Icon::Speaker,
        Icon::Mute,
        Icon::Headphones,
        Icon::Arm,
        Icon::Wave,
        Icon::Keys,
    ];

    /// What the icon shows, in words (for the icon sheet and tests).
    pub(crate) fn name(self) -> &'static str {
        match self {
            Icon::Play => "Play",
            Icon::Stop => "Stop",
            Icon::Pause => "Pause",
            Icon::Record => "Record",
            Icon::ToStart => "Back to the start",
            Icon::ToEnd => "To the end",
            Icon::FrameBack => "Back",
            Icon::FrameForward => "Forward",
            Icon::PrevHit => "Previous hit point",
            Icon::NextHit => "Next hit point",
            Icon::Loop => "Loop",
            Icon::Metronome => "Metronome",
            Icon::Undo => "Undo",
            Icon::Redo => "Redo",
            Icon::Split => "Split",
            Icon::Delete => "Delete",
            Icon::Marker => "Marker",
            Icon::Pointer => "Pointer",
            Icon::Pencil => "Draw",
            Icon::Snap => "Snap",
            Icon::Ripple => "Ripple",
            Icon::ZoomIn => "Zoom in",
            Icon::ZoomOut => "Zoom out",
            Icon::Fit => "Fit",
            Icon::Close => "Close",
            Icon::Plus => "Add",
            Icon::Minus => "Remove",
            Icon::Check => "Done",
            Icon::PopOut => "Own window",
            Icon::Restore => "Restore",
            Icon::FullSize => "Full size",
            Icon::More => "More",
            Icon::Expand => "Expand",
            Icon::Collapse => "Collapse",
            Icon::Previous => "Previous",
            Icon::Next => "Next",
            Icon::Options => "Options",
            Icon::MoveUp => "Move up",
            Icon::MoveDown => "Move down",
            Icon::Folder => "Folder",
            Icon::Search => "Search",
            Icon::Settings => "Settings",
            Icon::Assistant => "Assistant",
            Icon::Warning => "Warning",
            Icon::Eye => "Show",
            Icon::Lock => "Lock",
            Icon::Mic => "Microphone",
            Icon::Note => "Note",
            Icon::Speaker => "Speaker",
            Icon::Mute => "Mute",
            Icon::Headphones => "Headphones",
            Icon::Arm => "Arm",
            Icon::Wave => "Audio",
            Icon::Keys => "Instrument",
        }
    }
}

/// Maps the unit square onto a rect, and draws with one colour.
struct Pen<'a> {
    painter: &'a Painter,
    rect: Rect,
    colour: Color32,
    /// Line width, in points.
    width: f32,
}

impl Pen<'_> {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.rect.min + vec2(x, y) * self.rect.width()
    }
    fn pts(&self, pts: &[(f32, f32)]) -> Vec<Pos2> {
        pts.iter().map(|&(x, y)| self.p(x, y)).collect()
    }
    fn stroke(&self) -> Stroke {
        Stroke::new(self.width, self.colour)
    }
    fn fill(&self, pts: &[(f32, f32)]) {
        self.painter.add(Shape::convex_polygon(
            self.pts(pts),
            self.colour,
            Stroke::NONE,
        ));
    }
    fn line(&self, pts: &[(f32, f32)]) {
        self.painter.add(Shape::line(self.pts(pts), self.stroke()));
    }
    fn thick(&self, pts: &[(f32, f32)], times: f32) {
        let s = Stroke::new(self.width * times, self.colour);
        self.painter.add(Shape::line(self.pts(pts), s));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        self.painter
            .add(Shape::closed_line(self.pts(pts), self.stroke()));
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        let r = r * self.rect.width();
        self.painter.circle_filled(self.p(x, y), r, self.colour);
    }
    fn ring(&self, x: f32, y: f32, r: f32) {
        let r = r * self.rect.width();
        self.painter.circle_stroke(self.p(x, y), r, self.stroke());
    }
    fn rect_fill(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32) {
        let r = Rect::from_min_max(self.p(x0, y0), self.p(x1, y1));
        let round = (round * self.rect.width()) as u8;
        self.painter.rect_filled(r, round, self.colour);
    }
    fn rect_line(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32) {
        let r = Rect::from_min_max(self.p(x0, y0), self.p(x1, y1));
        let round = (round * self.rect.width()) as u8;
        self.painter
            .rect_stroke(r, round, self.stroke(), egui::StrokeKind::Middle);
    }
    /// Points on an arc round (x, y), angles in degrees clockwise from
    /// the right (the screen's y runs down).
    fn arc(x: f32, y: f32, r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
        let n = (((to - from).abs() / 12.0).ceil() as usize).max(2);
        (0..=n)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / n as f32).to_radians();
                (x + r * a.cos(), y + r * a.sin())
            })
            .collect()
    }
    /// A filled arrowhead with its tip at (x, y), pointing along `dir`.
    fn head(&self, x: f32, y: f32, dir: Vec2, size: f32) {
        let d = dir.normalized() * size;
        let n = vec2(-d.y, d.x) * 0.7;
        self.fill(&[
            (x, y),
            (x - d.x + n.x, y - d.y + n.y),
            (x - d.x - n.x, y - d.y - n.y),
        ]);
    }
    fn magnifier(&self) {
        self.ring(0.42, 0.42, 0.25);
        self.thick(&[(0.6, 0.6), (0.86, 0.86)], 1.6);
    }
    fn speaker(&self) {
        self.fill(&[
            (0.12, 0.38),
            (0.3, 0.38),
            (0.52, 0.18),
            (0.52, 0.82),
            (0.3, 0.62),
            (0.12, 0.62),
        ]);
    }
}

/// Draws `icon` filling the square `rect` in `colour`.
pub(crate) fn paint(painter: &Painter, rect: Rect, icon: Icon, colour: Color32) {
    let side = rect.width().min(rect.height());
    let rect = Rect::from_center_size(rect.center(), Vec2::splat(side));
    let pen = Pen {
        painter,
        rect,
        colour,
        width: (side * 0.1).max(1.2),
    };
    let arc = Pen::arc;
    match icon {
        Icon::Play => pen.fill(&[(0.26, 0.14), (0.84, 0.5), (0.26, 0.86)]),
        Icon::Stop => pen.rect_fill(0.22, 0.22, 0.78, 0.78, 0.06),
        Icon::Pause => {
            pen.rect_fill(0.24, 0.18, 0.42, 0.82, 0.03);
            pen.rect_fill(0.58, 0.18, 0.76, 0.82, 0.03);
        }
        Icon::Record => pen.dot(0.5, 0.5, 0.32),
        Icon::ToStart => {
            pen.rect_fill(0.18, 0.2, 0.3, 0.8, 0.02);
            pen.fill(&[(0.82, 0.2), (0.32, 0.5), (0.82, 0.8)]);
        }
        Icon::ToEnd => {
            pen.rect_fill(0.7, 0.2, 0.82, 0.8, 0.02);
            pen.fill(&[(0.18, 0.2), (0.68, 0.5), (0.18, 0.8)]);
        }
        Icon::FrameBack => pen.fill(&[(0.7, 0.22), (0.3, 0.5), (0.7, 0.78)]),
        Icon::FrameForward => pen.fill(&[(0.3, 0.22), (0.7, 0.5), (0.3, 0.78)]),
        Icon::PrevHit => {
            pen.fill(&[(0.5, 0.24), (0.12, 0.5), (0.5, 0.76)]);
            pen.fill(&[(0.72, 0.3), (0.9, 0.5), (0.72, 0.7), (0.54, 0.5)]);
        }
        Icon::NextHit => {
            pen.fill(&[(0.28, 0.3), (0.46, 0.5), (0.28, 0.7), (0.1, 0.5)]);
            pen.fill(&[(0.5, 0.24), (0.88, 0.5), (0.5, 0.76)]);
        }
        Icon::Loop => {
            pen.line(&arc(0.5, 0.5, 0.3, 200.0, 330.0));
            pen.head(0.79, 0.37, vec2(0.6, 1.0), 0.16);
            pen.line(&arc(0.5, 0.5, 0.3, 20.0, 150.0));
            pen.head(0.21, 0.63, vec2(-0.6, -1.0), 0.16);
        }
        Icon::Metronome => {
            pen.closed(&[(0.26, 0.86), (0.74, 0.86), (0.6, 0.14), (0.4, 0.14)]);
            pen.line(&[(0.5, 0.7), (0.74, 0.28)]);
            pen.dot(0.5, 0.7, 0.06);
        }
        Icon::Undo => {
            pen.line(&arc(0.54, 0.56, 0.28, 180.0, 400.0));
            pen.head(0.26, 0.74, vec2(0.0, 1.0), 0.2);
        }
        Icon::Redo => {
            pen.line(&arc(0.46, 0.56, 0.28, 360.0, 140.0));
            pen.head(0.74, 0.74, vec2(0.0, 1.0), 0.2);
        }
        Icon::Split => {
            pen.ring(0.3, 0.74, 0.12);
            pen.ring(0.7, 0.74, 0.12);
            pen.line(&[(0.37, 0.64), (0.72, 0.12)]);
            pen.line(&[(0.63, 0.64), (0.28, 0.12)]);
        }
        Icon::Delete => {
            pen.line(&[(0.18, 0.26), (0.82, 0.26)]);
            pen.line(&[(0.4, 0.26), (0.42, 0.14), (0.58, 0.14), (0.6, 0.26)]);
            pen.closed(&[(0.26, 0.26), (0.32, 0.86), (0.68, 0.86), (0.74, 0.26)]);
            pen.line(&[(0.44, 0.4), (0.44, 0.72)]);
            pen.line(&[(0.56, 0.4), (0.56, 0.72)]);
        }
        Icon::Marker => {
            pen.line(&[(0.3, 0.1), (0.3, 0.9)]);
            pen.fill(&[(0.3, 0.14), (0.8, 0.3), (0.3, 0.48)]);
        }
        Icon::Pointer => {
            // Not convex, so in two parts: the head and the tail.
            pen.fill(&[(0.28, 0.1), (0.76, 0.58), (0.28, 0.78)]);
            pen.fill(&[(0.42, 0.6), (0.52, 0.56), (0.66, 0.86), (0.56, 0.9)]);
        }
        Icon::Pencil => {
            pen.fill(&[(0.3, 0.6), (0.66, 0.24), (0.76, 0.34), (0.4, 0.7)]);
            pen.fill(&[(0.7, 0.2), (0.76, 0.14), (0.86, 0.24), (0.8, 0.3)]);
            pen.fill(&[(0.26, 0.64), (0.36, 0.74), (0.16, 0.84)]);
        }
        Icon::Snap => {
            // A magnet: a U, thick, with its two poles marked.
            let mut magnet = vec![(0.24, 0.14), (0.24, 0.5)];
            magnet.extend(arc(0.5, 0.5, 0.26, 180.0, 0.0));
            magnet.push((0.76, 0.14));
            pen.thick(&magnet, 1.5);
            pen.line(&[(0.14, 0.3), (0.34, 0.3)]);
            pen.line(&[(0.66, 0.3), (0.86, 0.3)]);
        }
        Icon::Ripple => {
            // A clip, and the ones after it pushed along.
            pen.rect_fill(0.08, 0.34, 0.3, 0.66, 0.04);
            pen.rect_line(0.42, 0.34, 0.62, 0.66, 0.04);
            pen.line(&[(0.66, 0.5), (0.8, 0.5)]);
            pen.head(0.92, 0.5, vec2(1.0, 0.0), 0.14);
        }
        Icon::ZoomIn => {
            pen.magnifier();
            pen.line(&[(0.3, 0.42), (0.54, 0.42)]);
            pen.line(&[(0.42, 0.3), (0.42, 0.54)]);
        }
        Icon::ZoomOut => {
            pen.magnifier();
            pen.line(&[(0.3, 0.42), (0.54, 0.42)]);
        }
        Icon::Search => pen.magnifier(),
        Icon::Fit => {
            pen.line(&[(0.12, 0.24), (0.12, 0.76)]);
            pen.line(&[(0.88, 0.24), (0.88, 0.76)]);
            pen.line(&[(0.32, 0.5), (0.68, 0.5)]);
            pen.head(0.22, 0.5, vec2(-1.0, 0.0), 0.14);
            pen.head(0.78, 0.5, vec2(1.0, 0.0), 0.14);
        }
        Icon::Close => {
            pen.line(&[(0.26, 0.26), (0.74, 0.74)]);
            pen.line(&[(0.74, 0.26), (0.26, 0.74)]);
        }
        Icon::Plus => {
            pen.line(&[(0.2, 0.5), (0.8, 0.5)]);
            pen.line(&[(0.5, 0.2), (0.5, 0.8)]);
        }
        Icon::Minus => pen.line(&[(0.2, 0.5), (0.8, 0.5)]),
        Icon::Check => pen.line(&[(0.18, 0.52), (0.4, 0.74), (0.82, 0.28)]),
        Icon::PopOut => {
            pen.line(&[
                (0.5, 0.3),
                (0.16, 0.3),
                (0.16, 0.84),
                (0.7, 0.84),
                (0.7, 0.5),
            ]);
            pen.line(&[(0.44, 0.56), (0.78, 0.22)]);
            pen.head(0.86, 0.14, vec2(1.0, -1.0), 0.2);
        }
        Icon::Restore => {
            pen.rect_line(0.16, 0.36, 0.64, 0.84, 0.03);
            pen.line(&[
                (0.36, 0.36),
                (0.36, 0.16),
                (0.84, 0.16),
                (0.84, 0.64),
                (0.64, 0.64),
            ]);
        }
        Icon::FullSize => {
            pen.line(&[(0.16, 0.38), (0.16, 0.16), (0.38, 0.16)]);
            pen.line(&[(0.62, 0.16), (0.84, 0.16), (0.84, 0.38)]);
            pen.line(&[(0.84, 0.62), (0.84, 0.84), (0.62, 0.84)]);
            pen.line(&[(0.38, 0.84), (0.16, 0.84), (0.16, 0.62)]);
        }
        Icon::More => {
            for x in [0.22, 0.5, 0.78] {
                pen.dot(x, 0.5, 0.08);
            }
        }
        Icon::Expand => pen.fill(&[(0.36, 0.26), (0.68, 0.5), (0.36, 0.74)]),
        Icon::Collapse => pen.fill(&[(0.26, 0.36), (0.74, 0.36), (0.5, 0.68)]),
        Icon::Previous => pen.line(&[(0.62, 0.22), (0.34, 0.5), (0.62, 0.78)]),
        Icon::Next => pen.line(&[(0.38, 0.22), (0.66, 0.5), (0.38, 0.78)]),
        Icon::Options => pen.fill(&[(0.32, 0.42), (0.68, 0.42), (0.5, 0.62)]),
        Icon::MoveUp => pen.fill(&[(0.18, 0.7), (0.5, 0.26), (0.82, 0.7)]),
        Icon::MoveDown => pen.fill(&[(0.18, 0.3), (0.82, 0.3), (0.5, 0.74)]),
        Icon::Folder => pen.closed(&[
            (0.12, 0.24),
            (0.4, 0.24),
            (0.48, 0.34),
            (0.88, 0.34),
            (0.88, 0.8),
            (0.12, 0.8),
        ]),
        Icon::Settings => {
            for i in 0..8 {
                let a = (i as f32 * 45.0).to_radians();
                let (s, c) = a.sin_cos();
                pen.thick(
                    &[
                        (0.5 + 0.26 * c, 0.5 + 0.26 * s),
                        (0.5 + 0.4 * c, 0.5 + 0.4 * s),
                    ],
                    1.4,
                );
            }
            pen.ring(0.5, 0.5, 0.25);
            pen.ring(0.5, 0.5, 0.1);
        }
        Icon::Assistant => {
            pen.fill(&[(0.5, 0.08), (0.6, 0.4), (0.5, 0.5), (0.4, 0.4)]);
            pen.fill(&[(0.92, 0.5), (0.6, 0.6), (0.5, 0.5), (0.6, 0.4)]);
            pen.fill(&[(0.5, 0.92), (0.4, 0.6), (0.5, 0.5), (0.6, 0.6)]);
            pen.fill(&[(0.08, 0.5), (0.4, 0.4), (0.5, 0.5), (0.4, 0.6)]);
        }
        Icon::Warning => {
            pen.closed(&[(0.5, 0.12), (0.9, 0.84), (0.1, 0.84)]);
            pen.line(&[(0.5, 0.38), (0.5, 0.6)]);
            pen.dot(0.5, 0.72, 0.05);
        }
        Icon::Eye => {
            let mut lid = arc(0.5, 0.86, 0.5, 217.0, 323.0);
            lid.extend(arc(0.5, 0.14, 0.5, 37.0, 143.0));
            pen.closed(&lid);
            pen.dot(0.5, 0.5, 0.12);
        }
        Icon::Lock => {
            pen.rect_fill(0.22, 0.46, 0.78, 0.86, 0.06);
            pen.line(&{
                let mut s = vec![(0.32, 0.46)];
                s.extend(arc(0.5, 0.34, 0.18, 180.0, 360.0));
                s.push((0.68, 0.46));
                s
            });
        }
        Icon::Mic => {
            pen.rect_line(0.38, 0.12, 0.62, 0.56, 0.12);
            pen.line(&arc(0.5, 0.46, 0.22, 0.0, 180.0));
            pen.line(&[(0.5, 0.68), (0.5, 0.86)]);
            pen.line(&[(0.34, 0.86), (0.66, 0.86)]);
        }
        Icon::Note => {
            let head: Vec<(f32, f32)> = (0..16)
                .map(|i| {
                    let a = (i as f32 * 22.5).to_radians();
                    let (x, y) = (0.15 * a.cos(), 0.1 * a.sin());
                    // Tilted a little, as a printed note head is.
                    let t = -0.35f32;
                    (
                        0.42 + x * t.cos() - y * t.sin(),
                        0.74 + x * t.sin() + y * t.cos(),
                    )
                })
                .collect();
            pen.fill(&head);
            pen.line(&[(0.56, 0.7), (0.56, 0.12)]);
        }
        Icon::Speaker => {
            pen.speaker();
            pen.line(&arc(0.52, 0.5, 0.16, -45.0, 45.0));
            pen.line(&arc(0.52, 0.5, 0.3, -45.0, 45.0));
        }
        Icon::Mute => {
            pen.speaker();
            pen.line(&[(0.64, 0.38), (0.88, 0.62)]);
            pen.line(&[(0.88, 0.38), (0.64, 0.62)]);
        }
        Icon::Headphones => {
            pen.line(&arc(0.5, 0.56, 0.34, 180.0, 360.0));
            pen.rect_fill(0.12, 0.52, 0.3, 0.84, 0.05);
            pen.rect_fill(0.7, 0.52, 0.88, 0.84, 0.05);
        }
        Icon::Arm => {
            pen.ring(0.5, 0.5, 0.32);
            pen.dot(0.5, 0.5, 0.16);
        }
        Icon::Wave => pen.line(&[
            (0.08, 0.5),
            (0.2, 0.5),
            (0.28, 0.3),
            (0.36, 0.72),
            (0.44, 0.14),
            (0.52, 0.86),
            (0.6, 0.24),
            (0.68, 0.64),
            (0.76, 0.42),
            (0.82, 0.5),
            (0.92, 0.5),
        ]),
        Icon::Keys => {
            pen.rect_line(0.1, 0.2, 0.9, 0.8, 0.04);
            for x in [0.3, 0.5, 0.7] {
                pen.line(&[(x, 0.56), (x, 0.8)]);
            }
            for x in [0.3, 0.5, 0.7] {
                pen.rect_fill(x - 0.06, 0.2, x + 0.06, 0.56, 0.0);
            }
        }
    }
}

/// Draws an icon on its own, the size of a line of text, as an image a
/// screen reader names.
pub(crate) fn show(ui: &mut Ui, icon: Icon, name: &str) -> Response {
    let side = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let colour = ui.visuals().text_color();
        paint(ui.painter(), rect.shrink(1.0), icon, colour);
    }
    let name = name.to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Image, true, &name));
    resp
}

/// A button with a drawn icon, and text after it if given. Without
/// text the icon's name is the tooltip and what a screen reader says.
pub(crate) struct IconButton<'a> {
    icon: Icon,
    name: String,
    text: Option<WidgetText>,
    /// The icon after the text rather than before it.
    trailing: bool,
    tip: Option<WidgetText>,
    /// What a screen reader says, if not the text.
    spoken: Option<String>,
    selected: Option<bool>,
    small: bool,
    frame: Option<bool>,
    colour: Option<Color32>,
    min_size: Vec2,
    _life: std::marker::PhantomData<&'a ()>,
}

impl<'a> IconButton<'a> {
    /// An icon-only button. `name` says what it does ("Delete the clip").
    pub(crate) fn new(icon: Icon, name: impl Into<String>) -> Self {
        IconButton {
            icon,
            name: name.into(),
            text: None,
            trailing: false,
            tip: None,
            spoken: None,
            selected: None,
            small: false,
            frame: None,
            colour: None,
            min_size: Vec2::ZERO,
            _life: std::marker::PhantomData,
        }
    }

    /// A button with an icon and text; the text is its accessible name.
    pub(crate) fn labelled(icon: Icon, text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        let mut b = IconButton::new(icon, text.text());
        b.text = Some(text);
        b
    }

    /// Puts the icon after the text.
    pub(crate) fn trailing(mut self) -> Self {
        self.trailing = true;
        self
    }

    /// A tooltip other than the name.
    pub(crate) fn tip(mut self, tip: impl Into<WidgetText>) -> Self {
        self.tip = Some(tip.into());
        self
    }

    /// What a screen reader says for a button with text, when the text
    /// alone is too short to say what it does.
    pub(crate) fn spoken(mut self, name: impl Into<String>) -> Self {
        self.spoken = Some(name.into());
        self
    }

    /// Shown pressed in when `on`, like a selectable label.
    pub(crate) fn selected(mut self, on: bool) -> Self {
        self.selected = Some(on);
        self
    }

    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub(crate) fn frame(mut self, frame: bool) -> Self {
        self.frame = Some(frame);
        self
    }

    /// The icon's colour, if not the button's text colour (a theme
    /// token, such as `record`).
    pub(crate) fn colour(mut self, colour: Color32) -> Self {
        self.colour = Some(colour);
        self
    }

    pub(crate) fn min_size(mut self, size: Vec2) -> Self {
        self.min_size = size;
        self
    }
}

impl<'a> IconButton<'a> {
    /// The egui button, with room for the icon, and the icon's size.
    fn button(&mut self, ui: &Ui) -> (egui::Button<'a>, f32) {
        let side = ui.text_style_height(&egui::TextStyle::Button);
        let atom = Atom::custom(IdSalt::new(ICON_SALT), Vec2::splat(side));
        let mut button = match (self.text.take(), self.trailing) {
            (Some(t), false) => egui::Button::new((atom, t)),
            (Some(t), true) => egui::Button::new((t, atom)),
            (None, _) => egui::Button::new(atom),
        }
        .wrap_mode(egui::TextWrapMode::Extend);
        if let Some(on) = self.selected {
            button = button.selected(on).frame_when_inactive(on).frame(true);
        }
        if self.small {
            button = button.small();
        }
        if let Some(f) = self.frame {
            button = button.frame(f);
        }
        (button.min_size(self.min_size), side)
    }

    fn draw(&self, ui: &Ui, resp: &Response, rect: Rect, side: f32) {
        if ui.is_rect_visible(rect) {
            let visuals = ui
                .style()
                .interact_selectable(resp, self.selected == Some(true));
            // A disabled Ui's painter fades what it draws by itself.
            let colour = self.colour.unwrap_or(visuals.text_color());
            paint(ui.painter(), rect.shrink(side * 0.06), self.icon, colour);
        }
    }

    fn finish(self, resp: Response, has_text: bool) -> Response {
        let resp = match (&self.spoken, has_text) {
            (Some(spoken), _) => resp.accessible_name(spoken.clone()),
            (None, true) => resp,
            (None, false) => resp.accessible_name(self.name.clone()),
        };
        match self.tip {
            Some(tip) => resp.on_hover_text(tip),
            None if !has_text => resp.on_hover_text(self.name),
            None => resp,
        }
    }

    /// The button opens a menu of `content`.
    pub(crate) fn menu<R>(mut self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> Response {
        let has_text = self.text.is_some();
        let (button, side) = self.button(ui);
        let (resp, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, content);
        // The icon's place: centred without text, at the start with it.
        let pad = ui.spacing().button_padding.x;
        let rect = if has_text {
            Rect::from_min_size(
                pos2(resp.rect.left() + pad, resp.rect.center().y - side / 2.0),
                Vec2::splat(side),
            )
        } else {
            Rect::from_center_size(resp.rect.center(), Vec2::splat(side))
        };
        self.draw(ui, &resp, rect, side);
        self.finish(resp, has_text)
    }
}

const ICON_SALT: &str = "daw icon";

impl Widget for IconButton<'_> {
    fn ui(mut self, ui: &mut Ui) -> Response {
        let has_text = self.text.is_some();
        let (button, side) = self.button(ui);
        let out = button.atom_ui(ui);
        if let Some(rect) = out.rect(IdSalt::new(ICON_SALT)) {
            self.draw(ui, &out.response, rect, side);
        }
        self.finish(out.response, has_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon draws something, and stays inside its square.
    #[test]
    fn every_icon_draws_inside_its_square() {
        let square = Rect::from_min_size(pos2(100.0, 100.0), Vec2::splat(40.0));
        for icon in Icon::ALL {
            let ctx = egui::Context::default();
            let mut out = ctx.run_ui(Default::default(), |ui| {
                paint(ui.painter(), square, icon, Color32::WHITE);
            });
            out.textures_delta.clear();
            let drawn: Vec<Rect> = (out.shapes.iter())
                .map(|s| s.shape.visual_bounding_rect())
                .filter(|b| b.is_positive() && square.expand(1.0).intersects(*b))
                .collect();
            assert!(!drawn.is_empty(), "{icon:?} draws nothing");
            for b in drawn {
                assert!(
                    square.expand(3.0).contains_rect(b),
                    "{icon:?} draws outside its square: {b:?}"
                );
            }
        }
    }

    /// Symbols drawn as text depend on the computer's fonts and showed as
    /// boxes; the app draws its icons instead.
    #[test]
    fn no_symbol_icons_are_left_in_the_source() {
        const SYMBOLS: &str = "▶⏹⏺⏮⏭◀▲▼▸▾⏶⏷✂🗑⚑✕✖✔◆■🗁⧉🗗⛶⋯↶↷🎙♩";
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(src).unwrap() {
            let path = entry.unwrap().path();
            // Windows checkouts may have CRLF line ends.
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            let code = text.split("\n#[cfg(test)]\nmod ").next().unwrap_or("");
            for (i, line) in code.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if line.chars().any(|c| SYMBOLS.contains(c)) {
                    found.push(format!("{}:{}", path.display(), i + 1));
                }
            }
        }
        assert!(
            found.is_empty(),
            "draw these with icons::IconButton: {found:?}"
        );
    }

    #[test]
    fn icons_have_distinct_names() {
        let names: std::collections::HashSet<_> = Icon::ALL.iter().map(|i| i.name()).collect();
        assert_eq!(names.len(), Icon::ALL.len());
    }
}

/// The drawn icon buttons in the running app, driven as a person and an
/// assistant would use them together.
#[cfg(test)]
mod in_the_app {
    use egui_kittest::kittest::Queryable as _;
    use serde_json::json;

    use crate::agent::driver::Driver;

    fn clips(d: &mut Driver) -> usize {
        let p = d.project();
        (p["tracks"].as_array().unwrap().iter())
            .map(|t| {
                let n = |k: &str| t[k].as_array().map_or(0, Vec::len);
                n("clips") + n("note_clips")
            })
            .sum()
    }

    fn click(d: &mut Driver, name: &str) {
        d.h.get_by_label(name).click();
        d.h.run();
    }

    /// An assistant sets up a clip; the toolbar's icon buttons split it,
    /// delete it and undo, found by their spoken names; the assistant
    /// sees each change. The transport's icons work in simpler mode too.
    #[test]
    fn the_icon_buttons_do_what_they_say_and_the_assistant_sees_it() {
        // Eight seconds of sound: four bars at 120 BPM.
        let dir = std::env::temp_dir().join(format!("daw-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("eight-seconds.wav");
        let mut d = Driver::new();
        let rate = d.app().rate;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..8 * rate {
            w.write_sample(((i % 100) as i16 - 50) * 10).unwrap();
        }
        w.finalize().unwrap();
        d.call("import_audio", json!({"path": path})).unwrap();
        for _ in 0..500 {
            if clips(&mut d) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(clips(&mut d), 1);
        let c = d.project()["tracks"][0]["clips"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Split at bar 3: two clips.
        d.call("seek", json!({"bar": 3})).unwrap();
        click(&mut d, "Split");
        assert_eq!(clips(&mut d), 2, "{}", d.project());

        // Undo, by its drawn arrow's name.
        click(&mut d, "Undo");
        assert_eq!(clips(&mut d), 1);

        // Delete the selected clip with the bin.
        d.call("select_clips", json!({"clips": [c]})).unwrap();
        click(&mut d, "Delete the selected clips");
        assert_eq!(clips(&mut d), 0);
        click(&mut d, "Undo");
        assert_eq!(clips(&mut d), 1);

        // The flag marks the playhead's bar.
        click(&mut d, "Marker");
        let p = d.project();
        assert_eq!(p["markers"].as_array().map_or(0, Vec::len), 1, "{p}");

        // Simpler mode keeps the drawn transport, named in words.
        d.command("simpler-mode");
        d.h.run();
        for name in ["Play", "Back to the start", "Record"] {
            assert!(d.h.query_by_label(name).is_some(), "{name} is shown");
        }
        click(&mut d, "Record");
        assert!(d.app().recording, "the record button arms recording");
        click(&mut d, "Record");
        assert!(!d.app().recording);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
