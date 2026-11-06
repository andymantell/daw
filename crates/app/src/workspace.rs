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

use crate::icons::{Icon, IconButton};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::commands::Cmd;
use eframe::egui::{
    self, Color32, CursorIcon, DragAndDrop, Id, Pos2, Rect, Sense, Stroke, UiBuilder,
    ViewportBuilder, ViewportCommand, ViewportId, pos2, vec2,
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
    Patterns,
    Video,
    Fretboard,
    PitchEditor,
    Inspector,
    /// The shown track's instrument.
    Instrument,
    /// The shown track's (or the master's) effect chain.
    Effects,
    /// How the shown track's notation lane draws its notes.
    Notation,
    /// What a lesson needs in one place: tuner, speed and pitch, the
    /// loop and trainer, this week's assignments, lesson notes.
    Practice,
}

pub(crate) const PANES: [Pane; 20] = [
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
    Pane::Patterns,
    Pane::Video,
    Pane::Fretboard,
    Pane::PitchEditor,
    Pane::Inspector,
    Pane::Instrument,
    Pane::Effects,
    Pane::Notation,
    Pane::Practice,
];

impl Pane {
    /// Kept in `layout.txt`: never change one.
    pub(crate) fn id(self) -> &'static str {
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
            Pane::Patterns => "patterns",
            Pane::Video => "video",
            Pane::Fretboard => "fretboard",
            Pane::PitchEditor => "pitch-editor",
            Pane::Inspector => "inspector",
            Pane::Instrument => "instrument",
            Pane::Effects => "effects",
            Pane::Notation => "notation",
            Pane::Practice => "practice",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Pane> {
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
            Pane::Patterns => "Pattern editor",
            Pane::Video => "Video",
            Pane::Fretboard => "Fretboard",
            Pane::PitchEditor => "Pitch editor",
            Pane::Inspector => "Inspector",
            Pane::Instrument => "Instrument",
            Pane::Effects => "Effects",
            Pane::Notation => "Notation",
            Pane::Practice => "Practice",
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

/// A fixed part of the main window around the timeline, which holds
/// panes as tabs (UI polish plan, section 1): the browser's panel on the
/// left, the inspector's on the right, and the dock along the bottom for
/// the editors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Region {
    Left,
    Right,
    Dock,
}

pub(crate) const REGIONS: [Region; 3] = [Region::Left, Region::Right, Region::Dock];

impl Region {
    /// Kept in `layout.txt` and given to assistants: never change one.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Region::Left => "left",
            Region::Right => "right",
            Region::Dock => "dock",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Region> {
        REGIONS.into_iter().find(|r| r.id() == id)
    }

    /// As said in a sentence: "the left panel".
    pub(crate) fn the(self) -> &'static str {
        match self {
            Region::Left => "the left panel",
            Region::Right => "the right panel",
            Region::Dock => "the dock",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

impl Pane {
    /// Where the pane opens: the browser on the left, the inspector and
    /// the lesson's panes on the right, the editors in the dock.
    pub(crate) fn home(self) -> Region {
        match self {
            Pane::Browser => Region::Left,
            Pane::Inspector
            | Pane::Practice
            | Pane::Notes
            | Pane::Comments
            | Pane::Lessons
            | Pane::Tuner => Region::Right,
            _ => Region::Dock,
        }
    }
}

/// Panes tabbed together, one of them shown.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tabs {
    pub(crate) panes: Vec<Pane>,
    pub(crate) active: usize,
    /// Only its header shows.
    pub(crate) collapsed: bool,
}

impl Tabs {
    fn of(panes: &[Pane]) -> Tabs {
        Tabs {
            panes: panes.to_vec(),
            active: 0,
            collapsed: false,
        }
    }

    fn shown(&self) -> Option<Pane> {
        self.panes.get(self.active).copied()
    }

    fn remove(&mut self, p: Pane) -> bool {
        let Some(i) = self.panes.iter().position(|&q| q == p) else {
            return false;
        };
        self.panes.remove(i);
        if self.active > i || self.active >= self.panes.len() {
            self.active = self.active.saturating_sub(1);
        }
        if self.panes.is_empty() {
            self.collapsed = false;
        }
        true
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

/// The side panels' widths, as parts of the main window's width.
const LEFT_SHARE: f32 = 0.2;
const RIGHT_SHARE: f32 = 0.17;
/// The dock's part of the height beside the side panels: enough for
/// an instrument's panel at 1440 x 900.
const DOCK_SHARE: f32 = 0.42;
/// The most a side panel or the dock may take.
const MAX_SHARE: f32 = 0.7;
const MIN_SHARE: f32 = 0.08;

/// The main window: the timeline in the middle, with the three regions
/// round it; and the panes in windows of their own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    /// By `Region::index`.
    pub(crate) regions: [Tabs; 3],
    /// Each region's part of the window: of its width for the side
    /// panels, of the height for the dock.
    pub(crate) shares: [f32; 3],
    pub(crate) windows: Vec<Window>,
}

impl Layout {
    fn with(left: &[Pane], right: &[Pane], dock: &[Pane], dock_share: f32) -> Layout {
        Layout {
            regions: [Tabs::of(left), Tabs::of(right), Tabs::of(dock)],
            shares: [LEFT_SHARE, RIGHT_SHARE, dock_share],
            windows: Vec::new(),
        }
    }

    /// The timeline above the mixer, the inspector on the right.
    pub(crate) fn recording() -> Layout {
        Layout::with(&[], &[Pane::Inspector], &[Pane::Mixer], DOCK_SHARE)
    }

    /// The mixer large, the timeline small above it.
    pub(crate) fn mixing() -> Layout {
        Layout::with(&[], &[], &[Pane::Mixer], 0.68)
    }

    /// The timeline and mixer, with the practice panel, notes and
    /// comments at the side.
    fn lesson() -> Layout {
        let mut l = Layout::with(
            &[],
            &[Pane::Practice, Pane::Notes, Pane::Comments],
            &[Pane::Mixer],
            0.3,
        );
        l.shares[Region::Right.index()] = 0.3;
        l
    }

    pub(crate) fn tabs(&self, r: Region) -> &Tabs {
        &self.regions[r.index()]
    }

    fn tabs_mut(&mut self, r: Region) -> &mut Tabs {
        &mut self.regions[r.index()]
    }

    /// The region the pane is in, if it is in one.
    pub(crate) fn region_of(&self, p: Pane) -> Option<Region> {
        REGIONS
            .into_iter()
            .find(|&r| self.tabs(r).panes.contains(&p))
    }

    /// In the main window.
    pub(crate) fn docked(&self, p: Pane) -> bool {
        p.fixed() || self.region_of(p).is_some()
    }

    pub(crate) fn in_window(&self, p: Pane) -> bool {
        self.windows.iter().any(|w| w.pane == p)
    }

    pub(crate) fn is_open(&self, p: Pane) -> bool {
        self.docked(p) || self.in_window(p)
    }

    /// The panes in the main window, the timeline first.
    #[cfg(test)]
    fn panes(&self) -> Vec<Pane> {
        let mut all = vec![Pane::Timeline];
        for t in &self.regions {
            all.extend(&t.panes);
        }
        all
    }

    /// Takes the pane out of the main window or its own.
    pub(crate) fn remove(&mut self, p: Pane) {
        if p.fixed() {
            return;
        }
        self.windows.retain(|w| w.pane != p);
        for t in &mut self.regions {
            t.remove(p);
        }
    }

    /// Puts `p` in a region as its last tab, shown, moving it from
    /// wherever it was.
    pub(crate) fn put(&mut self, p: Pane, r: Region) {
        if p.fixed() || self.region_of(p) == Some(r) {
            return;
        }
        self.remove(p);
        let t = self.tabs_mut(r);
        t.panes.push(p);
        t.active = t.panes.len() - 1;
    }

    /// Opens the pane where it goes, unless it is open.
    pub(crate) fn open(&mut self, p: Pane) {
        if !self.is_open(p) {
            self.put(p, p.home());
        }
    }

    /// Shows the pane's tab, and its region if collapsed.
    pub(crate) fn show(&mut self, p: Pane) {
        if let Some(r) = self.region_of(p) {
            let t = self.tabs_mut(r);
            t.active = t.panes.iter().position(|&q| q == p).unwrap_or(0);
            t.collapsed = false;
        }
    }

    pub(crate) fn toggle_collapsed(&mut self, p: Pane) {
        if let Some(r) = self.region_of(p) {
            let t = self.tabs_mut(r);
            t.collapsed = !t.collapsed;
        }
    }

    /// Gives the pane's region more room (or less); for the timeline,
    /// the dock less (or more).
    pub(crate) fn grow(&mut self, p: Pane, by: f32) {
        let (r, by) = match self.region_of(p) {
            Some(r) => (r, by),
            None if p.fixed() => (Region::Dock, -by),
            None => return,
        };
        let s = &mut self.shares[r.index()];
        *s = (*s + by).clamp(MIN_SHARE, MAX_SHARE);
    }

    /// Moves the pane into a window of its own.
    pub(crate) fn detach(&mut self, p: Pane, rect: Option<(Rect, f32)>) {
        if p.fixed() || self.in_window(p) {
            return;
        }
        self.remove(p);
        self.windows.push(Window { pane: p, rect });
    }

    /// Puts a pane in its own window back in the main window.
    pub(crate) fn dock_back(&mut self, p: Pane) {
        if self.in_window(p) {
            self.windows.retain(|w| w.pane != p);
            self.open(p);
        }
    }

    /// Mends a layout read from a file: each pane once, the timeline in
    /// the middle, and the sizes in range.
    fn mend(mut self) -> Layout {
        let mut seen = vec![Pane::Timeline];
        for t in &mut self.regions {
            t.panes.retain(|p| {
                let ok = !seen.contains(p);
                seen.push(*p);
                ok
            });
            t.active = t.active.min(t.panes.len().saturating_sub(1));
        }
        for s in &mut self.shares {
            *s = if s.is_finite() {
                s.clamp(MIN_SHARE, MAX_SHARE)
            } else {
                0.25
            };
        }
        self.windows.retain(|w| {
            let ok = !seen.contains(&w.pane);
            seen.push(w.pane);
            ok
        });
        self
    }

    /// `panels <left> <right> <dock>` (their shares), then
    /// `; <region> <active> <collapsed: 0 or 1> <pane,pane or ->` for each
    /// region and `; window <pane> [x y w h scale]` for each window.
    fn to_text(&self) -> String {
        let [l, r, d] = self.shares;
        let mut t = format!("panels {l:.3} {r:.3} {d:.3}");
        for reg in REGIONS {
            let tabs = self.tabs(reg);
            let ids: Vec<&str> = tabs.panes.iter().map(|p| p.id()).collect();
            t.push_str(&format!(
                " ; {} {} {} {}",
                reg.id(),
                tabs.active,
                u8::from(tabs.collapsed),
                if ids.is_empty() {
                    "-".to_string()
                } else {
                    ids.join(",")
                }
            ));
        }
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
        let first = parts.next()?;
        let mut words = first.split_whitespace();
        let mut l = match words.next()? {
            "panels" => {
                let shares: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                let [a, b, c] = shares.as_slice() else {
                    return None;
                };
                Layout {
                    regions: [Tabs::of(&[]), Tabs::of(&[]), Tabs::of(&[])],
                    shares: [*a, *b, *c],
                    windows: Vec::new(),
                }
            }
            // Written before the regions: panes split any way.
            "dock" => from_tree(&parse_node(&mut words)?),
            _ => return None,
        };
        for part in parts {
            let w: Vec<&str> = part.split_whitespace().collect();
            match w.as_slice() {
                ["window", id, rest @ ..] => {
                    let pane = Pane::from_id(id)?;
                    let n: Vec<f32> = rest.iter().filter_map(|x| x.parse().ok()).collect();
                    let rect = match n.as_slice() {
                        [x, y, w, h, s] if *w > 0.0 && *h > 0.0 && *s > 0.0 => {
                            Some((Rect::from_min_size(pos2(*x, *y), vec2(*w, *h)), *s))
                        }
                        _ => None,
                    };
                    l.windows.push(Window { pane, rect });
                }
                [region, active, collapsed, ids] => {
                    let r = Region::from_id(region)?;
                    let t = l.tabs_mut(r);
                    t.panes = ids.split(',').filter_map(Pane::from_id).collect();
                    t.active = active.parse().ok()?;
                    t.collapsed = *collapsed == "1" && !t.panes.is_empty();
                }
                _ => return None,
            }
        }
        Some(l.mend())
    }
}

/// A layout from before the regions, as it was kept: groups of tabs,
/// split in two again and again.
enum Node {
    Tabs(Tabs),
    Split {
        across: bool,
        share: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// `tabs <active> <collapsed> <pane,pane>` or
/// `across|stacked <share> ( <node> ) ( <node> )`.
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
            Some(Node::Split {
                across: kind == "across",
                share,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
        _ => None,
    }
}

/// Each pane of an old layout to where it goes now, the tab that was
/// shown still shown; and the room the timeline had over the panes
/// below it kept.
fn from_tree(n: &Node) -> Layout {
    fn walk(n: &Node, l: &mut Layout, shown: &mut Vec<Pane>) {
        match n {
            Node::Tabs(t) => {
                for &p in &t.panes {
                    if !p.fixed() {
                        let r = p.home();
                        if l.region_of(p).is_none() {
                            l.tabs_mut(r).panes.push(p);
                        }
                    }
                }
                if let Some(p) = t.shown() {
                    shown.push(p);
                }
            }
            Node::Split {
                across,
                share,
                first,
                second,
            } => {
                let timeline_above =
                    matches!(&**first, Node::Tabs(t) if t.panes.contains(&Pane::Timeline));
                if !across && timeline_above && share.is_finite() {
                    l.shares[Region::Dock.index()] = ((1.0 - share) * 1000.0).round() / 1000.0;
                }
                walk(first, l, shown);
                walk(second, l, shown);
            }
        }
    }
    let mut l = Layout::with(&[], &[], &[], DOCK_SHARE);
    let mut shown = Vec::new();
    walk(n, &mut l, &mut shown);
    for t in &mut l.regions {
        t.active = (t.panes.iter())
            .position(|p| shown.contains(p))
            .unwrap_or(0);
    }
    l
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
            "windows {}\ninspector known\nlayout {}\n",
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
        // Kept from before the inspector came: it opens once.
        let mut known = false;
        for line in text.lines() {
            let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
            match what {
                "windows" => self.windows_allowed = rest.trim() == "on",
                "inspector" => known = true,
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
        if !known && text.lines().any(|l| l.starts_with("layout ")) {
            self.layout.open(Pane::Inspector);
            self.changed = true;
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

    /// What an assistant sees of the main window: what each region
    /// holds and shows, the panes in windows of their own, and the pane
    /// shown full size.
    pub(crate) fn value(&self) -> serde_json::Value {
        let ids = |ps: &[Pane]| ps.iter().map(|p| p.id()).collect::<Vec<_>>();
        let mut v = serde_json::json!({
            "windows": self.layout.windows.iter().map(|w| w.pane.id()).collect::<Vec<_>>(),
            "full_size": self.full.map(Pane::id),
            "current": self.current.id(),
        });
        for r in REGIONS {
            let t = self.layout.tabs(r);
            v[r.id()] = serde_json::json!({
                "panes": ids(&t.panes),
                "shown": t.shown().map(Pane::id),
                "collapsed": t.collapsed,
            });
        }
        v
    }

    /// Opens, moves, collapses or closes a pane, as an assistant asks.
    pub(crate) fn set_pane(
        &mut self,
        p: Pane,
        open: bool,
        to: Option<Region>,
        collapsed: Option<bool>,
    ) -> Result<(), String> {
        if p.fixed() && (!open || to.is_some()) {
            return Err("The timeline is always in the middle of the main window.".into());
        }
        if !open {
            self.close(p);
            return Ok(());
        }
        if let Some(r) = to {
            self.forget_window(p);
            self.layout.put(p, r);
            self.changed = true;
        }
        self.open(p);
        if let Some(c) = collapsed {
            let Some(r) = self.layout.region_of(p) else {
                return Err(format!("{} isn't in a panel to collapse.", p.title()));
            };
            self.layout.regions[r.index()].collapsed = c;
            self.changed = true;
        }
        Ok(())
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
            full,
        };
        if let Some(p) = full {
            let mut tabs = Tabs {
                panes: vec![p],
                active: 0,
                collapsed: false,
            };
            cx.tabs(ui, &mut tabs, rect, Id::unique("full pane"), None);
        } else {
            cx.regions(ui, &mut self.layout, rect);
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
            Act::Put(p, r) => {
                self.forget_window(p);
                self.layout.put(p, r);
                self.layout.show(p);
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
                                // ai: set_pane
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
    Put(Pane, Region),
    Grow(Pane, f32),
    /// A region's edge was dragged.
    Moved,
    /// Where a pane's own window is now, and its screen's scale.
    At(Pane, Rect, f32),
}

/// The height of a group's tabs.
const HEADER: f32 = 26.0;
/// The gap between regions, which is dragged to resize them.
const GAP: f32 = 6.0;
/// The least a region shrinks to, when there is room.
const MIN: f32 = 90.0;
/// The least height the dock shrinks to, when there is room.
const DOCK_MIN: f32 = 260.0;
/// A collapsed side panel's width: room for the button to open it.
const NARROW: f32 = 30.0;

struct Draw<'a, H> {
    host: &'a mut H,
    acts: Vec<Act>,
    /// The pane being dragged by its tab.
    drag: Option<Pane>,
    windows: bool,
    full: Option<Pane>,
}

/// Where the regions go in `rect`: left, right, dock, and the timeline.
/// A dialog that opens in the middle of the window the first time, and
/// stays where the user drags it after that. Without this a window with
/// no position opens at the top-left corner, over the menu bar.
pub(crate) fn centred<'a>(w: egui::Window<'a>, ctx: &egui::Context) -> egui::Window<'a> {
    w.pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
}

pub(crate) fn places(l: &Layout, rect: Rect) -> ([Option<Rect>; 3], Rect) {
    let w = rect.width();
    let h = rect.height();
    let size = |r: Region, total: f32| {
        let t = l.tabs(r);
        if t.panes.is_empty() {
            0.0
        } else if t.collapsed {
            if r == Region::Dock { HEADER } else { NARROW }
        } else {
            // The dock holds the editors and instrument panels, which
            // need more height than a side panel needs width.
            let least = if r == Region::Dock { DOCK_MIN } else { MIN };
            (l.shares[r.index()] * total).clamp(least.min(total / 2.0), total * MAX_SHARE)
        }
    };
    let (mut lw, mut rw) = (size(Region::Left, w), size(Region::Right, w));
    // The timeline keeps some room between the side panels.
    let room = (w - 2.0 * GAP - MIN).max(0.0);
    if lw + rw > room && lw + rw > 0.0 {
        let k = room / (lw + rw);
        lw *= k;
        rw *= k;
    }
    let gap = |s: f32| if s > 0.0 { s + GAP } else { 0.0 };
    let left = (lw > 0.0).then(|| Rect::from_min_size(rect.min, vec2(lw, h)));
    let right =
        (rw > 0.0).then(|| Rect::from_min_max(pos2(rect.right() - rw, rect.top()), rect.max));
    let middle = Rect::from_min_max(
        pos2(rect.left() + gap(lw), rect.top()),
        pos2(rect.right() - gap(rw), rect.bottom()),
    );
    let dh = size(Region::Dock, h).min((h - GAP - MIN).max(HEADER.min(h)));
    let dock = (dh > 0.0)
        .then(|| Rect::from_min_max(pos2(middle.left(), middle.bottom() - dh), middle.max));
    let timeline = Rect::from_min_max(middle.min, pos2(middle.right(), middle.bottom() - gap(dh)));
    ([left, right, dock], timeline)
}

/// Which region a pane dropped at `at` on the timeline goes to: its
/// left or right edge puts it in that side panel, anywhere else in the
/// dock.
pub(crate) fn region_at(rect: Rect, at: Pos2) -> Region {
    let x = (at.x - rect.left()) / rect.width().max(1.0);
    if x < 0.25 {
        Region::Left
    } else if x > 0.75 {
        Region::Right
    } else {
        Region::Dock
    }
}

impl<H: Host> Draw<'_, H> {
    fn regions(&mut self, ui: &mut egui::Ui, l: &mut Layout, rect: Rect) {
        let (places, timeline) = places(l, rect);
        for r in REGIONS {
            let Some(at) = places[r.index()] else {
                continue;
            };
            let t = l.tabs(r);
            if !t.collapsed {
                let edge = match r {
                    Region::Left => Rect::from_min_max(
                        pos2(at.right(), at.top()),
                        pos2(at.right() + GAP, at.bottom()),
                    ),
                    Region::Right => Rect::from_min_max(
                        pos2(at.left() - GAP, at.top()),
                        pos2(at.left(), at.bottom()),
                    ),
                    Region::Dock => Rect::from_min_max(
                        pos2(at.left(), at.top() - GAP),
                        pos2(at.right(), at.top()),
                    ),
                };
                let resp = ui.interact(edge, Id::unique(("region edge", r.id())), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(if r == Region::Dock {
                        CursorIcon::ResizeVertical
                    } else {
                        CursorIcon::ResizeHorizontal
                    });
                    let line = ui.visuals().widgets.active.bg_stroke;
                    ui.painter().rect_filled(edge.shrink(1.0), 1.0, line.color);
                }
                if resp.dragged() {
                    let d = resp.drag_delta();
                    let (now, d, total) = match r {
                        Region::Left => (at.width(), d.x, rect.width()),
                        Region::Right => (at.width(), -d.x, rect.width()),
                        Region::Dock => (at.height(), -d.y, rect.height()),
                    };
                    l.shares[r.index()] = ((now + d) / total.max(1.0)).clamp(MIN_SHARE, MAX_SHARE);
                    self.acts.push(Act::Moved);
                }
            }
            let mut t = l.tabs(r).clone();
            self.tabs(ui, &mut t, at, Id::unique(("region", r.id())), Some(r));
        }
        self.timeline(ui, timeline);
    }

    /// The timeline, in the middle, without tabs of its own.
    fn timeline(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = Pane::Timeline;
        let mut tui = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink(2.0))
                .id_salt(("pane", p.id())),
        );
        tui.set_clip_rect(rect.intersect(ui.clip_rect()));
        self.host.pane(&mut tui, p);
        let clicked = ui.input(|i| {
            i.pointer.primary_pressed()
                && i.pointer.interact_pos().is_some_and(|at| rect.contains(at))
        });
        if clicked {
            self.acts.push(Act::Current(p));
        }
        let Some(dragged) = self.drag else {
            return;
        };
        let Some(at) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        if !rect.contains(at) {
            return;
        }
        let r = region_at(rect, at);
        let zone = match r {
            Region::Left => Rect::from_min_max(
                rect.min,
                pos2(rect.left() + rect.width() / 3.0, rect.bottom()),
            ),
            Region::Right => Rect::from_min_max(
                pos2(rect.right() - rect.width() / 3.0, rect.top()),
                rect.max,
            ),
            Region::Dock => Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max),
        };
        self.drop_here(ui, dragged, r, zone);
    }

    /// Shows where a dragged pane would go, and puts it there when let go.
    fn drop_here(&mut self, ui: &egui::Ui, p: Pane, r: Region, zone: Rect) {
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
        let words = format!("Into {}", r.the());
        painter.text(
            zone.center(),
            egui::Align2::CENTER_CENTER,
            words,
            egui::FontId::proportional(16.0),
            crate::theme::look(ui.ctx()).c().text,
        );
        if ui.input(|i| i.pointer.any_released()) {
            self.acts.push(Act::Put(p, r));
            DragAndDrop::clear_payload(ui.ctx());
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui, t: &mut Tabs, rect: Rect, id: Id, here: Option<Region>) {
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
            // A narrow collapsed panel: only the button to open it.
            self.collapse_button(&mut hui, t, shown, here);
        } else {
            // Tabs that don't all fit beside the header's buttons share
            // the room equally and truncate, rather than the last one
            // losing its end.
            let pad = 2.0 * hui.spacing().button_padding.x;
            let gap = hui.spacing().item_spacing.x;
            let font = egui::TextStyle::Button.resolve(hui.style());
            let natural: f32 = hui.fonts_mut(|f| {
                (t.panes.iter())
                    .map(|p| {
                        f.layout_no_wrap(p.title().into(), font.clone(), Color32::WHITE)
                            .size()
                            .x
                            + pad
                            + gap
                    })
                    .sum()
            });
            // The buttons go first, from the right, so the tabs get
            // exactly what they leave: their width differs by theme.
            // When the names won't fit beside them (judged by the
            // buttons' width last frame), the buttons draw tight, with
            // no padding round their icons, to give the names the room.
            let whole = hui.max_rect();
            let last = hui.data(|d| d.get_temp::<f32>(id.with("buttons")));
            let tight = last.is_some_and(|b| natural > whole.width() - b);
            let mut bui = hui.new_child(
                UiBuilder::new()
                    .max_rect(whole)
                    .id_salt("buttons")
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            if tight {
                bui.spacing_mut().button_padding.x = 1.0;
                bui.spacing_mut().item_spacing.x = 2.0;
            }
            self.header_buttons(&mut bui, t, shown, here);
            let buttons = whole.right() - bui.min_rect().left() + gap;
            // What they take at their natural size, for next frame.
            let natural_buttons = if tight {
                hui.data(|d| d.get_temp::<f32>(id.with("buttons_wide")))
                    .unwrap_or(buttons)
            } else {
                buttons
            };
            hui.data_mut(|d| {
                d.insert_temp(id.with("buttons"), natural_buttons);
                d.insert_temp(id.with("buttons_wide"), natural_buttons);
            });
            let room = whole.width() - buttons;
            let each = (natural > room).then(|| ((room / t.panes.len() as f32) - gap).max(28.0));
            hui.set_max_width(room);
            for (i, &p) in t.panes.iter().enumerate() {
                let mut button = egui::Button::selectable(i == t.active, p.title())
                    .sense(Sense::click_and_drag());
                if let Some(w) = each {
                    button = button.min_size(vec2(w, 0.0)).truncate();
                }
                let tab = match each {
                    Some(w) => {
                        hui.allocate_ui_with_layout(
                            vec2(w, hui.available_height()),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.add(button),
                        )
                        .inner
                    }
                    None => hui.add(button),
                }
                .accessible_name(format!("{} tab", p.title()))
                .on_hover_text("Drag to move it to another panel");
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
        // A tab dragged here from another region joins this one.
        if let (Some(r), Some(p)) = (here, self.drag)
            && !t.panes.contains(&p)
            && ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|at| rect.contains(at))
        {
            self.drop_here(ui, p, r, rect);
        }
    }

    fn collapse_button(&mut self, ui: &mut egui::Ui, t: &Tabs, shown: Pane, here: Option<Region>) {
        let (icon, what) = if t.collapsed {
            (Icon::Expand, "Expand")
        } else {
            (Icon::Collapse, "Collapse")
        };
        let whom = here.map_or_else(|| shown.the(), |r| r.the().to_string());
        if ui
            .add(
                // ai: set_pane
                IconButton::new(icon, format!("{what} {whom}"))
                    .tip(what)
                    .small(),
            )
            .clicked()
        {
            self.acts.push(Act::Collapse(shown));
        }
    }

    /// From the right: close, own window, full size, collapse, and the
    /// menu with the rest.
    fn header_buttons(&mut self, ui: &mut egui::Ui, t: &Tabs, shown: Pane, here: Option<Region>) {
        let the = shown.the();
        if !shown.fixed()
            && ui
                .add(
                    // ai: set_pane
                    IconButton::new(
                        Icon::Close,
                        format!("Close {}", shown.title().to_lowercase()),
                    )
                    .tip("Close")
                    .small(),
                )
                .clicked()
        {
            self.acts.push(Act::Close(shown));
        }
        if self.windows
            && !shown.fixed()
            && ui
                .add(
                    // ai: set_pane
                    IconButton::new(Icon::PopOut, format!("Open {the} in its own window"))
                        .tip("Its own window, to put on another screen")
                        .small(),
                )
                .clicked()
        {
            self.acts.push(Act::Detach(shown));
        }
        let full = self.full.is_some();
        if ui
            // ai: full-size
            .add(
                if full {
                    IconButton::new(Icon::Restore, "Back to all panes")
                } else {
                    // ai: full-size
                    IconButton::new(Icon::FullSize, format!("Show {the} full size"))
                        .tip("Full size")
                }
                .small(),
            )
            .clicked()
        {
            self.acts.push(Act::Full(shown));
        }
        if full {
            return;
        }
        self.collapse_button(ui, t, shown, here);
        // ai: view
        IconButton::new(Icon::More, format!("More for {the}"))
            .tip("Move, resize and more")
            .small()
            .menu(ui, |ui| self.menu(ui, shown, here));
    }

    /// The pane's menu: moving it, without dragging.
    fn menu(&mut self, ui: &mut egui::Ui, p: Pane, here: Option<Region>) {
        if !p.fixed() {
            ui.menu_button("Move it", |ui| {
                for r in REGIONS.into_iter().filter(|&r| Some(r) != here) {
                    // ai: set_pane
                    if ui.button(format!("Into {}", r.the())).clicked() {
                        self.acts.push(Act::Put(p, r));
                        ui.close();
                    }
                }
            });
        }
        // ai: view
        if ui.button("Larger").clicked() {
            self.acts.push(Act::Grow(p, 0.05));
        }
        // ai: view
        if ui.button("Smaller").clicked() {
            self.acts.push(Act::Grow(p, -0.05));
        }
        // ai: set_pane
        if self.windows && !p.fixed() && ui.button("Open in its own window").clicked() {
            self.acts.push(Act::Detach(p));
        }
        // ai: set_pane
        if !p.fixed() && ui.button("Close").clicked() {
            self.acts.push(Act::Close(p));
        }
    }
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
        changed |= self.sync_track_panes();
        if changed {
            ui.ctx().request_repaint();
        }
        self.commands.names = (self.workspace.saved.iter().enumerate())
            .map(|(n, (name, _))| (Cmd::Layout(n as u8), format!("Layout {}: {name}", n + 1)))
            .collect();
        let mut ws = std::mem::take(&mut self.workspace);
        self.drawing_panes = true;
        self.practice_ui.drawn = false;
        ws.show(ui, self);
        self.drawing_panes = false;
        self.workspace = ws;
        // The practice panel's tuner listens while the panel shows.
        if let Some(c) = self.tuner.set_in_panel(self.practice_ui.drawn) {
            self.send(c);
        }
        let ctx = ui.ctx().clone();
        for c in std::mem::take(&mut self.workspace_cmds) {
            self.run(&ctx, c);
        }
        for p in std::mem::take(&mut self.panes_to_open) {
            self.workspace.open(p);
        }
    }

    /// Opens a pane and shows its tab. While the panes are drawn the
    /// workspace is out being drawn, so it opens once they are.
    pub(crate) fn open_pane(&mut self, p: Pane) {
        if self.drawing_panes {
            self.panes_to_open.push(p);
        } else {
            self.workspace.open(p);
        }
    }

    /// A pane in a menu: ticked while it is open, and clicking it then
    /// closes it.
    pub(crate) fn pane_entry(&mut self, ui: &mut egui::Ui, c: Cmd) {
        if self.menu_hold(c) {
            return;
        }
        self.menu_mark(ui);
        let open = match c {
            Cmd::Mixer => self.workspace.layout.is_open(Pane::Mixer),
            Cmd::MonitorSection => self.workspace.layout.is_open(Pane::Monitor),
            Cmd::TakeLanes => self.workspace.layout.is_open(Pane::Takes),
            Cmd::AutomationLanes => self.workspace.layout.is_open(Pane::Automation),
            Cmd::Browser => self.workspace.layout.is_open(Pane::Browser),
            Cmd::Inspector => self.workspace.layout.is_open(Pane::Inspector),
            Cmd::InstrumentWindow => self.workspace.layout.is_open(Pane::Instrument),
            Cmd::EffectsPane => self.workspace.layout.is_open(Pane::Effects),
            Cmd::NotationSettings => self.workspace.layout.is_open(Pane::Notation),
            Cmd::Video => self.workspace.layout.is_open(Pane::Video),
            Cmd::Fretboard => self.workspace.layout.is_open(Pane::Fretboard),
            Cmd::PitchEditor => self.workspace.layout.is_open(Pane::PitchEditor),
            Cmd::PracticePanel => self.workspace.layout.is_open(Pane::Practice),
            Cmd::Comments => self.comment_ui.open,
            Cmd::LessonNotes => self.notes_ui.open,
            Cmd::Lessons => self.workspace.layout.is_open(Pane::Lessons),
            Cmd::NotesEditor => self.workspace.layout.is_open(Pane::NotesEditor),
            Cmd::PatternEditor => self.workspace.layout.is_open(Pane::Patterns),
            _ => self.tuner.open,
        };
        let name = c.info().name.trim_end_matches('…');
        let keys = self.commands.keymap.label(c);
        let r = ui
            .add(egui::Button::selectable(open, name).shortcut_text(keys))
            .on_hover_text(if open { "Close it" } else { "Open it" });
        if r.clicked() {
            ui.close();
            let ctx = ui.ctx().clone();
            match (c, open) {
                (Cmd::Comments, true) => self.workspace.close(Pane::Comments),
                (Cmd::LessonNotes, true) => self.workspace.close(Pane::Notes),
                (Cmd::Tuner, true) => self.workspace.close(Pane::Tuner),
                (Cmd::InstrumentWindow, true) => self.workspace.close(Pane::Instrument),
                (Cmd::EffectsPane, true) => self.workspace.close(Pane::Effects),
                (Cmd::NotationSettings, true) => self.workspace.close(Pane::Notation),
                _ => self.run(&ctx, c),
            }
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
                    // ai: view
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
                // ai: layouts.save
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
                    // ai: layouts.save
                    if ui.add_enabled(ok, egui::Button::new("Save")).clicked() || (enter && ok) {
                        ws.save_as(&name);
                        done = true;
                    }
                    // ai: view
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
                            ui.weak(self.commands.keymap.label(Cmd::Layout(n as u8)));
                            // ai: layouts.remove
                            if ui
                                .add(
                                    IconButton::new(
                                        Icon::Close,
                                        format!("Remove the layout {saved}"),
                                    )
                                    .tip("Remove")
                                    .small(),
                                )
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

pub(crate) const TOOLS: &[crate::agent::Tool] = &[crate::agent::Tool {
    name: "layouts",
    description: "The saved layouts of the panes (each used with its command, layout-1 to \
                  layout-9): save keeps the panes as they are now under a name, replacing a \
                  layout of that name; remove takes one away by name (those after it move \
                  up one). Answers with the layouts saved.",
    access: crate::agent::Access::Edit,
    schema: || {
        serde_json::json!({"type": "object", "properties": {
            "save": {"type": "string", "description": "A name, such as Mixing."},
            "remove": {"type": "string"}
        }})
    },
}];

impl crate::App {
    pub(crate) fn layouts_tool(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<Result<(serde_json::Value, Option<String>), String>> {
        (name == "layouts").then(|| self.layouts(args).map(|v| (v, None)))
    }

    fn layouts(&mut self, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        use serde_json::json;
        let ws = &mut self.workspace;
        if let Some(n) = args["remove"].as_str() {
            let i = (ws.saved.iter().position(|s| s.0 == n.trim()))
                .ok_or_else(|| format!("There is no layout {n}."))?;
            ws.saved.remove(i);
            ws.changed();
        }
        if let Some(n) = args["save"].as_str() {
            if n.trim().is_empty() {
                return Err("Give the layout a name.".into());
            }
            if !ws.save_as(n) {
                return Err(format!(
                    "{SLOTS} layouts are saved: remove one, or use one's name to replace it."
                ));
            }
        }
        let saved: Vec<serde_json::Value> = (ws.saved.iter().enumerate())
            .map(|(i, s)| json!({"name": s.0, "command": format!("layout-{}", i + 1)}))
            .collect();
        Ok(json!({"layouts": saved}))
    }
}

impl Host for crate::App {
    fn pane(&mut self, ui: &mut egui::Ui, pane: Pane) {
        // Where it is, for an assistant's picture of it.
        let at = (pane, ui.ctx().viewport_id(), ui.max_rect());
        self.listening.drawn.push(at);
        match pane {
            Pane::Timeline => self.timeline(ui),
            // The desk scrolls its own strips, with the master pinned.
            Pane::Mixer => self.mixer(ui),
            Pane::Monitor => {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.monitor_pane(ui));
            }
            Pane::Comments => self.comments_panel(ui),
            Pane::Takes => self.takes_pane(ui),
            Pane::Automation => self.automation_pane(ui),
            Pane::Browser => self.browser_pane(ui),
            Pane::Inspector => self.inspector_pane(ui),
            Pane::Instrument => self.instrument_pane(ui),
            Pane::Effects => self.effects_pane(ui),
            Pane::Notation => self.notation_pane(ui),
            Pane::Video => self.video_pane(ui),
            Pane::Fretboard => {
                egui::ScrollArea::vertical()
                    .id_salt("fretboard-pane")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.fretboard_pane(ui))
                    .inner
            }
            Pane::PitchEditor => {
                egui::ScrollArea::vertical()
                    .id_salt("pitch-editor-pane")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.pitch_editor_pane(ui))
                    .inner
            }
            Pane::Notes => self.notes_pane(ui),
            Pane::Lessons => self.lessons_pane(ui),
            Pane::NotesEditor => self.notes_editor_pane(ui),
            Pane::Patterns => {
                egui::ScrollArea::vertical()
                    .id_salt("patterns-pane")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.pattern_editor_pane(ui))
                    .inner
            }
            Pane::Tuner => {
                let inputs = self.input_count();
                if let Some(c) = self.tuner.pane(ui, inputs) {
                    self.send(c);
                }
            }
            Pane::Practice => self.practice_pane(ui),
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        self.command_keys(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The timeline over the mixer, without the inspector.
    fn plain() -> Layout {
        let mut l = Layout::recording();
        l.remove(Pane::Inspector);
        l
    }

    #[test]
    fn the_inspector_is_on_the_right_from_the_start() {
        let l = Layout::recording();
        assert_eq!(l.tabs(Region::Right).panes, [Pane::Inspector]);
        assert_eq!(l.tabs(Region::Dock).panes, [Pane::Mixer]);
        assert!(l.tabs(Region::Left).panes.is_empty());
        // Closed and opened again, it goes back there.
        let mut l = plain();
        l.open(Pane::Inspector);
        assert_eq!(l, Layout::recording());
    }

    #[test]
    fn a_layout_from_before_the_inspector_gets_it_once() {
        let mut w = Workspace::default();
        w.read("windows off\nlayout dock tabs 0 0 timeline\n");
        assert!(w.layout.is_open(Pane::Inspector));
        let mut w = Workspace::default();
        w.read("windows off\ninspector known\nlayout dock tabs 0 0 timeline\n");
        assert!(
            !w.layout.is_open(Pane::Inspector),
            "closed since stays closed"
        );
        assert!(w.to_text().contains("inspector known"));
    }

    #[test]
    fn opens_panes_in_their_regions() {
        let mut l = plain();
        l.open(Pane::Comments);
        l.open(Pane::Notes);
        l.open(Pane::Browser);
        l.open(Pane::NotesEditor);
        l.open(Pane::PitchEditor);
        assert_eq!(l.tabs(Region::Right).panes, [Pane::Comments, Pane::Notes]);
        assert_eq!(l.tabs(Region::Left).panes, [Pane::Browser]);
        let dock = l.tabs(Region::Dock);
        assert_eq!(
            dock.panes,
            [Pane::Mixer, Pane::NotesEditor, Pane::PitchEditor]
        );
        // The one opened last is shown.
        assert_eq!(dock.shown(), Some(Pane::PitchEditor));
        assert_eq!(l.panes()[0], Pane::Timeline);
    }

    #[test]
    fn closing_a_tab_shows_its_neighbour_and_the_timeline_stays() {
        let mut l = plain();
        l.open(Pane::Automation);
        l.open(Pane::Video);
        l.show(Pane::Automation);
        // The tab after it is shown.
        l.remove(Pane::Automation);
        assert_eq!(l.tabs(Region::Dock).shown(), Some(Pane::Video));
        l.remove(Pane::Mixer);
        l.remove(Pane::Video);
        assert!(l.tabs(Region::Dock).panes.is_empty());
        l.remove(Pane::Timeline);
        assert!(l.docked(Pane::Timeline));
    }

    #[test]
    fn moves_a_pane_into_another_region() {
        let mut l = Layout::recording();
        l.put(Pane::Mixer, Region::Right);
        assert_eq!(l.tabs(Region::Right).panes, [Pane::Inspector, Pane::Mixer]);
        assert!(l.tabs(Region::Dock).panes.is_empty());
        // The timeline stays in the middle.
        l.put(Pane::Timeline, Region::Left);
        assert!(l.tabs(Region::Left).panes.is_empty());
    }

    #[test]
    fn grows_a_region() {
        let mut l = plain();
        l.grow(Pane::Mixer, 0.1);
        assert!((l.shares[Region::Dock.index()] - 0.52).abs() < 1e-4);
        // Growing the timeline shrinks the dock.
        l.grow(Pane::Timeline, 0.2);
        assert!((l.shares[Region::Dock.index()] - 0.32).abs() < 1e-4);
        l.grow(Pane::Timeline, 5.0);
        assert_eq!(l.shares[Region::Dock.index()], MIN_SHARE);
    }

    #[test]
    fn the_regions_fit_round_the_timeline() {
        let mut l = Layout::recording();
        l.open(Pane::Browser);
        let rect = Rect::from_min_size(pos2(0.0, 40.0), vec2(1920.0, 1100.0));
        let ([left, right, dock], timeline) = places(&l, rect);
        let (left, right, dock) = (left.unwrap(), right.unwrap(), dock.unwrap());
        // The side panels the whole height, the dock under the timeline.
        assert_eq!(left.height(), rect.height());
        assert_eq!(right.height(), rect.height());
        assert!(left.right() < timeline.left() && timeline.right() < right.left());
        assert!(timeline.bottom() < dock.top());
        assert_eq!(dock.left(), timeline.left());
        assert_eq!(dock.right(), timeline.right());
        // A collapsed one keeps its header.
        l.toggle_collapsed(Pane::Mixer);
        let ([_, _, dock], timeline) = places(&l, rect);
        assert_eq!(dock.unwrap().height(), HEADER);
        assert!(timeline.height() > rect.height() - 40.0);
        // An empty one takes no room.
        let ([left, _, _], _) = places(&Layout::recording(), rect);
        assert!(left.is_none());
        // In a small window the timeline keeps some width.
        l.shares = [0.7, 0.7, 0.42];
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0));
        let (_, timeline) = places(&l, small);
        assert!(timeline.width() >= MIN - 0.5, "{timeline:?}");
    }

    #[test]
    fn a_dropped_tab_goes_by_where_it_is_let_go() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        assert_eq!(region_at(r, pos2(10.0, 50.0)), Region::Left);
        assert_eq!(region_at(r, pos2(90.0, 50.0)), Region::Right);
        assert_eq!(region_at(r, pos2(50.0, 90.0)), Region::Dock);
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
        l.toggle_collapsed(Pane::Practice);
        l.detach(
            Pane::Mixer,
            Some((
                Rect::from_min_size(pos2(1930.0, 40.0), vec2(800.0, 600.0)),
                1.5,
            )),
        );
        l.detach(Pane::Comments, None);
        l.open(Pane::Browser);
        let text = l.to_text();
        assert_eq!(Layout::from_text(&text), Some(l), "{text}");
    }

    #[test]
    fn layouts_from_before_the_regions_are_carried_over() {
        // Recording as it was: the timeline over the mixer, the
        // inspector at the side.
        let old = "dock across 0.830 ( stacked 0.620 ( tabs 0 0 timeline ) \
                   ( tabs 0 0 mixer ) ) ( tabs 0 0 inspector )";
        let mut l = Layout::from_text(old).unwrap();
        assert!((l.shares[Region::Dock.index()] - 0.38).abs() < 1e-4);
        l.shares = Layout::recording().shares;
        assert_eq!(l, Layout::recording());
        // Mixing: the mixer's height is kept.
        let old = "dock stacked 0.320 ( tabs 0 0 timeline ) ( tabs 0 0 mixer )";
        assert_eq!(Layout::from_text(old), Some(Layout::mixing()));
        // Panes anywhere go to their regions, the one shown still shown,
        // and windows stay.
        let old = "dock across 0.5 ( across 0.3 ( tabs 0 0 browser ) ( tabs 0 0 timeline ) ) \
                   ( stacked 0.5 ( tabs 1 0 automation,comments ) ( tabs 0 0 pitch-editor ) ) \
                   ; window tuner";
        let l = Layout::from_text(old).unwrap();
        assert_eq!(l.tabs(Region::Left).panes, [Pane::Browser]);
        assert_eq!(l.tabs(Region::Right).panes, [Pane::Comments]);
        let dock = l.tabs(Region::Dock);
        assert_eq!(dock.panes, [Pane::Automation, Pane::PitchEditor]);
        assert_eq!(dock.shown(), Some(Pane::PitchEditor));
        assert!(l.in_window(Pane::Tuner));
    }

    #[test]
    fn mends_a_layout_with_panes_twice_or_none() {
        let l = Layout::from_text(
            "panels 0.2 9 0.4 ; left 0 0 mixer,timeline ; dock 5 0 mixer,nonsense ; window mixer",
        )
        .unwrap();
        // The timeline stays in the middle, the mixer kept once, in the
        // main window, and the sizes in range.
        assert_eq!(l.panes(), [Pane::Timeline, Pane::Mixer]);
        assert_eq!(l.tabs(Region::Left).active, 0);
        assert!(l.windows.is_empty());
        assert_eq!(l.shares[Region::Right.index()], MAX_SHARE);
        assert_eq!(Layout::from_text("nonsense"), None);
        assert_eq!(Layout::from_text("panels 0.2 0.2"), None);
        assert_eq!(
            Layout::from_text("dock across 0.5 ( tabs 0 0 timeline )"),
            None
        );
    }

    #[test]
    fn an_assistant_opens_moves_and_collapses_panes() {
        let mut w = Workspace::default();
        w.set_pane(Pane::PitchEditor, true, None, None).unwrap();
        assert_eq!(w.value()["dock"]["shown"], "pitch-editor");
        w.set_pane(Pane::PitchEditor, true, Some(Region::Right), Some(true))
            .unwrap();
        let v = w.value();
        assert_eq!(
            v["right"]["panes"],
            serde_json::json!(["inspector", "pitch-editor"])
        );
        assert_eq!(v["right"]["collapsed"], true);
        assert_eq!(v["dock"]["shown"], "mixer");
        w.set_pane(Pane::PitchEditor, false, None, None).unwrap();
        assert!(!w.layout.is_open(Pane::PitchEditor));
        assert!(w.set_pane(Pane::Timeline, false, None, None).is_err());
        assert!(
            w.set_pane(Pane::Timeline, true, Some(Region::Left), None)
                .is_err()
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
        assert!(w.layout.is_open(Pane::Practice));
        w.recall(0);
        assert!(!w.layout.is_open(Pane::Practice));
        // Nine at most.
        for i in 0..SLOTS {
            w.save_as(&format!("Layout {i}"));
        }
        assert_eq!(w.saved.len(), SLOTS);
        assert!(!w.save_as("One more"));
        assert!(w.save_as("Mixing"));
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
            // The practice panel shows first, and its tuner listens.
            assert!(h.state().notes_ui.open && h.state().tuner.listening());
            assert!(h.query_by_label("Lesson notes tab").is_some());
            h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
            h.run();
            h.run();
            assert!(!h.state().notes_ui.open && !h.state().tuner.listening());
        }

        #[test]
        fn a_tab_dragged_to_the_timeline_edge_goes_to_that_side() {
            let mut h = harness();
            let from = h.get_by_label("Mixer tab").rect().center();
            // Near the timeline's left edge: into the left panel.
            let mixer = h.get_by_label("Mixer tab").rect();
            let to = pos2(mixer.left() + 40.0, 300.0);
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
            let l = &h.state().workspace.layout;
            assert_eq!(l.tabs(Region::Left).panes, [Pane::Mixer]);
            assert!(l.tabs(Region::Dock).panes.is_empty());
            // And onto the right panel: a tab there.
            let from = h.get_by_label("Mixer tab").rect().center();
            let to = h.get_by_label("Inspector tab").rect().center() + vec2(0.0, 200.0);
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
            let l = &h.state().workspace.layout;
            assert_eq!(l.tabs(Region::Right).panes, [Pane::Inspector, Pane::Mixer]);
        }

        #[test]
        fn the_panels_work_with_an_assistant() {
            use crate::agent::driver::Driver;
            use serde_json::json;
            let mut d = Driver::new();
            let panels =
                |d: &mut Driver| d.call("get_selection", json!({})).unwrap()["panels"].clone();
            // From the start: the inspector on the right, the mixer in
            // the dock.
            let p = panels(&mut d);
            assert_eq!(p["right"]["shown"], "inspector");
            assert_eq!(p["dock"]["panes"], json!(["mixer"]));
            assert_eq!(p["left"]["panes"], json!([]));
            // An editor opens as a dock tab, shown, as from its command.
            d.call("set_pane", json!({"pane": "automation"})).unwrap();
            d.h.run();
            assert_eq!(panels(&mut d)["dock"]["shown"], "automation");
            d.h.get_by_label("Automation tab");
            // The browser opens on the left by its command.
            d.command("browser");
            d.h.run();
            assert_eq!(panels(&mut d)["left"]["panes"], json!(["browser"]));
            // The person picks the mixer's tab: the assistant sees it.
            d.h.get_by_label("Mixer tab").click();
            d.h.run();
            assert_eq!(panels(&mut d)["dock"]["shown"], "mixer");
            // The assistant moves the mixer to the right and collapses
            // the dock; the person sees both.
            d.call("set_pane", json!({"pane": "mixer", "to": "right"}))
                .unwrap();
            d.call("set_pane", json!({"pane": "automation", "collapsed": true}))
                .unwrap();
            d.h.run();
            let p = panels(&mut d);
            assert_eq!(p["right"]["panes"], json!(["inspector", "mixer"]));
            assert_eq!(p["dock"]["collapsed"], true);
            d.h.get_by_label("Expand the dock").click();
            d.h.run();
            assert_eq!(panels(&mut d)["dock"]["collapsed"], false);
            // The person moves the automation from its menu into the left
            // panel.
            d.h.get_by_label("More for the automation").click();
            d.h.run();
            d.h.get_by_label("Move it").click();
            d.h.run();
            d.h.get_by_label("Into the left panel").click();
            d.h.run();
            let p = panels(&mut d);
            assert_eq!(p["left"]["panes"], json!(["browser", "automation"]));
            assert_eq!(p["dock"]["panes"], json!([]));
            // And the assistant closes it; the timeline stays.
            d.call("set_pane", json!({"pane": "automation", "open": false}))
                .unwrap();
            assert!(
                d.call("set_pane", json!({"pane": "timeline", "open": false}))
                    .is_err()
            );
            assert!(d.call("set_pane", json!({"pane": "nonsense"})).is_err());
            assert!(
                d.call("set_pane", json!({"pane": "mixer", "to": "top"}))
                    .is_err()
            );
            d.h.run();
            assert_eq!(panels(&mut d)["left"]["panes"], json!(["browser"]));
        }

        #[test]
        fn full_size_shows_one_pane() {
            let mut h = harness();
            h.get_by_label("Show the mixer full size").click();
            h.run();
            assert_eq!(h.state().workspace.full, Some(Pane::Mixer));
            assert!(h.query_by_label("Inspector tab").is_none());
            h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::F);
            h.run();
            assert_eq!(h.state().workspace.full, None);
            assert!(h.query_by_label("Inspector tab").is_some());
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

    #[test]
    fn an_assistant_saves_and_removes_layouts() {
        use serde_json::json;
        let mut d = crate::agent::driver::Driver::new();
        let before = d.call("layouts", json!({})).unwrap()["layouts"]
            .as_array()
            .unwrap()
            .len();
        assert!(d.call("layouts", json!({"save": "  "})).is_err());
        let r = d.call("layouts", json!({"save": "Vocal edit"})).unwrap();
        assert_eq!(
            r["layouts"][before],
            json!({"name": "Vocal edit", "command": format!("layout-{}", before + 1)})
        );
        let r = d.call("layouts", json!({"save": "Vocal edit"})).unwrap();
        assert_eq!(
            r["layouts"].as_array().unwrap().len(),
            before + 1,
            "replaced"
        );
        assert!(d.call("layouts", json!({"remove": "Nothing"})).is_err());
        let r = d.call("layouts", json!({"remove": "Vocal edit"})).unwrap();
        assert_eq!(r["layouts"].as_array().unwrap().len(), before);
    }
}
