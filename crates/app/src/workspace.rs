// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace (roadmap 2.9): the panes of the main window, which the
//! user can put side by side or one above the other, tab together,
//! resize, collapse and show full size. If they turn it on, a pane can
//! also open in a window of its own, on another screen. Named layouts
//! recall an arrangement with a key command each.
//!
//! Layouts belong to this computer, not the project, since people who
//! share a project have different screens: they are kept in `layout.txt`
//! beside the device settings.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::commands::Cmd;
use eframe::egui::{
    self, CursorIcon, DragAndDrop, Id, Pos2, Rect, Sense, Stroke, UiBuilder, ViewportBuilder,
    ViewportCommand, ViewportId, pos2, vec2,
};

/// A part of the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pane {
    Timeline,
    Mixer,
    Comments,
    Notes,
    Lessons,
    Tuner,
    Takes,
    Monitor,
    NotesEditor,
    Automation,
    Browser,
}

pub(crate) const PANES: [Pane; 11] = [
    Pane::Timeline,
    Pane::Mixer,
    Pane::Comments,
    Pane::Notes,
    Pane::Lessons,
    Pane::Tuner,
    Pane::Takes,
    Pane::Monitor,
    Pane::NotesEditor,
    Pane::Automation,
    Pane::Browser,
];

impl Pane {
    /// Kept in `layout.txt`: never change one.
    fn id(self) -> &'static str {
        match self {
            Pane::Timeline => "timeline",
            Pane::Mixer => "mixer",
            Pane::Comments => "comments",
            Pane::Notes => "notes",
            Pane::Lessons => "lessons",
            Pane::Tuner => "tuner",
            Pane::Takes => "takes",
            Pane::Monitor => "monitor",
            Pane::NotesEditor => "notes-editor",
            Pane::Automation => "automation",
            Pane::Browser => "browser",
        }
    }

    fn from_id(id: &str) -> Option<Pane> {
        PANES.into_iter().find(|p| p.id() == id)
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Pane::Timeline => "Timeline",
            Pane::Mixer => "Mixer",
            Pane::Comments => "Comments",
            Pane::Notes => "Lesson notes",
            Pane::Lessons => "Lessons",
            Pane::Tuner => "Tuner",
            Pane::Takes => "Take lanes",
            Pane::Monitor => "Monitor section",
            Pane::NotesEditor => "Note editor",
            Pane::Automation => "Automation",
            Pane::Browser => "Content browser",
        }
    }

    /// As said in a sentence: "the mixer".
    fn the(self) -> String {
        format!("the {}", self.title().to_lowercase())
    }

    /// The timeline is always in the main window, so the window always
    /// has something in it. Every other pane can close or move out.
    fn fixed(self) -> bool {
        self == Pane::Timeline
    }
}

/// Where a pane goes, beside another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    Above,
    Below,
    /// As another tab of the same group.
    Tab,
}

const SIDES: [(Side, &str); 5] = [
    (Side::Left, "Left of"),
    (Side::Right, "Right of"),
    (Side::Above, "Above"),
    (Side::Below, "Below"),
    (Side::Tab, "A tab with"),
];

/// Panes tabbed together, one of them shown.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tabs {
    pub(crate) panes: Vec<Pane>,
    pub(crate) active: usize,
    /// Only its header shows.
    pub(crate) collapsed: bool,
}

impl Tabs {
    fn of(panes: &[Pane]) -> Node {
        Node::Tabs(Tabs {
            panes: panes.to_vec(),
            active: 0,
            collapsed: false,
        })
    }

    fn shown(&self) -> Option<Pane> {
        self.panes.get(self.active).copied()
    }
}

/// The main window's arrangement: groups of tabs, split in two again
/// and again.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    Tabs(Tabs),
    Split {
        /// Side by side, or else one above the other.
        across: bool,
        /// The first one's part of the space, 0 to 1.
        share: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    fn split(across: bool, share: f32, first: Node, second: Node) -> Node {
        Node::Split {
            across,
            share,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn tabs_with(&mut self, p: Pane) -> Option<&mut Tabs> {
        match self {
            Node::Tabs(t) => t.panes.contains(&p).then_some(t),
            Node::Split { first, second, .. } => {
                first.tabs_with(p).or_else(move || second.tabs_with(p))
            }
        }
    }

    fn panes(&self, out: &mut Vec<Pane>) {
        match self {
            Node::Tabs(t) => out.extend(&t.panes),
            Node::Split { first, second, .. } => {
                first.panes(out);
                second.panes(out);
            }
        }
    }

    /// Takes `p` out. True if nothing is left in this node.
    fn remove(&mut self, p: Pane) -> bool {
        match self {
            Node::Tabs(t) => {
                if let Some(i) = t.panes.iter().position(|&q| q == p) {
                    t.panes.remove(i);
                    if t.active > i || t.active >= t.panes.len() {
                        t.active = t.active.saturating_sub(1);
                    }
                }
                t.panes.is_empty()
            }
            Node::Split { first, second, .. } => {
                let left = if first.remove(p) {
                    Some(std::mem::replace(&mut **second, Tabs::of(&[])))
                } else if second.remove(p) {
                    Some(std::mem::replace(&mut **first, Tabs::of(&[])))
                } else {
                    None
                };
                if let Some(n) = left {
                    *self = n;
                }
                false
            }
        }
    }

    /// The node holding `p`, to replace.
    fn node_with(&mut self, p: Pane) -> Option<&mut Node> {
        match self {
            Node::Tabs(t) if t.panes.contains(&p) => Some(self),
            Node::Tabs(_) => None,
            Node::Split { first, second, .. } => {
                first.node_with(p).or_else(move || second.node_with(p))
            }
        }
    }

    /// Grows (or shrinks, if `by` < 0) the pane's part of the split
    /// nearest it. True if there was one.
    fn grow(&mut self, p: Pane, by: f32) -> bool {
        let Node::Split {
            share,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        let in_first = first.tabs_with(p).is_some();
        let inner = if in_first { first } else { second };
        if inner.grow(p, by) {
            return true;
        }
        if !in_first && inner.tabs_with(p).is_none() {
            return false;
        }
        let by = if in_first { by } else { -by };
        *share = (*share + by).clamp(0.1, 0.9);
        true
    }

    /// Only its header shows, so it needs no more space than that.
    fn collapsed(&self) -> bool {
        matches!(self, Node::Tabs(t) if t.collapsed)
    }
}

/// A pane in a window of its own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Window {
    pub(crate) pane: Pane,
    /// Where the user last put it, in points, and the screen scale then;
    /// none if it hasn't opened yet.
    pub(crate) rect: Option<(Rect, f32)>,
}

/// The main window's panes, and the panes in windows of their own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) dock: Node,
    pub(crate) windows: Vec<Window>,
}

impl Layout {
    /// The timeline above the mixer.
    pub(crate) fn recording() -> Layout {
        Layout {
            dock: Node::split(
                false,
                0.62,
                Tabs::of(&[Pane::Timeline]),
                Tabs::of(&[Pane::Mixer]),
            ),
            windows: Vec::new(),
        }
    }

    /// The mixer large, the timeline small above it.
    fn mixing() -> Layout {
        Layout {
            dock: Node::split(
                false,
                0.32,
                Tabs::of(&[Pane::Timeline]),
                Tabs::of(&[Pane::Mixer]),
            ),
            windows: Vec::new(),
        }
    }

    /// The timeline and mixer, with the notes, comments and tuner at
    /// the side.
    fn lesson() -> Layout {
        Layout {
            dock: Node::split(
                true,
                0.66,
                Node::split(
                    false,
                    0.7,
                    Tabs::of(&[Pane::Timeline]),
                    Tabs::of(&[Pane::Mixer]),
                ),
                Node::split(
                    false,
                    0.68,
                    Tabs::of(&[Pane::Notes, Pane::Comments]),
                    Tabs::of(&[Pane::Tuner]),
                ),
            ),
            windows: Vec::new(),
        }
    }

    pub(crate) fn docked(&self, p: Pane) -> bool {
        let mut all = Vec::new();
        self.dock.panes(&mut all);
        all.contains(&p)
    }

    pub(crate) fn in_window(&self, p: Pane) -> bool {
        self.windows.iter().any(|w| w.pane == p)
    }

    pub(crate) fn is_open(&self, p: Pane) -> bool {
        self.docked(p) || self.in_window(p)
    }

    /// Takes the pane out of the main window or its own.
    pub(crate) fn remove(&mut self, p: Pane) {
        if p.fixed() {
            return;
        }
        self.windows.retain(|w| w.pane != p);
        self.dock.remove(p);
    }

    /// Puts `p` beside `target`, moving it from wherever it was.
    pub(crate) fn put(&mut self, p: Pane, target: Pane, side: Side) {
        if p == target {
            return;
        }
        // The fixed pane only moves within the main window.
        self.windows.retain(|w| w.pane != p);
        self.dock.remove(p);
        let target = if self.docked(target) {
            target
        } else {
            Pane::Timeline
        };
        let Some(node) = self.dock.node_with(target) else {
            return;
        };
        if side == Side::Tab {
            if let Node::Tabs(t) = node {
                t.panes.push(p);
                t.active = t.panes.len() - 1;
            }
            return;
        }
        let old = std::mem::replace(node, Tabs::of(&[]));
        let new = Tabs::of(&[p]);
        let across = matches!(side, Side::Left | Side::Right);
        // A pane put beside another takes a third of its space.
        *node = match side {
            Side::Left | Side::Above => Node::split(across, 0.34, new, old),
            _ => Node::split(across, 0.66, old, new),
        };
    }

    /// Opens the pane where it goes by default, unless it is open.
    pub(crate) fn open(&mut self, p: Pane) {
        if self.is_open(p) {
            return;
        }
        let side_panes = [Pane::Notes, Pane::Lessons, Pane::Comments, Pane::Tuner];
        let (target, side) = match p {
            Pane::Timeline => return,
            // Left of the timeline, where sounds are dragged from.
            Pane::Browser => (Pane::Timeline, Side::Left),
            Pane::Mixer | Pane::Takes | Pane::Monitor | Pane::NotesEditor | Pane::Automation => {
                (Pane::Timeline, Side::Below)
            }
            // Below the side panes open now, so all can be seen.
            _ => match side_panes.into_iter().rfind(|&q| q != p && self.docked(q)) {
                Some(q) => (q, Side::Below),
                None => (Pane::Timeline, Side::Right),
            },
        };
        self.put(p, target, side);
    }

    /// Shows the pane's tab, uncollapsed.
    pub(crate) fn show(&mut self, p: Pane) {
        if let Some(t) = self.dock.tabs_with(p) {
            t.active = t.panes.iter().position(|&q| q == p).unwrap_or(0);
            t.collapsed = false;
        }
    }

    pub(crate) fn toggle_collapsed(&mut self, p: Pane) {
        if let Some(t) = self.dock.tabs_with(p) {
            t.collapsed = !t.collapsed;
        }
    }

    pub(crate) fn grow(&mut self, p: Pane, by: f32) {
        self.dock.grow(p, by);
    }

    /// Moves the pane into a window of its own.
    pub(crate) fn detach(&mut self, p: Pane, rect: Option<(Rect, f32)>) {
        if p.fixed() || self.in_window(p) {
            return;
        }
        self.dock.remove(p);
        self.windows.push(Window { pane: p, rect });
    }

    /// Puts a pane in its own window back in the main window.
    pub(crate) fn dock_back(&mut self, p: Pane) {
        if self.in_window(p) {
            self.windows.retain(|w| w.pane != p);
            self.open(p);
        }
    }

    /// Mends a layout read from a file: each pane once, and the
    /// timeline in the main window.
    fn mend(mut self) -> Layout {
        let mut seen = Vec::new();
        mend_node(&mut self.dock, &mut seen);
        if seen.is_empty() {
            self.dock = Tabs::of(&[Pane::Timeline]);
            seen.push(Pane::Timeline);
        } else if !seen.contains(&Pane::Timeline) {
            let old = std::mem::replace(&mut self.dock, Tabs::of(&[]));
            self.dock = Node::split(true, 0.66, Tabs::of(&[Pane::Timeline]), old);
            seen.push(Pane::Timeline);
        }
        self.windows.retain(|w| {
            let ok = !seen.contains(&w.pane);
            seen.push(w.pane);
            ok
        });
        self
    }

    /// One line: `dock <node>`, then `; window <pane> [x y w h scale]`
    /// for each window.
    fn to_text(&self) -> String {
        let mut t = String::from("dock ");
        node_text(&self.dock, &mut t);
        for w in &self.windows {
            t.push_str(" ; window ");
            t.push_str(w.pane.id());
            if let Some((r, s)) = w.rect {
                t.push_str(&format!(
                    " {} {} {} {} {s}",
                    r.min.x,
                    r.min.y,
                    r.width(),
                    r.height()
                ));
            }
        }
        t
    }

    fn from_text(text: &str) -> Option<Layout> {
        let mut parts = text.split(" ; ");
        let mut words = parts.next()?.split_whitespace();
        if words.next()? != "dock" {
            return None;
        }
        let dock = parse_node(&mut words)?;
        let mut windows = Vec::new();
        for part in parts {
            let w: Vec<&str> = part.split_whitespace().collect();
            let (["window", id], rest) = w.split_at(2.min(w.len())) else {
                return None;
            };
            let pane = Pane::from_id(id)?;
            let n: Vec<f32> = rest.iter().filter_map(|x| x.parse().ok()).collect();
            let rect = match n.as_slice() {
                [x, y, w, h, s] if *w > 0.0 && *h > 0.0 && *s > 0.0 => {
                    Some((Rect::from_min_size(pos2(*x, *y), vec2(*w, *h)), *s))
                }
                _ => None,
            };
            windows.push(Window { pane, rect });
        }
        Some(Layout { dock, windows }.mend())
    }
}

fn mend_node(n: &mut Node, seen: &mut Vec<Pane>) {
    match n {
        Node::Tabs(t) => {
            t.panes.retain(|p| {
                let ok = !seen.contains(p);
                seen.push(*p);
                ok
            });
            t.active = t.active.min(t.panes.len().saturating_sub(1));
        }
        Node::Split {
            share,
            first,
            second,
            ..
        } => {
            *share = if share.is_finite() {
                share.clamp(0.1, 0.9)
            } else {
                0.5
            };
            mend_node(first, seen);
            mend_node(second, seen);
            let empty = |n: &Node| matches!(n, Node::Tabs(t) if t.panes.is_empty());
            if empty(first) {
                *n = std::mem::replace(&mut **second, Tabs::of(&[]));
            } else if empty(second) {
                *n = std::mem::replace(&mut **first, Tabs::of(&[]));
            }
        }
    }
}

/// `tabs <active> <collapsed: 0 or 1> <pane,pane>` or
/// `across|stacked <share> ( <node> ) ( <node> )`.
fn node_text(n: &Node, t: &mut String) {
    match n {
        Node::Tabs(tabs) => {
            let ids: Vec<&str> = tabs.panes.iter().map(|p| p.id()).collect();
            t.push_str(&format!(
                "tabs {} {} {}",
                tabs.active,
                u8::from(tabs.collapsed),
                ids.join(",")
            ));
        }
        Node::Split {
            across,
            share,
            first,
            second,
        } => {
            t.push_str(if *across { "across " } else { "stacked " });
            t.push_str(&format!("{share:.3} ( "));
            node_text(first, t);
            t.push_str(" ) ( ");
            node_text(second, t);
            t.push_str(" )");
        }
    }
}

fn parse_node<'a>(w: &mut impl Iterator<Item = &'a str>) -> Option<Node> {
    match w.next()? {
        "tabs" => {
            let active = w.next()?.parse().ok()?;
            let collapsed = w.next()? == "1";
            let panes = w.next()?.split(',').filter_map(Pane::from_id).collect();
            Some(Node::Tabs(Tabs {
                panes,
                active,
                collapsed,
            }))
        }
        kind @ ("across" | "stacked") => {
            let share = w.next()?.parse().ok()?;
            let mut inner = || -> Option<Node> {
                (w.next()? == "(").then_some(())?;
                let n = parse_node(w)?;
                (w.next()? == ")").then_some(n)
            };
            let first = inner()?;
            let second = inner()?;
            Some(Node::split(kind == "across", share, first, second))
        }
        _ => None,
    }
}

/// What the app does for the workspace.
pub(crate) trait Host {
    /// Draws the pane's contents.
    fn pane(&mut self, ui: &mut egui::Ui, pane: Pane);
    /// Runs the key commands pressed in a window of its own.
    fn keys(&mut self, ctx: &egui::Context);
}

/// Layouts that can be named and recalled, by the commands Layout 1 to 9.
pub(crate) const SLOTS: usize = 9;

/// The saved layouts, and the layout now.
pub(crate) struct Workspace {
    pub(crate) layout: Layout,
    pub(crate) saved: Vec<(String, Layout)>,
    /// Panes may open in windows of their own. Off by default.
    pub(crate) windows_allowed: bool,
    /// The pane shown full size, if one is.
    pub(crate) full: Option<Pane>,
    /// The pane last clicked in, which Full size acts on.
    pub(crate) current: Pane,
    /// Where each pane's own window was, to open it there again.
    remembered: Vec<(Pane, Rect, f32)>,
    /// Windows moved onto another screen because theirs was missing,
    /// and where they were moved to.
    moved: Vec<(Pane, Pos2)>,
    /// Where each window was opened, which stays the same while it is
    /// open (the toolkit moves a window whose builder changes).
    opened_at: Vec<(Pane, Option<Pos2>)>,
    screens_checked: Option<Instant>,
    /// Open/closed as the app last had it, for `sync`.
    seen: Vec<(Pane, bool)>,
    /// The Save layout window: the name being typed.
    pub(crate) saving: Option<String>,
    pub(crate) note: Option<String>,
    /// Where the layouts are kept; none in tests.
    path: Option<PathBuf>,
    changed: bool,
}

impl Default for Workspace {
    fn default() -> Workspace {
        Workspace {
            layout: Layout::recording(),
            saved: vec![
                ("Recording".into(), Layout::recording()),
                ("Mixing".into(), Layout::mixing()),
                ("Lesson".into(), Layout::lesson()),
            ],
            windows_allowed: false,
            full: None,
            current: Pane::Timeline,
            remembered: Vec::new(),
            moved: Vec::new(),
            opened_at: Vec::new(),
            screens_checked: None,
            seen: Vec::new(),
            saving: None,
            note: None,
            path: None,
            changed: false,
        }
    }
}

/// How often to look for screens that came or went.
const SCREENS_EVERY: Duration = Duration::from_secs(2);

impl Workspace {
    pub(crate) fn new(path: Option<PathBuf>) -> Workspace {
        let mut w = Workspace::default();
        if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            w.read(&text);
        }
        w.path = path;
        w
    }

    /// `windows on|off`, `layout <layout>`, then `saved <name>\t<layout>`
    /// for each saved layout. Lines it can't read are left out.
    pub(crate) fn to_text(&self) -> String {
        let mut t = format!(
            "windows {}\nlayout {}\n",
            if self.windows_allowed { "on" } else { "off" },
            self.layout.to_text()
        );
        for (name, l) in &self.saved {
            t.push_str(&format!("saved {name}\t{}\n", l.to_text()));
        }
        t
    }

    pub(crate) fn read(&mut self, text: &str) {
        let mut saved = Vec::new();
        for line in text.lines() {
            let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
            match what {
                "windows" => self.windows_allowed = rest.trim() == "on",
                "layout" => {
                    if let Some(l) = Layout::from_text(rest) {
                        self.layout = l;
                    }
                }
                "saved" => {
                    if let Some((name, l)) = rest.split_once('\t')
                        && let Some(l) = Layout::from_text(l)
                        && !name.trim().is_empty()
                        && saved.len() < SLOTS
                    {
                        saved.push((name.trim().to_string(), l));
                    }
                }
                _ => {}
            }
        }
        if text.lines().any(|l| l.starts_with("saved ")) {
            self.saved = saved;
        }
        if !self.windows_allowed {
            self.dock_all();
        }
    }

    fn save(&mut self) {
        self.changed = false;
        let Some(path) = &self.path else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Written whole and renamed, so a crash never leaves half a file.
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, self.to_text())?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            self.note = Some(format!("Couldn't save the layout: {e}"));
        }
    }

    pub(crate) fn changed(&mut self) {
        self.changed = true;
    }

    /// Keeps a pane the app opens and closes itself (the comments, say)
    /// in step: if the app changed `open` since last time, the layout
    /// follows; otherwise `open` follows the layout. True if it changed.
    pub(crate) fn sync(&mut self, p: Pane, open: &mut bool) -> bool {
        let seen = self.seen.iter().any(|&(q, open)| q == p && open);
        if *open != seen {
            if *open {
                self.open(p);
            } else {
                self.close(p);
            }
        }
        let now = self.layout.is_open(p);
        self.seen.retain(|(q, _)| *q != p);
        self.seen.push((p, now));
        let changed = now != *open;
        *open = now;
        changed
    }

    pub(crate) fn open(&mut self, p: Pane) {
        if !self.layout.is_open(p) {
            self.layout.open(p);
            self.changed = true;
        }
        if self.full.is_some_and(|f| f != p) && self.layout.docked(p) {
            self.full = None;
        }
        self.layout.show(p);
        self.current = p;
    }

    pub(crate) fn close(&mut self, p: Pane) {
        if p.fixed() || !self.layout.is_open(p) {
            return;
        }
        self.forget_window(p);
        self.layout.remove(p);
        if self.full == Some(p) {
            self.full = None;
        }
        if self.current == p {
            self.current = Pane::Timeline;
        }
        self.changed = true;
    }

    pub(crate) fn toggle(&mut self, p: Pane) {
        if self.layout.is_open(p) {
            self.close(p);
        } else {
            self.open(p);
        }
    }

    /// Shows the current pane full size, or the layout again.
    pub(crate) fn toggle_full(&mut self) {
        self.full = match self.full {
            Some(_) => None,
            None => self.layout.docked(self.current).then_some(self.current),
        };
    }

    pub(crate) fn detach(&mut self, p: Pane) {
        if !self.windows_allowed {
            return;
        }
        let rect = (self.remembered.iter())
            .find(|r| r.0 == p)
            .map(|r| (r.1, r.2));
        self.layout.detach(p, rect);
        if self.full == Some(p) {
            self.full = None;
        }
        self.changed = true;
    }

    pub(crate) fn dock_back(&mut self, p: Pane) {
        self.forget_window(p);
        self.layout.dock_back(p);
        self.layout.show(p);
        self.changed = true;
    }

    /// Remembers where the pane's window was, for next time.
    fn forget_window(&mut self, p: Pane) {
        if let Some((r, s)) = (self.layout.windows.iter())
            .find(|w| w.pane == p)
            .and_then(|w| w.rect)
        {
            self.remembered.retain(|m| m.0 != p);
            self.remembered.push((p, r, s));
        }
        self.moved.retain(|m| m.0 != p);
        self.opened_at.retain(|m| m.0 != p);
    }

    /// Every pane back in the main window.
    fn dock_all(&mut self) {
        for p in PANES {
            if self.layout.in_window(p) {
                self.dock_back(p);
            }
        }
    }

    pub(crate) fn allow_windows(&mut self, on: bool) {
        self.windows_allowed = on;
        if !on {
            self.dock_all();
        }
        self.changed = true;
    }

    /// Recalls saved layout `n`, counted from 0.
    pub(crate) fn recall(&mut self, n: usize) {
        let Some((name, l)) = self.saved.get(n).cloned() else {
            return;
        };
        self.note = None;
        for p in PANES {
            self.forget_window(p);
        }
        self.layout = l.clone();
        if !self.windows_allowed {
            for w in std::mem::take(&mut self.layout.windows) {
                self.layout.open(w.pane);
            }
            if !l.windows.is_empty() {
                self.note = Some(format!(
                    "“{name}” has panes in their own windows: they are in the main window, \
                     as panes may not open in windows of their own (View menu)."
                ));
            }
        }
        self.full = None;
        if !self.layout.is_open(self.current) {
            self.current = Pane::Timeline;
        }
        self.changed = true;
    }

    /// Saves the layout now under `name`, replacing a layout of that
    /// name. False if every slot is taken.
    pub(crate) fn save_as(&mut self, name: &str) -> bool {
        let name = name.trim().to_string();
        let l = self.layout.clone();
        if let Some(s) = self.saved.iter_mut().find(|s| s.0 == name) {
            s.1 = l;
        } else if self.saved.len() < SLOTS {
            self.saved.push((name, l));
        } else {
            return false;
        }
        self.changed = true;
        true
    }

    /// Draws the main window's panes in `ui`, and the panes in windows
    /// of their own.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, host: &mut impl Host) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let full = self.full.filter(|&p| self.layout.docked(p));
        let drag = DragAndDrop::payload::<Pane>(ui.ctx()).map(|p| *p);
        let mut cx = Draw {
            host,
            acts: Vec::new(),
            drag,
            windows: self.windows_allowed,
            others: {
                let mut all = Vec::new();
                self.layout.dock.panes(&mut all);
                all
            },
            full,
        };
        if let Some(p) = full {
            let mut tabs = Tabs {
                panes: vec![p],
                active: 0,
                collapsed: false,
            };
            cx.tabs(ui, &mut tabs, rect, Id::unique("full pane"));
        } else {
            cx.node(ui, &mut self.layout.dock, rect, Id::unique("panes"));
        }
        let Draw { host, mut acts, .. } = cx;
        let ppp = ui.ctx().pixels_per_point();
        self.windows(ui.ctx(), host, &mut acts, ppp);
        for a in acts {
            self.act(a);
        }
        if self.changed && !ui.ctx().input(|i| i.pointer.any_down()) {
            self.save();
        }
    }

    fn act(&mut self, a: Act) {
        match a {
            Act::Show(p) => {
                self.layout.show(p);
                self.current = p;
                self.changed = true;
            }
            Act::Current(p) => self.current = p,
            Act::Close(p) => self.close(p),
            Act::Collapse(p) => {
                self.layout.toggle_collapsed(p);
                self.changed = true;
            }
            Act::Full(p) => {
                self.current = p;
                self.toggle_full();
            }
            Act::Detach(p) => self.detach(p),
            Act::DockBack(p) => self.dock_back(p),
            Act::Put(p, target, side) => {
                self.forget_window(p);
                self.layout.put(p, target, side);
                self.current = p;
                self.changed = true;
            }
            Act::Grow(p, by) => {
                self.layout.grow(p, by);
                self.changed = true;
            }
            Act::Moved => self.changed = true,
            Act::At(p, r, s) => {
                // Its own place now, not one it was moved to.
                self.moved.retain(|m| m.0 != p);
                if let Some(w) = self.layout.windows.iter_mut().find(|w| w.pane == p) {
                    w.rect = Some((r, s));
                    self.changed = true;
                }
            }
        }
    }

    /// The panes in windows of their own.
    fn windows(
        &mut self,
        ctx: &egui::Context,
        host: &mut impl Host,
        acts: &mut Vec<Act>,
        ppp: f32,
    ) {
        let check = self
            .screens_checked
            .is_none_or(|t| t.elapsed() >= SCREENS_EVERY);
        let screens = if check && !self.layout.windows.is_empty() {
            self.screens_checked = Some(Instant::now());
            Some(screens())
        } else {
            None
        };
        if !self.layout.windows.is_empty() {
            ctx.request_repaint_after(SCREENS_EVERY);
        }
        for w in self.layout.windows.clone() {
            let p = w.pane;
            let id = ViewportId::from_hash_of(("pane window", p.id()));
            let at = match self.opened_at.iter().find(|o| o.0 == p) {
                Some(o) => o.1,
                None => {
                    let at = w.rect.map(|(r, s)| {
                        let screens = screens.clone().unwrap_or_else(self::screens);
                        match placed(r, s, &screens, ppp) {
                            Some(away) => {
                                self.moved.push((p, away));
                                away
                            }
                            None => r.min,
                        }
                    });
                    self.opened_at.push((p, at));
                    at
                }
            };
            // A screen that came or went: off a missing screen, and back
            // when it returns.
            if let (Some(screens), Some((r, s))) = (&screens, w.rect) {
                let away = self.moved.iter().find(|m| m.0 == p).map(|m| m.1);
                match (placed(r, s, screens, ppp), away) {
                    (Some(to), None) => {
                        self.moved.push((p, to));
                        ctx.send_viewport_cmd_to(id, ViewportCommand::OuterPosition(to));
                    }
                    (None, Some(_)) => {
                        self.moved.retain(|m| m.0 != p);
                        ctx.send_viewport_cmd_to(id, ViewportCommand::OuterPosition(r.min));
                    }
                    _ => {}
                }
            }
            let size = w.rect.map_or(vec2(520.0, 420.0), |(r, _)| r.size());
            let mut builder = ViewportBuilder::default()
                .with_title(format!("{} · {}", p.title(), crate::APP_NAME))
                .with_inner_size(size);
            if let Some(at) = at {
                builder = builder.with_position(at);
            }
            let away = self.moved.iter().find(|m| m.0 == p).map(|m| m.1);
            ctx.show_viewport_immediate(id, builder, |ui, _class| {
                let ctx = ui.ctx().clone();
                host.keys(&ctx);
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(p.title());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button("Put back")
                                .accessible_name(format!("Put {} back in the main window", p.the()))
                                .on_hover_text("Back in the main window")
                                .clicked()
                            {
                                acts.push(Act::DockBack(p));
                            }
                        });
                    });
                    ui.separator();
                    host.pane(ui, p);
                });
                let (close, outer, scale) = ctx.input(|i| {
                    let v = i.viewport();
                    (v.close_requested(), v.outer_rect, v.native_pixels_per_point)
                });
                // Closing the window puts the pane back; it never closes
                // the project.
                if close {
                    acts.push(Act::DockBack(p));
                }
                if let Some(outer) = outer {
                    let scale = scale.unwrap_or(1.0);
                    let near = |a: Pos2| a.distance(outer.min) < 8.0;
                    let saved = w.rect.map(|(r, _)| r);
                    let same = saved
                        .is_some_and(|r| near(r.min) && (r.size() - outer.size()).length() < 1.0);
                    let mine = match away {
                        // Where it was put, or not moved there yet: its
                        // own place is still the one saved.
                        Some(a) => !near(a) && !saved.is_some_and(|r| near(r.min)),
                        None => !same,
                    };
                    if mine {
                        acts.push(Act::At(p, outer, scale));
                    }
                }
            });
        }
    }
}

/// Something done in the workspace while drawing it, done after.
enum Act {
    /// Shows a pane's tab.
    Show(Pane),
    /// Clicked in.
    Current(Pane),
    Close(Pane),
    Collapse(Pane),
    Full(Pane),
    Detach(Pane),
    DockBack(Pane),
    Put(Pane, Pane, Side),
    Grow(Pane, f32),
    /// A split was dragged.
    Moved,
    /// Where a pane's own window is now, and its screen's scale.
    At(Pane, Rect, f32),
}

/// The height of a group's tabs.
const HEADER: f32 = 26.0;
/// The gap between panes, which is dragged to resize them.
const GAP: f32 = 6.0;
/// The least a pane shrinks to, when there is room.
const MIN: f32 = 90.0;

struct Draw<'a, H> {
    host: &'a mut H,
    acts: Vec<Act>,
    /// The pane being dragged by its tab.
    drag: Option<Pane>,
    windows: bool,
    /// The panes in the main window, to move one beside.
    others: Vec<Pane>,
    full: Option<Pane>,
}

impl<H: Host> Draw<'_, H> {
    fn node(&mut self, ui: &mut egui::Ui, n: &mut Node, rect: Rect, id: Id) {
        match n {
            Node::Tabs(t) => self.tabs(ui, t, rect, id),
            Node::Split {
                across,
                share,
                first,
                second,
            } => {
                let total = if *across { rect.width() } else { rect.height() };
                let room = (total - GAP).max(0.0);
                let min = MIN.min(room / 2.0);
                let a = match (first.collapsed(), second.collapsed()) {
                    (true, _) => HEADER.min(room),
                    (false, true) => (room - HEADER).max(0.0),
                    _ => (room * *share).clamp(min, room - min),
                };
                let (ra, gap, rb) = if *across {
                    let x = rect.left() + a;
                    (
                        Rect::from_min_max(rect.min, pos2(x, rect.bottom())),
                        Rect::from_min_max(pos2(x, rect.top()), pos2(x + GAP, rect.bottom())),
                        Rect::from_min_max(pos2(x + GAP, rect.top()), rect.max),
                    )
                } else {
                    let y = rect.top() + a;
                    (
                        Rect::from_min_max(rect.min, pos2(rect.right(), y)),
                        Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right(), y + GAP)),
                        Rect::from_min_max(pos2(rect.left(), y + GAP), rect.max),
                    )
                };
                if !first.collapsed() && !second.collapsed() && room > 0.0 {
                    let r = ui.interact(gap, id.with("gap"), Sense::drag());
                    if r.hovered() || r.dragged() {
                        ui.ctx().set_cursor_icon(if *across {
                            CursorIcon::ResizeHorizontal
                        } else {
                            CursorIcon::ResizeVertical
                        });
                        let line = ui.visuals().widgets.active.bg_stroke;
                        ui.painter().rect_filled(gap.shrink(1.0), 1.0, line.color);
                    }
                    if r.dragged() {
                        let d = r.drag_delta();
                        let d = if *across { d.x } else { d.y };
                        *share = ((a + d) / room).clamp(0.1, 0.9);
                        self.acts.push(Act::Moved);
                    }
                }
                self.node(ui, first, ra, id.with(1));
                self.node(ui, second, rb, id.with(2));
            }
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui, t: &mut Tabs, rect: Rect, id: Id) {
        let Some(shown) = t.shown() else {
            return;
        };
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER.min(rect.height())));
        let body = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
        let visuals = ui.visuals().clone();
        ui.painter()
            .rect_filled(header, 2.0, visuals.faint_bg_color);
        let mut hui = ui.new_child(
            UiBuilder::new()
                .max_rect(header.shrink2(vec2(4.0, 1.0)))
                .id_salt(id.with("header"))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        hui.set_clip_rect(header.intersect(ui.clip_rect()));
        if rect.width() < 70.0 {
            // A narrow collapsed group: only the button to open it.
            self.collapse_button(&mut hui, t, shown);
        } else {
            for (i, &p) in t.panes.iter().enumerate() {
                let tab = hui
                    .add(
                        egui::Button::selectable(i == t.active, p.title())
                            .sense(Sense::click_and_drag()),
                    )
                    .accessible_name(format!("{} tab", p.title()))
                    .on_hover_text("Drag to move it beside another pane");
                if tab.clicked() {
                    self.acts.push(Act::Show(p));
                }
                if tab.double_clicked() {
                    self.acts.push(Act::Full(p));
                }
                if tab.drag_started() && self.full.is_none() {
                    DragAndDrop::set_payload(ui.ctx(), p);
                }
            }
            hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.header_buttons(ui, t, shown);
            });
        }
        if !t.collapsed && body.height() > 1.0 {
            let mut bui = ui.new_child(
                UiBuilder::new()
                    .max_rect(body.shrink(4.0))
                    .id_salt(("pane", shown.id())),
            );
            bui.set_clip_rect(body.intersect(ui.clip_rect()));
            self.host.pane(&mut bui, shown);
            let clicked = ui.input(|i| {
                i.pointer.primary_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| body.contains(p))
            });
            if clicked {
                self.acts.push(Act::Current(shown));
            }
        }
        self.drop_zone(ui, t, rect, shown);
    }

    fn collapse_button(&mut self, ui: &mut egui::Ui, t: &Tabs, shown: Pane) {
        let (icon, what) = if t.collapsed {
            ("▸", "Expand")
        } else {
            ("▾", "Collapse")
        };
        if ui
            .small_button(icon)
            .accessible_name(format!("{what} {}", shown.the()))
            .on_hover_text(what)
            .clicked()
        {
            self.acts.push(Act::Collapse(shown));
        }
    }

    /// From the right: close, own window, full size, collapse, and the
    /// menu with the rest.
    fn header_buttons(&mut self, ui: &mut egui::Ui, t: &Tabs, shown: Pane) {
        let the = shown.the();
        if !shown.fixed()
            && ui
                .small_button("✕")
                .accessible_name(format!("Close {}", shown.title().to_lowercase()))
                .on_hover_text("Close")
                .clicked()
        {
            self.acts.push(Act::Close(shown));
        }
        if self.windows
            && !shown.fixed()
            && ui
                .small_button("⧉")
                .accessible_name(format!("Open {the} in its own window"))
                .on_hover_text("Its own window, to put on another screen")
                .clicked()
        {
            self.acts.push(Act::Detach(shown));
        }
        let full = self.full.is_some();
        if ui
            .small_button(if full { "🗗" } else { "⛶" })
            .accessible_name(if full {
                "Back to all panes".to_string()
            } else {
                format!("Show {the} full size")
            })
            .on_hover_text(if full {
                "Back to all panes"
            } else {
                "Full size"
            })
            .clicked()
        {
            self.acts.push(Act::Full(shown));
        }
        if full {
            return;
        }
        self.collapse_button(ui, t, shown);
        ui.menu_button("⋯", |ui| self.menu(ui, shown))
            .response
            .accessible_name(format!("More for {the}"))
            .on_hover_text("Move, resize and more");
    }

    /// The pane's menu: moving it, without dragging.
    fn menu(&mut self, ui: &mut egui::Ui, p: Pane) {
        let others: Vec<Pane> = self.others.iter().copied().filter(|&q| q != p).collect();
        ui.add_enabled_ui(!others.is_empty(), |ui| {
            ui.menu_button("Move it", |ui| {
                for q in others {
                    ui.menu_button(q.title(), |ui| {
                        for (side, words) in SIDES {
                            if ui.button(format!("{words} {}", q.the())).clicked() {
                                self.acts.push(Act::Put(p, q, side));
                                ui.close();
                            }
                        }
                    });
                }
            });
        });
        if ui.button("Larger").clicked() {
            self.acts.push(Act::Grow(p, 0.1));
        }
        if ui.button("Smaller").clicked() {
            self.acts.push(Act::Grow(p, -0.1));
        }
        if self.windows && !p.fixed() && ui.button("Open in its own window").clicked() {
            self.acts.push(Act::Detach(p));
        }
        if !p.fixed() && ui.button("Close").clicked() {
            self.acts.push(Act::Close(p));
        }
    }

    /// While a tab is dragged over this group: where it would go.
    fn drop_zone(&mut self, ui: &mut egui::Ui, t: &Tabs, rect: Rect, shown: Pane) {
        let Some(p) = self.drag else {
            return;
        };
        let Some(at) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        if !rect.contains(at) || (t.panes == [p]) {
            return;
        }
        let side = side_at(rect, at);
        let zone = match side {
            Side::Left => Rect::from_min_max(rect.min, pos2(rect.center().x, rect.bottom())),
            Side::Right => Rect::from_min_max(pos2(rect.center().x, rect.top()), rect.max),
            Side::Above => Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y)),
            Side::Below => Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max),
            Side::Tab => rect,
        };
        let c = ui.visuals().selection.bg_fill;
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::unique("pane drop"),
        ));
        painter.rect(
            zone.shrink(2.0),
            4.0,
            c.gamma_multiply(0.35),
            Stroke::new(2.0, c),
            egui::StrokeKind::Inside,
        );
        let words = SIDES.iter().find(|s| s.0 == side).map_or("", |s| s.1);
        painter.text(
            zone.center(),
            egui::Align2::CENTER_CENTER,
            format!("{words} {}", shown.the()),
            egui::FontId::proportional(16.0),
            crate::theme::look(ui.ctx()).c().text,
        );
        if ui.input(|i| i.pointer.any_released()) {
            self.acts.push(Act::Put(p, shown, side));
            DragAndDrop::clear_payload(ui.ctx());
        }
    }
}

/// Which side of a group the pointer is nearest: its middle makes a
/// tab, its edges put the pane beside.
pub(crate) fn side_at(rect: Rect, at: Pos2) -> Side {
    let x = (at.x - rect.left()) / rect.width().max(1.0);
    let y = (at.y - rect.top()) / rect.height().max(1.0);
    let edges = [
        (x, Side::Left),
        (1.0 - x, Side::Right),
        (y, Side::Above),
        (1.0 - y, Side::Below),
    ];
    let (d, side) = edges
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("four edges");
    if d > 0.3 { Side::Tab } else { side }
}

/// The screens there are, in pixels, the main one first; empty where
/// the system can't say.
fn screens() -> Vec<Rect> {
    #[cfg(windows)]
    {
        win::screens()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Where to put a window saved at `rect` (points, at `scale`) if its
/// screen is missing: onto the main screen, which has `ppp` pixels to
/// the point. None if its own place can still be seen, or if the
/// screens aren't known.
pub(crate) fn placed(rect: Rect, scale: f32, screens: &[Rect], ppp: f32) -> Option<Pos2> {
    let main = *screens.first()?;
    let px = Rect::from_min_max(
        (rect.min.to_vec2() * scale).to_pos2(),
        (rect.max.to_vec2() * scale).to_pos2(),
    );
    // The title bar must be on a screen, enough of it to drag.
    let bar = Rect::from_min_size(px.min, vec2(px.width(), 30.0 * scale));
    let seen = screens.iter().any(|s| {
        let i = s.intersect(bar);
        i.is_positive() && i.width() >= (100.0 * scale).min(bar.width())
    });
    if seen {
        return None;
    }
    let size = px
        .size()
        .min(main.size() - vec2(80.0, 80.0))
        .max(vec2(0.0, 0.0));
    let x = (main.left() + 40.0).min(main.right() - size.x);
    let y = (main.top() + 40.0).min(main.bottom() - size.y);
    Some(pos2(x / ppp, y / ppp))
}

#[cfg(windows)]
mod win {
    use eframe::egui::{Rect, pos2};
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::core::BOOL;

    /// MONITORINFOF_PRIMARY.
    const PRIMARY: u32 = 1;

    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the Vec passed to EnumDisplayMonitors below,
        // which outlives the call.
        let list = unsafe { &mut *(list as *mut Vec<Rect>) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        // SAFETY: `info` is a MONITORINFO with its size set.
        if unsafe { GetMonitorInfoW(m, &mut info) } != 0 {
            let r = info.rcWork;
            let rect = Rect::from_min_max(
                pos2(r.left as f32, r.top as f32),
                pos2(r.right as f32, r.bottom as f32),
            );
            if info.dwFlags & PRIMARY != 0 {
                list.insert(0, rect);
            } else {
                list.push(rect);
            }
        }
        1
    }

    pub(super) fn screens() -> Vec<Rect> {
        let mut list: Vec<Rect> = Vec::new();
        // SAFETY: the callback only uses `list` during the call.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(each),
                &mut list as *mut Vec<Rect> as LPARAM,
            );
        }
        list
    }
}

impl crate::App {
    /// The workspace, drawn in the main window's middle.
    pub(crate) fn panes(&mut self, ui: &mut egui::Ui) {
        let ws = &mut self.workspace;
        let mut changed = ws.sync(Pane::Comments, &mut self.comment_ui.open);
        changed |= ws.sync(Pane::Notes, &mut self.notes_ui.open);
        let mut tuner = self.tuner.open;
        changed |= ws.sync(Pane::Tuner, &mut tuner);
        if let Some(c) = self.tuner.set_open(tuner) {
            self.send(c);
        }
        if changed {
            ui.ctx().request_repaint();
        }
        self.commands.names = (self.workspace.saved.iter().enumerate())
            .map(|(n, (name, _))| (Cmd::Layout(n as u8), format!("Layout {}: {name}", n + 1)))
            .collect();
        let mut ws = std::mem::take(&mut self.workspace);
        self.drawing_panes = true;
        ws.show(ui, self);
        self.drawing_panes = false;
        self.workspace = ws;
        let ctx = ui.ctx().clone();
        for c in std::mem::take(&mut self.workspace_cmds) {
            self.run(&ctx, c);
        }
    }

    /// The View menu.
    pub(crate) fn view_menu(&mut self, ui: &mut egui::Ui) {
        for c in [
            Cmd::Mixer,
            Cmd::MonitorSection,
            Cmd::TakeLanes,
            Cmd::NotesEditor,
            Cmd::AutomationLanes,
            Cmd::Browser,
            Cmd::Comments,
            Cmd::LessonNotes,
            Cmd::Lessons,
            Cmd::Tuner,
        ] {
            let open = match c {
                Cmd::Mixer => self.workspace.layout.is_open(Pane::Mixer),
                Cmd::MonitorSection => self.workspace.layout.is_open(Pane::Monitor),
                Cmd::TakeLanes => self.workspace.layout.is_open(Pane::Takes),
                Cmd::AutomationLanes => self.workspace.layout.is_open(Pane::Automation),
                Cmd::Browser => self.workspace.layout.is_open(Pane::Browser),
                Cmd::Comments => self.comment_ui.open,
                Cmd::LessonNotes => self.notes_ui.open,
                Cmd::Lessons => self.workspace.layout.is_open(Pane::Lessons),
                Cmd::NotesEditor => self.workspace.layout.is_open(Pane::NotesEditor),
                _ => self.tuner.open,
            };
            ui.add_enabled_ui(true, |ui| {
                let name = c.info().name.trim_end_matches('…');
                let keys = self.commands.keymap.text(c);
                let r = ui
                    .add(egui::Button::selectable(open, name).shortcut_text(keys))
                    .on_hover_text(if open { "Close it" } else { "Open it" });
                if r.clicked() {
                    ui.close();
                    let ctx = ui.ctx().clone();
                    match (c, open) {
                        (
                            Cmd::Mixer
                            | Cmd::MonitorSection
                            | Cmd::TakeLanes
                            | Cmd::Lessons
                            | Cmd::NotesEditor
                            | Cmd::Browser,
                            _,
                        ) => self.run(&ctx, c),
                        (Cmd::Comments, true) => self.workspace.close(Pane::Comments),
                        (Cmd::LessonNotes, true) => self.workspace.close(Pane::Notes),
                        (Cmd::Tuner, true) => self.workspace.close(Pane::Tuner),
                        _ => self.run(&ctx, c),
                    }
                }
            });
        }
        self.menu_item(ui, Cmd::FullSize);
        ui.separator();
        for n in 0..self.workspace.saved.len() {
            self.menu_item(ui, Cmd::Layout(n as u8));
        }
        self.menu_item(ui, Cmd::SaveLayout);
        ui.separator();
        let mut on = self.workspace.windows_allowed;
        if ui
            .checkbox(&mut on, Cmd::PaneWindows.info().name)
            .on_hover_text("Off: every pane stays in this window")
            .changed()
        {
            self.workspace.allow_windows(on);
        }
    }

    /// The Save layout window, and the workspace's notes.
    pub(crate) fn layout_windows(&mut self, ctx: &egui::Context) {
        if let Some(n) = self.workspace.note.clone() {
            let mut open = true;
            egui::Window::new("Layout")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(n);
                    if ui.button("OK").clicked() {
                        self.workspace.note = None;
                    }
                });
            if !open {
                self.workspace.note = None;
            }
        }
        let Some(mut name) = self.workspace.saving.take() else {
            return;
        };
        let mut open = true;
        let mut done = false;
        egui::Window::new("Save layout")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label("The panes as they are now, and where their windows are.");
                let field = ui
                    .add(egui::TextEdit::singleline(&mut name).hint_text("Name, such as Mixing"))
                    .accessible_name("Layout name");
                if name.is_empty() && !field.has_focus() {
                    field.request_focus();
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ws = &mut self.workspace;
                let replaces = ws.saved.iter().position(|s| s.0 == name.trim());
                let full = ws.saved.len() >= SLOTS && replaces.is_none();
                ui.weak(match replaces {
                    Some(n) => format!("Replaces layout {} ({}).", n + 1, name.trim()),
                    None if full => {
                        "Nine layouts are saved: remove one, or use one's name to replace it."
                            .into()
                    }
                    None => format!("Becomes layout {}.", ws.saved.len() + 1),
                });
                let ok = !name.trim().is_empty() && !full;
                ui.horizontal(|ui| {
                    if ui.add_enabled(ok, egui::Button::new("Save")).clicked() || (enter && ok) {
                        ws.save_as(&name);
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
                ui.separator();
                let mut remove = None;
                egui::Grid::new("saved layouts")
                    .striped(true)
                    .show(ui, |ui| {
                        for (n, (saved, _)) in ws.saved.iter().enumerate() {
                            ui.label(format!("{}", n + 1));
                            if ui.link(saved).on_hover_text("Use this name").clicked() {
                                name = saved.clone();
                            }
                            ui.weak(self.commands.keymap.text(Cmd::Layout(n as u8)));
                            if ui
                                .small_button("✕")
                                .accessible_name(format!("Remove the layout {saved}"))
                                .on_hover_text("Remove")
                                .clicked()
                            {
                                remove = Some(n);
                            }
                            ui.end_row();
                        }
                    });
                if let Some(n) = remove {
                    ws.saved.remove(n);
                    ws.changed();
                }
            });
        if open && !done {
            self.workspace.saving = Some(name);
        }
    }
}

impl Host for crate::App {
    fn pane(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Timeline => self.timeline(ui),
            Pane::Mixer => {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.mixer(ui));
            }
            Pane::Monitor => {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.monitor_pane(ui));
            }
            Pane::Comments => self.comments_panel(ui),
            Pane::Takes => self.takes_pane(ui),
            Pane::Automation => self.automation_pane(ui),
            Pane::Browser => self.browser_pane(ui),
            Pane::Notes => self.notes_pane(ui),
            Pane::Lessons => self.lessons_pane(ui),
            Pane::NotesEditor => self.notes_editor_pane(ui),
            Pane::Tuner => {
                let inputs = self.input_count();
                if let Some(c) = self.tuner.pane(ui, inputs) {
                    self.send(c);
                }
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        self.command_keys(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes(l: &Layout) -> Vec<Pane> {
        let mut all = Vec::new();
        l.dock.panes(&mut all);
        all
    }

    #[test]
    fn opens_panes_where_they_go() {
        let mut l = Layout::recording();
        l.open(Pane::Comments);
        l.open(Pane::Notes);
        assert_eq!(
            panes(&l),
            [Pane::Timeline, Pane::Comments, Pane::Notes, Pane::Mixer]
        );
        // The notes go below the comments, in the space beside the timeline.
        let Node::Split { first, .. } = &l.dock else {
            panic!()
        };
        let Node::Split {
            across: true,
            second,
            ..
        } = &**first
        else {
            panic!("{first:?}")
        };
        assert!(matches!(&**second, Node::Split { across: false, .. }));
    }

    #[test]
    fn closing_the_last_pane_of_a_split_gives_its_room_back() {
        let mut l = Layout::recording();
        l.remove(Pane::Mixer);
        assert_eq!(l.dock, Tabs::of(&[Pane::Timeline]));
        // The timeline stays.
        l.remove(Pane::Timeline);
        assert_eq!(l.dock, Tabs::of(&[Pane::Timeline]));
    }

    #[test]
    fn moves_a_pane_beside_another() {
        let mut l = Layout::recording();
        l.put(Pane::Mixer, Pane::Timeline, Side::Left);
        match &l.dock {
            Node::Split {
                across: true,
                first,
                second,
                ..
            } => {
                assert_eq!(**first, Tabs::of(&[Pane::Mixer]));
                assert_eq!(**second, Tabs::of(&[Pane::Timeline]));
            }
            n => panic!("{n:?}"),
        }
        l.put(Pane::Mixer, Pane::Timeline, Side::Tab);
        let t = l.dock.tabs_with(Pane::Mixer).unwrap();
        assert_eq!(t.panes, [Pane::Timeline, Pane::Mixer]);
    }

    #[test]
    fn grows_a_pane() {
        let mut l = Layout::recording();
        l.grow(Pane::Mixer, 0.1);
        let Node::Split { share, .. } = l.dock else {
            panic!()
        };
        assert!((share - 0.52).abs() < 1e-4, "{share}");
    }

    #[test]
    fn panes_go_out_to_windows_and_back() {
        let mut l = Layout::recording();
        l.detach(Pane::Mixer, None);
        assert!(l.in_window(Pane::Mixer) && !l.docked(Pane::Mixer));
        // The timeline stays in the main window.
        l.detach(Pane::Timeline, None);
        assert!(l.docked(Pane::Timeline) && !l.in_window(Pane::Timeline));
        l.dock_back(Pane::Mixer);
        assert!(l.docked(Pane::Mixer) && l.windows.is_empty());
    }

    #[test]
    fn layouts_are_kept_as_text() {
        let mut l = Layout::lesson();
        l.toggle_collapsed(Pane::Tuner);
        l.detach(
            Pane::Mixer,
            Some((
                Rect::from_min_size(pos2(1930.0, 40.0), vec2(800.0, 600.0)),
                1.5,
            )),
        );
        l.detach(Pane::Comments, None);
        let text = l.to_text();
        assert_eq!(Layout::from_text(&text), Some(l), "{text}");
    }

    #[test]
    fn mends_a_layout_with_panes_twice_or_none() {
        let l = Layout::from_text("dock tabs 5 0 mixer,mixer,nonsense ; window mixer").unwrap();
        // The timeline is put back, the mixer kept once, in the main window.
        assert_eq!(panes(&l), [Pane::Timeline, Pane::Mixer]);
        assert!(l.windows.is_empty());
        assert_eq!(Layout::from_text("nonsense"), None);
        assert_eq!(
            Layout::from_text("dock across 0.5 ( tabs 0 0 timeline )"),
            None
        );
    }

    #[test]
    fn the_workspace_is_kept_as_text() {
        let mut w = Workspace::default();
        w.allow_windows(true);
        w.open(Pane::Notes);
        w.detach(Pane::Notes);
        assert!(w.save_as("Two screens"));
        let mut read = Workspace::default();
        read.read(&w.to_text());
        assert!(read.windows_allowed);
        assert_eq!(read.layout, w.layout);
        assert_eq!(read.saved, w.saved);
        assert_eq!(read.saved.len(), 4);
    }

    #[test]
    fn windows_come_back_into_the_main_window_when_not_allowed() {
        let mut w = Workspace::default();
        w.allow_windows(true);
        w.detach(Pane::Mixer);
        assert!(w.layout.in_window(Pane::Mixer));
        assert!(w.save_as("Recording"));
        w.allow_windows(false);
        assert!(w.layout.docked(Pane::Mixer));
        // A saved layout with a window opens it in the main window.
        w.recall(0);
        assert!(w.layout.docked(Pane::Mixer));
        assert!(w.note.is_some());
        // So does a file written with them allowed, once turned off.
        let mut read = Workspace::default();
        read.read("windows off\nlayout dock tabs 0 0 timeline ; window tuner\n");
        assert!(read.layout.docked(Pane::Tuner));
    }

    #[test]
    fn detaching_needs_windows_allowed() {
        let mut w = Workspace::default();
        w.detach(Pane::Mixer);
        assert!(w.layout.docked(Pane::Mixer));
    }

    #[test]
    fn sync_follows_whichever_changed() {
        let mut w = Workspace::default();
        let mut open = false;
        w.sync(Pane::Comments, &mut open);
        assert!(!w.layout.is_open(Pane::Comments));
        // The app opened it.
        open = true;
        w.sync(Pane::Comments, &mut open);
        assert!(w.layout.is_open(Pane::Comments) && open);
        // The user closed it in the workspace.
        w.close(Pane::Comments);
        assert!(w.sync(Pane::Comments, &mut open));
        assert!(!open);
    }

    #[test]
    fn recalls_saved_layouts() {
        let mut w = Workspace::default();
        w.recall(2);
        assert!(w.layout.is_open(Pane::Tuner));
        w.recall(0);
        assert!(!w.layout.is_open(Pane::Tuner));
        // Nine at most.
        for i in 0..SLOTS {
            w.save_as(&format!("Layout {i}"));
        }
        assert_eq!(w.saved.len(), SLOTS);
        assert!(!w.save_as("One more"));
        assert!(w.save_as("Mixing"));
    }

    #[test]
    fn the_pointer_picks_a_side() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        assert_eq!(side_at(r, pos2(50.0, 50.0)), Side::Tab);
        assert_eq!(side_at(r, pos2(5.0, 50.0)), Side::Left);
        assert_eq!(side_at(r, pos2(95.0, 40.0)), Side::Right);
        assert_eq!(side_at(r, pos2(50.0, 2.0)), Side::Above);
        assert_eq!(side_at(r, pos2(50.0, 99.0)), Side::Below);
    }

    #[test]
    fn a_window_on_a_missing_screen_comes_onto_the_main_one() {
        let main = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1040.0));
        let right = Rect::from_min_size(pos2(1920.0, 0.0), vec2(2560.0, 1400.0));
        let on_right = Rect::from_min_size(pos2(2000.0, 100.0), vec2(800.0, 600.0));
        assert_eq!(placed(on_right, 1.0, &[main, right], 1.0), None);
        let to = placed(on_right, 1.0, &[main], 1.0).unwrap();
        assert_eq!(to, pos2(40.0, 40.0));
        // In points, at the main screen's scale.
        let to = placed(on_right, 1.0, &[main], 2.0).unwrap();
        assert_eq!(to, pos2(20.0, 20.0));
        // Scale counts: 1100 points at 2x is on the right screen.
        let r = Rect::from_min_size(pos2(1100.0, 50.0), vec2(400.0, 300.0));
        assert_eq!(placed(r, 2.0, &[main, right], 1.0), None);
        assert!(placed(r, 2.0, &[main], 1.0).is_some());
        // With the screens unknown, it stays where it was.
        assert_eq!(placed(on_right, 1.0, &[], 1.0), None);
    }

    mod in_the_app {
        use eframe::egui::{Key, Modifiers};
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;

        use super::super::*;
        use crate::App;

        fn harness() -> Harness<'static, App> {
            let mut h = Harness::builder()
                .with_size([1400.0, 900.0])
                .allow_missing_glyphs()
                .build_eframe(|_cc| App::new(Vec::new()));
            h.run();
            h
        }

        #[test]
        fn panes_close_and_open_again() {
            let mut h = harness();
            assert!(h.query_by_label("Mixer tab").is_some());
            // The timeline can't be closed.
            assert!(h.query_by_label("Close timeline").is_none());
            h.get_by_label("Close mixer").click();
            h.run();
            assert!(!h.state().workspace.layout.is_open(Pane::Mixer));
            assert!(h.query_by_label("Mixer tab").is_none());
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::Mixer);
            h.run();
            assert!(h.query_by_label("Mixer tab").is_some());
        }

        #[test]
        fn panes_the_app_opens_follow_it() {
            let mut h = harness();
            h.state_mut().comment_ui.open = true;
            h.run();
            assert!(h.state().workspace.layout.docked(Pane::Comments));
            h.get_by_label("Close comments").click();
            h.run();
            h.run();
            assert!(!h.state().comment_ui.open);
            // The tuner listens only while it is open.
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::Tuner);
            h.run();
            assert!(h.state().workspace.layout.docked(Pane::Tuner));
            h.get_by_label("Close tuner").click();
            h.run();
            h.run();
            assert!(!h.state().tuner.open);
        }

        #[test]
        fn own_windows_are_off_until_turned_on() {
            let mut h = harness();
            assert!(
                h.query_by_label("Open the mixer in its own window")
                    .is_none()
            );
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::PaneWindows);
            h.run();
            h.get_by_label("Open the mixer in its own window").click();
            h.run();
            h.run();
            assert!(h.state().workspace.layout.in_window(Pane::Mixer));
            // Its window has its controls, and a way back.
            assert!(
                h.query_by_label("Or add an empty track to record on")
                    .is_some()
            );
            h.get_by_label("Put the mixer back in the main window")
                .click();
            h.run();
            h.run();
            assert!(h.state().workspace.layout.docked(Pane::Mixer));
            // Turning it off brings every window back.
            h.state_mut().workspace.detach(Pane::Mixer);
            h.state_mut().run(&ctx, Cmd::PaneWindows);
            h.run();
            assert!(h.state().workspace.layout.docked(Pane::Mixer));
        }

        #[test]
        fn a_key_recalls_a_layout() {
            let mut h = harness();
            h.key_press_modifiers(Modifiers::COMMAND, Key::Num3);
            h.run();
            h.run();
            assert!(h.state().workspace.layout.docked(Pane::Notes));
            assert!(h.state().notes_ui.open && h.state().tuner.open);
            assert!(h.query_by_label("Lesson notes tab").is_some());
            h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
            h.run();
            h.run();
            assert!(!h.state().notes_ui.open && !h.state().tuner.open);
        }

        #[test]
        fn a_tab_dragged_to_an_edge_goes_beside() {
            let mut h = harness();
            let from = h.get_by_label("Mixer tab").rect().center();
            // The timeline's group: from the top to the gap above the mixer.
            let timeline = h.get_by_label("Timeline tab").rect();
            let to = pos2(1390.0, timeline.top() + 200.0);
            h.hover_at(from);
            h.run();
            h.drag_at(from);
            h.run();
            for i in 1..=10 {
                h.hover_at(from + (to - from) * (i as f32 / 10.0));
                h.run();
            }
            h.drop_at(to);
            h.run();
            h.run();
            match &h.state().workspace.layout.dock {
                Node::Split {
                    across: true,
                    first,
                    second,
                    ..
                } => {
                    assert_eq!(**first, Tabs::of(&[Pane::Timeline]));
                    assert_eq!(**second, Tabs::of(&[Pane::Mixer]));
                }
                n => panic!("{n:?}"),
            }
        }

        #[test]
        fn full_size_shows_one_pane() {
            let mut h = harness();
            h.get_by_label("Show the mixer full size").click();
            h.run();
            assert_eq!(h.state().workspace.full, Some(Pane::Mixer));
            assert!(h.query_by_label("Timeline tab").is_none());
            h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::F);
            h.run();
            assert_eq!(h.state().workspace.full, None);
            assert!(h.query_by_label("Timeline tab").is_some());
        }

        #[test]
        fn layouts_are_named_in_the_command_search() {
            let mut h = harness();
            let ctx = h.ctx.clone();
            h.state_mut().run(&ctx, Cmd::FindCommand);
            h.run();
            let names = &h.state().commands.names;
            let found = crate::commands::search_named("lesson", names);
            assert!(found.contains(&Cmd::Layout(2)));
            assert!(h.query_by_label("Layout 1: Recording").is_some());
        }
    }
}
