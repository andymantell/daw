// SPDX-License-Identifier: GPL-3.0-or-later
//! Themes (roadmap 2.10): the app's whole look as data. A theme file
//! sets colours, type sizes, corner radii, line weights, spacing, how
//! waveforms are drawn, a texture and a glow, with a dark and a light
//! set of colours. Egui's own style is made from it, and the timeline,
//! the mixer and the other hand-drawn parts read its colours with
//! [`look`] instead of hard-coding any.
//!
//! The built-in themes are files in `crates/app/themes/`, in the same
//! format as the user's own, which are loaded from a `themes` folder in
//! the settings folder. The choice is kept per computer in `theme.txt`,
//! never in the project, so each collaborator runs their own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle, vec2,
};

/// The built-in theme files, by id. The first is the default.
const BUILT_IN: [(&str, &str); 4] = [
    ("valve-glow", include_str!("../themes/valve-glow.theme")),
    ("workshop", include_str!("../themes/workshop.theme")),
    ("vellum", include_str!("../themes/vellum.theme")),
    ("tidal", include_str!("../themes/tidal.theme")),
];

/// A typeface shipped with the app (SIL Open Font License 1.1, its
/// licence beside it in `crates/app/fonts/`), by the name themes use.
struct Face {
    id: &'static str,
    data: &'static [u8],
    /// The weight to use, for a variable font.
    weight: Option<f32>,
}

const FACES: [Face; 5] = [
    Face {
        id: "barlow-condensed",
        data: include_bytes!("../fonts/barlow-condensed/BarlowCondensed-Medium.ttf"),
        weight: None,
    },
    Face {
        id: "ibm-plex-mono",
        data: include_bytes!("../fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf"),
        weight: None,
    },
    Face {
        id: "ibm-plex-sans-condensed",
        data: include_bytes!("../fonts/ibm-plex-sans-condensed/IBMPlexSansCondensed-Regular.ttf"),
        weight: None,
    },
    Face {
        id: "libre-franklin",
        data: include_bytes!("../fonts/libre-franklin/LibreFranklin-wght.ttf"),
        weight: Some(400.0),
    },
    Face {
        id: "nunito",
        data: include_bytes!("../fonts/nunito/Nunito-wght.ttf"),
        weight: Some(500.0),
    },
];

/// Egui's own faces, by the names themes use for them.
const DEFAULT: &str = "default";
const DEFAULT_MONO: &str = "default-mono";

/// The family egui draws a theme's font name with.
fn family(name: &str) -> FontFamily {
    match name {
        DEFAULT => FontFamily::Proportional,
        DEFAULT_MONO => FontFamily::Monospace,
        _ => FontFamily::Name(name.into()),
    }
}

/// Egui's fonts with the bundled faces added, each falling back to
/// egui's own for symbols it lacks.
fn font_definitions() -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();
    let fallback = defs.families[&FontFamily::Proportional].clone();
    for f in &FACES {
        let mut tweak = egui::FontTweak::default();
        if let Some(w) = f.weight {
            tweak.coords.push(b"wght", w);
        }
        defs.font_data.insert(
            f.id.into(),
            Arc::new(egui::FontData::from_static(f.data).tweak(tweak)),
        );
        let mut list = vec![f.id.to_string()];
        list.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name(f.id.into()), list);
    }
    defs
}

static BUILT_INS: LazyLock<Vec<Arc<Theme>>> = LazyLock::new(|| {
    BUILT_IN
        .iter()
        .map(|(id, text)| {
            let map = parse(text).unwrap_or_else(|e| panic!("{id}.theme: {e}"));
            Arc::new(Theme::from_map(id, &map, true).unwrap_or_else(|e| panic!("{id}.theme: {e}")))
        })
        .collect()
});

/// How a waveform is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaveStyle {
    /// Solid, from the lowest to the highest point in each column.
    Filled,
    /// Its top and bottom edges as lines, and a faint centre line.
    Outline,
    /// One line, swinging between the top and bottom.
    Line,
    /// [`WaveStyle::Line`] with a glow around it.
    Trace,
}

/// A texture drawn by code over the timeline's lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Texture {
    None,
    /// A faint scatter of specks, like the tooth of paper.
    Grain,
}

/// How the loop range is filled on the ruler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoopFill {
    Filled,
    /// Diagonal hatching, like a section on a drawing.
    Hatched,
}

/// A colour that may follow the track's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Paint {
    Fixed(Color32),
    /// The track's colour at this opacity, 0 to 1.
    Track(f32),
}

impl Paint {
    pub(crate) fn of(self, track: Color32) -> Color32 {
        match self {
            Paint::Fixed(c) => c,
            Paint::Track(a) => track.gamma_multiply(a),
        }
    }
}

/// One variant's colours.
#[derive(Clone, Debug)]
pub(crate) struct Colours {
    /// Behind everything.
    pub(crate) bg: Color32,
    /// Panels, the ruler and the track names.
    pub(crate) panel: Color32,
    /// The timeline's lanes.
    pub(crate) lane: Color32,
    /// Buttons, fields and mixer strips.
    pub(crate) raised: Color32,
    pub(crate) line: Color32,
    /// Bar lines, and the lines between beats and their parts.
    pub(crate) grid: Color32,
    pub(crate) grid_fine: Color32,
    pub(crate) text: Color32,
    pub(crate) text_dim: Color32,
    pub(crate) accent: Color32,
    /// Text and marks on the accent and on markers.
    pub(crate) on_accent: Color32,
    /// Behind selected text and switched-on buttons, under `text`.
    pub(crate) selection: Color32,
    pub(crate) record: Color32,
    pub(crate) playhead: Color32,
    pub(crate) glow: Color32,
    pub(crate) loop_range: Color32,
    pub(crate) marker: Color32,
    pub(crate) comment: Color32,
    /// Tempo and time-signature changes on the ruler, under `bg` text.
    pub(crate) tempo: Color32,
    pub(crate) signature: Color32,
    pub(crate) clip_fill: Paint,
    pub(crate) clip_border: Paint,
    pub(crate) clip_text: Paint,
    /// A label behind each clip's name, if the theme has one.
    pub(crate) clip_label: Option<Color32>,
    pub(crate) meter_bg: Color32,
    /// Meter levels: below −12 dB, up to 0 dB, and over.
    pub(crate) meter_low: Color32,
    pub(crate) meter_mid: Color32,
    pub(crate) meter_high: Color32,
    /// Messages: all well, take care, and something went wrong.
    pub(crate) good: Color32,
    pub(crate) warn: Color32,
    pub(crate) error: Color32,
    pub(crate) texture: Color32,
    pub(crate) shadow: Color32,
    /// Track colours, used in turn.
    pub(crate) tracks: Vec<Color32>,
}

impl Colours {
    /// Track `i`'s colour.
    pub(crate) fn track(&self, i: usize) -> Color32 {
        self.tracks[i % self.tracks.len()]
    }
}

/// A whole theme.
#[derive(Clone, Debug)]
pub(crate) struct Theme {
    /// Its file's name without `.theme`.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) about: String,
    pub(crate) built_in: bool,
    /// The variant drawn first; the other is derived from it.
    pub(crate) designed_dark: bool,
    /// Corners of buttons, panels and windows, and of clips.
    pub(crate) radius: f32,
    pub(crate) clip_radius: f32,
    /// Weight of borders and lines.
    pub(crate) line: f32,
    /// Space between widgets, and inside buttons.
    pub(crate) gap: egui::Vec2,
    pub(crate) padding: egui::Vec2,
    /// Blur of the shadow under windows and clips; 0 for none.
    pub(crate) shadow: f32,
    pub(crate) loop_fill: LoopFill,
    pub(crate) body: f32,
    pub(crate) small: f32,
    pub(crate) heading: f32,
    pub(crate) numerals: f32,
    /// The typeface for text, and for readouts such as the position: a
    /// bundled face's name, `default` or `default-mono`.
    pub(crate) font: String,
    pub(crate) numerals_font: String,
    pub(crate) wave: WaveStyle,
    pub(crate) wave_weight: f32,
    pub(crate) texture: Texture,
    /// Glow around the playhead and traced waveforms, if on.
    pub(crate) glow: Option<f32>,
    pub(crate) dark: Colours,
    pub(crate) light: Colours,
}

/// A theme file's settings, by `section.key`.
type Map = BTreeMap<String, String>;

/// Reads a theme file: `key = value` lines under `[section]` headings,
/// with `#` starting a comment.
fn parse(text: &str) -> Result<Map, String> {
    let mut map = Map::new();
    let mut section = String::new();
    for (n, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = s.trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("line {}: expected `name = value`", n + 1));
        };
        let key = if section.is_empty() {
            k.trim().to_string()
        } else {
            format!("{section}.{}", k.trim())
        };
        map.insert(key, v.trim().to_string());
    }
    Ok(map)
}

/// The line without a trailing comment: a `#` at the start, or one after
/// a space that is not a colour (`# note`, not `#1a2b3c`).
fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    let b = line.as_bytes();
    for i in 1..b.len() {
        if b[i] == b'#' && b[i - 1] == b' ' {
            let rest = &line[i + 1..];
            let hex = rest.bytes().take_while(|c| c.is_ascii_hexdigit()).count();
            let colour =
                matches!(hex, 6 | 8) && rest[hex..].chars().next().is_none_or(char::is_whitespace);
            if !colour {
                return &line[..i];
            }
        }
    }
    line
}

/// `#rrggbb` or `#rrggbbaa`.
pub(crate) fn colour(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    if !matches!(h.len(), 6 | 8) || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let a = if h.len() == 8 { byte(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(
        byte(0)?,
        byte(2)?,
        byte(4)?,
        a,
    ))
}

/// `#rrggbb[aa]`, `track` or `track 35%`.
fn paint(s: &str) -> Option<Paint> {
    if let Some(rest) = s.strip_prefix("track") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Some(Paint::Track(1.0));
        }
        let pct: f32 = rest.strip_suffix('%')?.trim().parse().ok()?;
        return (0.0..=100.0)
            .contains(&pct)
            .then_some(Paint::Track(pct / 100.0));
    }
    colour(s).map(Paint::Fixed)
}

/// Looks keys up in a theme's map, saying which is missing or wrong.
struct Reader<'a> {
    map: &'a Map,
}

impl Reader<'_> {
    fn get(&self, key: &str) -> Result<&str, String> {
        self.map
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("`{key}` is missing"))
    }

    fn bad(key: &str, v: &str, want: &str) -> String {
        format!("`{key} = {v}`: expected {want}")
    }

    fn num(&self, key: &str, range: std::ops::RangeInclusive<f32>) -> Result<f32, String> {
        let v = self.get(key)?;
        v.parse::<f32>()
            .ok()
            .filter(|n| range.contains(n))
            .ok_or_else(|| {
                Self::bad(
                    key,
                    v,
                    &format!("a number from {} to {}", range.start(), range.end()),
                )
            })
    }

    fn pair(&self, key: &str) -> Result<egui::Vec2, String> {
        let v = self.get(key)?;
        let n: Vec<f32> = v
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .filter(|n| (0.0..=40.0).contains(n))
            .collect();
        match n[..] {
            [x, y] if v.split_whitespace().count() == 2 => Ok(vec2(x, y)),
            _ => Err(Self::bad(
                key,
                v,
                "two numbers from 0 to 40, across and down",
            )),
        }
    }

    fn yes(&self, key: &str) -> Result<bool, String> {
        match self.get(key)? {
            "yes" | "on" | "true" => Ok(true),
            "no" | "off" | "false" => Ok(false),
            v => Err(Self::bad(key, v, "yes or no")),
        }
    }

    fn choice<T: Copy>(&self, key: &str, options: &[(&str, T)]) -> Result<T, String> {
        let v = self.get(key)?;
        (options.iter().find(|(name, _)| *name == v).map(|(_, t)| *t)).ok_or_else(|| {
            let names: Vec<_> = options.iter().map(|(n, _)| *n).collect();
            Self::bad(key, v, &format!("one of {}", names.join(", ")))
        })
    }

    fn font(&self, key: &str) -> Result<String, String> {
        let v = self.get(key)?;
        if v == DEFAULT || v == DEFAULT_MONO || FACES.iter().any(|f| f.id == v) {
            return Ok(v.to_string());
        }
        let names: Vec<_> = (FACES.iter().map(|f| f.id))
            .chain([DEFAULT, DEFAULT_MONO])
            .collect();
        Err(Self::bad(key, v, &format!("one of {}", names.join(", "))))
    }

    fn colour(&self, key: &str) -> Result<Color32, String> {
        let v = self.get(key)?;
        colour(v).ok_or_else(|| Self::bad(key, v, "a colour like #1a2b3c or #1a2b3c80"))
    }

    fn paint(&self, key: &str) -> Result<Paint, String> {
        let v = self.get(key)?;
        paint(v).ok_or_else(|| Self::bad(key, v, "a colour, `track` or `track 35%`"))
    }

    fn colours(&self, variant: &str) -> Result<Colours, String> {
        let c = |k: &str| self.colour(&format!("{variant}.{k}"));
        let p = |k: &str| self.paint(&format!("{variant}.{k}"));
        let label_key = format!("{variant}.clip_label");
        let clip_label = match self.get(&label_key)? {
            "none" => None,
            _ => Some(self.colour(&label_key)?),
        };
        let tracks_key = format!("{variant}.tracks");
        let tv = self.get(&tracks_key)?;
        let tracks: Option<Vec<Color32>> = tv.split_whitespace().map(colour).collect();
        let tracks = tracks
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Self::bad(&tracks_key, tv, "colours separated by spaces"))?;
        Ok(Colours {
            bg: c("bg")?,
            panel: c("panel")?,
            lane: c("lane")?,
            raised: c("raised")?,
            line: c("line")?,
            grid: c("grid")?,
            grid_fine: c("grid_fine")?,
            text: c("text")?,
            text_dim: c("text_dim")?,
            accent: c("accent")?,
            on_accent: c("on_accent")?,
            selection: c("selection")?,
            record: c("record")?,
            playhead: c("playhead")?,
            glow: c("glow")?,
            loop_range: c("loop")?,
            marker: c("marker")?,
            comment: c("comment")?,
            tempo: c("tempo")?,
            signature: c("signature")?,
            clip_fill: p("clip_fill")?,
            clip_border: p("clip_border")?,
            clip_text: p("clip_text")?,
            clip_label,
            meter_bg: c("meter_bg")?,
            meter_low: c("meter_low")?,
            meter_mid: c("meter_mid")?,
            meter_high: c("meter_high")?,
            good: c("good")?,
            warn: c("warn")?,
            error: c("error")?,
            texture: c("texture")?,
            shadow: c("shadow")?,
            tracks,
        })
    }
}

impl Theme {
    fn from_map(id: &str, map: &Map, built_in: bool) -> Result<Theme, String> {
        let r = Reader { map };
        Ok(Theme {
            id: id.to_string(),
            name: r.get("name")?.to_string(),
            about: map.get("about").cloned().unwrap_or_default(),
            built_in,
            designed_dark: r.choice("designed", &[("dark", true), ("light", false)])?,
            radius: r.num("shape.radius", 0.0..=24.0)?,
            clip_radius: r.num("shape.clip_radius", 0.0..=24.0)?,
            line: r.num("shape.line", 0.5..=4.0)?,
            gap: r.pair("shape.gap")?,
            padding: r.pair("shape.padding")?,
            shadow: r.num("shape.shadow", 0.0..=40.0)?,
            loop_fill: r.choice(
                "shape.loop",
                &[("filled", LoopFill::Filled), ("hatched", LoopFill::Hatched)],
            )?,
            body: r.num("type.body", 8.0..=32.0)?,
            small: r.num("type.small", 7.0..=24.0)?,
            heading: r.num("type.heading", 10.0..=48.0)?,
            numerals: r.num("type.numerals", 8.0..=48.0)?,
            font: r.font("type.font")?,
            numerals_font: r.font("type.numerals_font")?,
            wave: r.choice(
                "waveform.style",
                &[
                    ("filled", WaveStyle::Filled),
                    ("outline", WaveStyle::Outline),
                    ("line", WaveStyle::Line),
                    ("trace", WaveStyle::Trace),
                ],
            )?,
            wave_weight: r.num("waveform.weight", 0.5..=4.0)?,
            texture: r.choice(
                "texture.kind",
                &[("none", Texture::None), ("grain", Texture::Grain)],
            )?,
            glow: if r.yes("glow.on")? {
                Some(r.num("glow.radius", 0.0..=40.0)?)
            } else {
                None
            },
            dark: r.colours("dark")?,
            light: r.colours("light")?,
        })
    }

    /// A theme from a user's file. `based_on = <id>` takes whatever it
    /// leaves out from that built-in theme (Valve Glow if not given).
    pub(crate) fn from_file_text(id: &str, text: &str) -> Result<Theme, String> {
        let own = parse(text)?;
        let base_id = own.get("based_on").map_or(BUILT_IN[0].0, String::as_str);
        let base_text = BUILT_IN
            .iter()
            .find(|(b, _)| *b == base_id)
            .map(|(_, t)| *t)
            .ok_or_else(|| format!("`based_on = {base_id}`: no built-in theme has that name"))?;
        let mut map = parse(base_text)?;
        map.extend(own);
        Theme::from_map(id, &map, false)
    }

    pub(crate) fn colours(&self, dark: bool) -> &Colours {
        if dark { &self.dark } else { &self.light }
    }

    /// Egui's style for one variant.
    pub(crate) fn style(&self, dark: bool) -> egui::Style {
        let c = self.colours(dark);
        let mut style = egui::Style {
            visuals: self.visuals(dark),
            spacing: egui::style::Spacing {
                item_spacing: self.gap,
                button_padding: self.padding,
                ..Default::default()
            },
            text_styles: [
                (TextStyle::Small, self.font(self.small)),
                (TextStyle::Body, self.font(self.body)),
                (TextStyle::Button, self.font(self.body)),
                (TextStyle::Heading, self.font(self.heading)),
                (TextStyle::Monospace, FontId::monospace(self.body - 0.5)),
            ]
            .into(),
            ..Default::default()
        };
        // Keep the text cursor visible on every lane colour.
        style.visuals.text_cursor.stroke.color = c.text;
        style
    }

    fn visuals(&self, dark: bool) -> egui::Visuals {
        let c = self.colours(dark);
        let mut v = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let r = CornerRadius::same(self.radius.round() as u8);
        let line = Stroke::new(self.line, c.line);
        let hover = mix(c.raised, c.accent, 0.18);
        let press = mix(c.raised, c.accent, 0.32);
        let w = &mut v.widgets;
        for (wv, bg, stroke, fg) in [
            (&mut w.noninteractive, c.panel, line, 1.0),
            (&mut w.inactive, c.raised, line, 1.0),
            (&mut w.hovered, hover, Stroke::new(self.line, c.accent), 1.5),
            (&mut w.active, press, Stroke::new(self.line, c.accent), 2.0),
            (&mut w.open, c.raised, line, 1.0),
        ] {
            wv.bg_fill = bg;
            wv.weak_bg_fill = bg;
            wv.bg_stroke = stroke;
            wv.fg_stroke = Stroke::new(fg, c.text);
            wv.corner_radius = r;
        }
        v.selection.bg_fill = c.selection;
        v.selection.stroke = Stroke::new(1.0, c.text);
        v.hyperlink_color = c.accent;
        v.weak_text_color = Some(c.text_dim);
        v.faint_bg_color = c.raised;
        v.extreme_bg_color = c.lane;
        v.code_bg_color = c.raised;
        v.warn_fg_color = c.warn;
        v.error_fg_color = c.error;
        v.panel_fill = c.panel;
        v.window_fill = c.panel;
        v.window_stroke = line;
        v.window_corner_radius = CornerRadius::same((self.radius * 1.5).round() as u8);
        v.menu_corner_radius = r;
        let blur = self.shadow.max(6.0).round() as u8;
        v.window_shadow = Shadow {
            offset: [0, (blur / 3) as i8],
            blur,
            spread: 0,
            color: c.shadow,
        };
        v.popup_shadow = Shadow {
            offset: [0, (blur / 4) as i8],
            blur: blur / 2,
            spread: 0,
            color: c.shadow,
        };
        v
    }

    /// The font for readouts such as the position counter.
    pub(crate) fn numerals_font(&self) -> FontId {
        FontId::new(self.numerals, family(&self.numerals_font))
    }

    /// The theme's text font at `size`.
    pub(crate) fn font(&self, size: f32) -> FontId {
        FontId::new(size, family(&self.font))
    }

    /// The same theme in egui's own faces, for the frame before the
    /// bundled ones are loaded.
    fn in_default_fonts(&self) -> Theme {
        Theme {
            font: DEFAULT.into(),
            numerals_font: DEFAULT_MONO.into(),
            ..self.clone()
        }
    }
}

/// The active theme, as the hand-drawn parts see it.
#[derive(Clone)]
pub(crate) struct Look {
    pub(crate) theme: Arc<Theme>,
    pub(crate) dark: bool,
}

impl Look {
    pub(crate) fn c(&self) -> &Colours {
        self.theme.colours(self.dark)
    }
}

impl std::ops::Deref for Look {
    type Target = Theme;
    fn deref(&self) -> &Theme {
        &self.theme
    }
}

fn data_id() -> egui::Id {
    egui::Id::unique("daw theme")
}

/// The theme in use, and whether its dark colours are showing.
pub(crate) fn look(ctx: &egui::Context) -> Look {
    let theme = ctx
        .data(|d| d.get_temp::<Arc<Theme>>(data_id()))
        .unwrap_or_else(|| Arc::new(BUILT_INS[0].in_default_fonts()));
    Look {
        theme,
        dark: ctx.theme() == egui::Theme::Dark,
    }
}

/// Which of a theme's colours to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The variant the theme was designed in.
    Designed,
    /// Follow the computer's light or dark setting.
    System,
    Dark,
    Light,
}

impl Mode {
    pub(crate) const ALL: [Mode; 4] = [Mode::Designed, Mode::System, Mode::Dark, Mode::Light];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Designed => "As designed",
            Mode::System => "Follow the computer",
            Mode::Dark => "Dark",
            Mode::Light => "Light",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Mode::Designed => "designed",
            Mode::System => "system",
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
}

/// The themes on offer, the one chosen, and the theme window.
pub(crate) struct Themes {
    /// Built in first, then the user's.
    pub(crate) all: Vec<Arc<Theme>>,
    /// User theme files that could not be read, and why.
    pub(crate) problems: Vec<(String, String)>,
    pub(crate) chosen: String,
    pub(crate) mode: Mode,
    pub(crate) open: bool,
    /// `theme.txt`, unless testing.
    path: Option<PathBuf>,
    /// The user's theme files, unless testing.
    dir: Option<PathBuf>,
    /// What egui was last given, so it is only set again on a change.
    applied: Option<(String, Mode)>,
    /// The bundled faces have been handed to egui.
    fonts_loaded: bool,
    /// Said in the window after copying or reloading.
    pub(crate) note: Option<String>,
}

impl Themes {
    /// Built in themes only, for tests, or with the user's settings and
    /// themes folder under `dir`.
    pub(crate) fn new(dir: Option<&Path>) -> Themes {
        let mut t = Themes {
            all: BUILT_INS.clone(),
            problems: Vec::new(),
            chosen: BUILT_IN[0].0.to_string(),
            mode: Mode::Designed,
            open: false,
            path: dir.map(|d| d.join("theme.txt")),
            dir: dir.map(|d| d.join("themes")),
            applied: None,
            fonts_loaded: false,
            note: None,
        };
        if let Some(text) = t
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            t.read_settings(&text);
        }
        t.load_user_themes();
        t
    }

    fn read_settings(&mut self, text: &str) {
        for line in text.lines() {
            match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("theme", v)) => self.chosen = v.to_string(),
                Some(("mode", v)) => {
                    if let Some(m) = Mode::ALL.into_iter().find(|m| m.id() == v) {
                        self.mode = m;
                    }
                }
                _ => {}
            }
        }
    }

    fn settings_text(&self) -> String {
        format!("theme = {}\nmode = {}\n", self.chosen, self.mode.id())
    }

    fn save(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let result = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, self.settings_text())?;
            std::fs::rename(tmp, path)
        })();
        if let Err(e) = result {
            self.note = Some(format!("Could not save the theme choice: {e}"));
        }
    }

    /// Reads the `.theme` files in the user's themes folder.
    pub(crate) fn load_user_themes(&mut self) {
        self.all.truncate(BUILT_IN.len());
        self.problems.clear();
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "theme"))
            .collect();
        files.sort();
        for f in files {
            let id = f
                .file_stem()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned());
            let file = f
                .file_name()
                .map_or(String::new(), |s| s.to_string_lossy().into_owned());
            if self.all.iter().any(|t| t.id == id) {
                self.problems.push((
                    file,
                    "has the same file name as a built-in theme; rename it".into(),
                ));
                continue;
            }
            match std::fs::read_to_string(&f)
                .map_err(|e| e.to_string())
                .and_then(|text| Theme::from_file_text(&id, &text))
            {
                Ok(t) => self.all.push(Arc::new(t)),
                Err(e) => self.problems.push((file, e)),
            }
        }
    }

    /// The chosen theme, or the default if it is gone.
    pub(crate) fn current(&self) -> Arc<Theme> {
        (self.all.iter().find(|t| t.id == self.chosen))
            .unwrap_or(&self.all[0])
            .clone()
    }

    pub(crate) fn choose(&mut self, id: &str) {
        if self.chosen != id {
            self.chosen = id.to_string();
            self.save();
        }
    }

    pub(crate) fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.save();
        }
    }

    /// The next theme along, for the command.
    pub(crate) fn next(&mut self) {
        let i = self
            .all
            .iter()
            .position(|t| t.id == self.chosen)
            .unwrap_or(0);
        let id = self.all[(i + 1) % self.all.len()].id.clone();
        self.choose(&id);
    }

    /// Swaps between the theme's light and dark colours.
    pub(crate) fn flip(&mut self, ctx: &egui::Context) {
        let dark = ctx.theme() == egui::Theme::Dark;
        self.set_mode(if dark { Mode::Light } else { Mode::Dark });
    }

    /// Hands the chosen theme to egui, if it changed. Cheap otherwise, so
    /// it runs every frame.
    pub(crate) fn apply(&mut self, ctx: &egui::Context) {
        let want = (self.chosen.clone(), self.mode);
        if self.applied.as_ref() == Some(&want) {
            return;
        }
        // Fonts handed over now are only ready next frame, so this one
        // draws in egui's own and the theme is applied again then.
        let first = !self.fonts_loaded;
        if first {
            ctx.set_fonts(font_definitions());
            self.fonts_loaded = true;
        }
        let mut theme = self.current();
        if first {
            theme = Arc::new(theme.in_default_fonts());
        }
        ctx.set_style_of(egui::Theme::Dark, theme.style(true));
        ctx.set_style_of(egui::Theme::Light, theme.style(false));
        ctx.set_theme(match self.mode {
            Mode::Designed if theme.designed_dark => egui::ThemePreference::Dark,
            Mode::Designed => egui::ThemePreference::Light,
            Mode::System => egui::ThemePreference::System,
            Mode::Dark => egui::ThemePreference::Dark,
            Mode::Light => egui::ThemePreference::Light,
        });
        ctx.data_mut(|d| d.insert_temp(data_id(), theme));
        self.applied = (!first).then_some(want);
        ctx.request_repaint();
    }

    /// Writes a copy of theme `t` into the user's folder to edit.
    fn copy_to_edit(&mut self, t: &Theme) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let text = match BUILT_IN.iter().find(|(id, _)| *id == t.id) {
            Some((_, text)) => text.replacen(
                &format!("name = {}", t.name),
                &format!("name = {} (my copy)", t.name),
                1,
            ),
            None => match std::fs::read_to_string(dir.join(format!("{}.theme", t.id))) {
                Ok(text) => text,
                Err(e) => {
                    self.note = Some(format!("Could not read {}: {e}", t.name));
                    return;
                }
            },
        };
        let mut n = 1;
        let path = loop {
            let p = dir.join(format!(
                "{}-copy{}.theme",
                t.id,
                if n == 1 {
                    String::new()
                } else {
                    format!("-{n}")
                }
            ));
            if !p.exists() {
                break p;
            }
            n += 1;
        };
        let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text));
        self.note = Some(match written {
            Ok(()) => {
                self.load_user_themes();
                format!(
                    "Copied to {}. Edit it in any text editor, then press Reload.",
                    path.display()
                )
            }
            Err(e) => format!("Could not write {}: {e}", path.display()),
        });
    }

    /// Settings → Theme.
    pub(crate) fn window(&mut self, ctx: &egui::Context) {
        let mut open = self.open;
        let dark = ctx.theme() == egui::Theme::Dark;
        let mut pick = None;
        let mut copy = None;
        let mut reload = false;
        egui::Window::new("Theme")
            .open(&mut open)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label("How the app looks on this computer. It is never saved in the project, so everyone working on a song can use their own.");
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Colours:");
                    for m in Mode::ALL {
                        if ui.selectable_label(self.mode == m, m.label()).clicked() {
                            self.set_mode(m);
                        }
                    }
                });
                ui.separator();
                for t in &self.all {
                    let chosen = t.id == self.chosen;
                    ui.horizontal(|ui| {
                        swatches(ui, t.colours(dark));
                        let mut name = egui::RichText::new(&t.name).strong();
                        if !t.built_in {
                            name = egui::RichText::new(format!("{} (yours)", t.name)).strong();
                        }
                        let r = ui
                            .selectable_label(chosen, name)
                            .on_hover_text(&t.about);
                        if r.clicked() {
                            pick = Some(t.id.clone());
                        }
                        if self.dir.is_some()
                            && ui
                                .small_button("Copy to edit")
                                .on_hover_text("Make your own theme from this one")
                                .clicked()
                        {
                            copy = Some(t.clone());
                        }
                    });
                    if !t.about.is_empty() {
                        ui.label(egui::RichText::new(&t.about).weak().small());
                    }
                    ui.add_space(2.0);
                }
                for (file, why) in &self.problems {
                    ui.colored_label(ui.visuals().error_fg_color, format!("{file}: {why}"));
                }
                if let Some(dir) = &self.dir {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(format!("Your themes: {}", dir.display())).weak());
                        if ui.button("Reload").clicked() {
                            reload = true;
                        }
                    });
                }
                if let Some(n) = &self.note {
                    ui.label(n);
                }
            });
        self.open = open;
        if let Some(id) = pick {
            self.choose(&id);
        }
        if let Some(t) = copy {
            self.copy_to_edit(&t);
        }
        if reload {
            self.load_user_themes();
            // A reloaded file may have changed the chosen theme.
            self.applied = None;
            self.note = Some(format!(
                "{} themes of yours loaded.",
                self.all.len() - BUILT_IN.len()
            ));
        }
    }
}

/// A row of a theme's main colours.
fn swatches(ui: &mut egui::Ui, c: &Colours) {
    let cols = [c.bg, c.raised, c.text, c.accent, c.record]
        .into_iter()
        .chain(c.tracks.iter().copied().take(4));
    let n = 9.0;
    let (rect, _) = ui.allocate_exact_size(vec2(n * 12.0 + 2.0, 16.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, c.panel);
    for (i, col) in cols.enumerate() {
        let r = egui::Rect::from_min_size(
            rect.min + vec2(1.0 + i as f32 * 12.0, 2.0),
            vec2(11.0, 12.0),
        );
        p.rect_filled(r, 2.0, col);
    }
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, c.line),
        egui::StrokeKind::Inside,
    );
}

/// `a` moved `k` of the way to `b`, in sRGB like CSS, ignoring alpha.
pub(crate) fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A cheap repeatable hash, for scattering texture specks.
pub(crate) fn hash(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// WCAG contrast: the colours' relative luminance, and their ratio.
#[cfg(test)]
pub(crate) mod contrast {
    use eframe::egui::Color32;

    /// `fg` laid over the opaque `bg`, as unmultiplied sRGB.
    pub(crate) fn over(fg: Color32, bg: Color32) -> [f32; 3] {
        let [r, g, b, a] = fg.to_srgba_unmultiplied();
        let a = a as f32 / 255.0;
        let m = |f: u8, b: u8| f as f32 * a + b as f32 * (1.0 - a);
        [m(r, bg.r()), m(g, bg.g()), m(b, bg.b())]
    }

    fn luminance([r, g, b]: [f32; 3]) -> f32 {
        let lin = |c: f32| {
            let c = c / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    /// The contrast ratio of `fg` over `bg`, both laid over `base`.
    pub(crate) fn ratio(fg: Color32, bg: Color32, base: Color32) -> f32 {
        let bg = over(bg, base);
        let bg32 = Color32::from_rgb(bg[0] as u8, bg[1] as u8, bg[2] as u8);
        let (a, b) = (luminance(over(fg, bg32)), luminance(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

#[cfg(test)]
mod tests {
    use super::contrast::ratio;
    use super::*;

    #[test]
    fn every_built_in_theme_reads() {
        assert_eq!(BUILT_INS.len(), 4);
        let names: Vec<_> = BUILT_INS.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Valve Glow", "Workshop", "Vellum", "Tidal"]);
    }

    #[test]
    fn built_in_themes_set_every_key_themselves() {
        // Nothing comes from another theme by accident: each file has
        // the same keys as the first.
        let keys = |text: &str| parse(text).unwrap().into_keys().collect::<Vec<_>>();
        let first = keys(BUILT_IN[0].1);
        for (id, text) in BUILT_IN {
            assert_eq!(keys(text), first, "{id}.theme has different keys");
        }
    }

    #[test]
    fn every_bundled_face_reads_and_has_its_licence() {
        for f in &FACES {
            let data = egui::FontData::from_static(f.data);
            let licence = std::fs::read_to_string(format!(
                "{}/fonts/{}/OFL.txt",
                env!("CARGO_MANIFEST_DIR"),
                f.id
            ))
            .unwrap_or_else(|e| panic!("{}: no OFL.txt: {e}", f.id));
            assert!(
                licence.contains("SIL Open Font License, Version 1.1"),
                "{}",
                f.id
            );
            if f.weight.is_some() {
                assert!(
                    data.variation_axes().iter().any(|a| a.tag == *b"wght"),
                    "{}",
                    f.id
                );
            }
        }
    }

    #[test]
    fn the_bundled_faces_draw_once_loaded() {
        let ctx = egui::Context::default();
        let mut t = Themes::new(None);
        for _ in 0..3 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                t.apply(ui.ctx());
                ui.label("Andy · guitar 1.1.000");
            });
            out.textures_delta.clear();
        }
        let l = look(&ctx);
        assert_eq!(l.font, "barlow-condensed");
        assert!(ctx.fonts(|f| f.families().contains(&family("barlow-condensed"))));
    }

    #[test]
    fn comments_go_but_colours_stay() {
        let map =
            parse("# a note\nname = A # its name\n[dark]\nbg = #102030 # dark\nloop = #10203040\n")
                .unwrap();
        assert_eq!(map["name"], "A");
        assert_eq!(map["dark.bg"], "#102030");
        assert_eq!(map["dark.loop"], "#10203040");
    }

    #[test]
    fn colours_and_paints_parse() {
        assert_eq!(colour("#ff0000"), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(colour("#ff000080").map(|c| c.a()), Some(128));
        assert_eq!(colour("ff0000"), None);
        assert_eq!(colour("#ff00"), None);
        assert_eq!(paint("track"), Some(Paint::Track(1.0)));
        assert_eq!(paint("track 35%"), Some(Paint::Track(0.35)));
        assert_eq!(paint("track 135%"), None);
    }

    #[test]
    fn a_user_theme_takes_what_it_leaves_out_from_its_base() {
        let t = Theme::from_file_text(
            "mine",
            "name = Mine\nbased_on = vellum\n[shape]\nradius = 9\n[light]\naccent = #123456\n",
        )
        .unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(t.radius, 9.0);
        assert_eq!(t.light.accent, Color32::from_rgb(0x12, 0x34, 0x56));
        assert_eq!(t.wave, WaveStyle::Line);
        assert!(!t.built_in);
    }

    #[test]
    fn a_bad_user_theme_says_what_is_wrong() {
        let e = Theme::from_file_text("x", "[waveform]\nstyle = wobbly\n").unwrap_err();
        assert!(e.contains("waveform.style"), "{e}");
        assert!(e.contains("filled, outline, line, trace"), "{e}");
        let e = Theme::from_file_text("x", "based_on = nothing\n").unwrap_err();
        assert!(e.contains("nothing"), "{e}");
        let e = Theme::from_file_text("x", "[dark]\nbg = red\n").unwrap_err();
        assert!(e.contains("dark.bg"), "{e}");
    }

    #[test]
    fn the_choice_is_written_and_read_back() {
        let mut t = Themes::new(None);
        t.chosen = "tidal".into();
        t.mode = Mode::System;
        let mut u = Themes::new(None);
        u.read_settings(&t.settings_text());
        assert_eq!(u.chosen, "tidal");
        assert_eq!(u.mode, Mode::System);
        assert_eq!(u.current().name, "Tidal");
    }

    #[test]
    fn a_missing_choice_falls_back_to_the_default() {
        let mut t = Themes::new(None);
        t.chosen = "gone".into();
        assert_eq!(t.current().id, "valve-glow");
        t.next();
        assert_eq!(t.chosen, "workshop");
    }

    #[test]
    fn switching_reaches_egui_and_the_look() {
        let ctx = egui::Context::default();
        let mut t = Themes::new(None);
        t.apply(&ctx);
        assert_eq!(look(&ctx).id, "valve-glow");
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        t.choose("tidal");
        t.apply(&ctx);
        let l = look(&ctx);
        assert_eq!(l.id, "tidal");
        // Tidal was designed light.
        assert!(!l.dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, l.c().panel);
        t.flip(&ctx);
        t.apply(&ctx);
        assert!(look(&ctx).dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, l.theme.dark.panel);
    }

    /// The check CI runs on every theme (roadmap 2.10): WCAG AA, 4.5:1
    /// for text and 3:1 for marks that carry meaning (WCAG 1.4.11), in
    /// both variants.
    fn problems(t: &Theme) -> Vec<String> {
        let mut out = Vec::new();
        for dark in [true, false] {
            let c = t.colours(dark);
            let variant = if dark { "dark" } else { "light" };
            let mut need = |what: &str, fg: Color32, bg: Color32, min: f32| {
                let r = ratio(fg, bg, c.bg);
                if r < min {
                    out.push(format!(
                        "{} {variant}: {what} is {r:.2}:1, needs {min}:1",
                        t.name
                    ));
                }
            };
            for (name, bg) in [
                ("bg", c.bg),
                ("panel", c.panel),
                ("lane", c.lane),
                ("raised", c.raised),
                ("selection", c.selection),
            ] {
                need(&format!("text on {name}"), c.text, bg, 4.5);
            }
            for (name, bg) in [("panel", c.panel), ("raised", c.raised), ("lane", c.lane)] {
                need(&format!("dim text on {name}"), c.text_dim, bg, 4.5);
            }
            for (name, fg) in [("error", c.error), ("warn", c.warn), ("good", c.good)] {
                need(&format!("{name} text on panel"), fg, c.panel, 4.5);
            }
            need("on-accent text on accent", c.on_accent, c.accent, 4.5);
            need("on-accent text on markers", c.on_accent, c.marker, 4.5);
            need("labels on tempo changes", c.bg, c.tempo, 4.5);
            need("labels on signature changes", c.bg, c.signature, 4.5);
            need("accent on panel", c.accent, c.panel, 3.0);
            need("accent on raised", c.accent, c.raised, 3.0);
            for (name, fg) in [
                ("record", c.record),
                ("playhead", c.playhead),
                ("comment pins", c.comment),
                ("markers", c.marker),
            ] {
                need(&format!("{name} on lane"), fg, c.lane, 3.0);
            }
            need("record on raised", c.record, c.raised, 3.0);
            for (name, fg) in [
                ("low", c.meter_low),
                ("mid", c.meter_mid),
                ("high", c.meter_high),
            ] {
                need(&format!("meter {name}"), fg, c.meter_bg, 3.0);
            }
            for (i, &track) in c.tracks.iter().enumerate() {
                let fill = super::contrast::over(c.clip_fill.of(track), c.lane);
                let fill = Color32::from_rgb(fill[0] as u8, fill[1] as u8, fill[2] as u8);
                need(
                    &format!("track {} waveform on its clip", i + 1),
                    track,
                    fill,
                    3.0,
                );
                need(&format!("track {} on lane", i + 1), track, c.lane, 3.0);
                let behind = c.clip_label.unwrap_or(fill);
                need(
                    &format!("clip name on track {}", i + 1),
                    c.clip_text.of(track),
                    behind,
                    4.5,
                );
            }
        }
        out
    }

    #[test]
    fn every_theme_passes_the_contrast_check() {
        let all: Vec<String> = BUILT_INS.iter().flat_map(|t| problems(t)).collect();
        assert!(all.is_empty(), "contrast problems:\n{}", all.join("\n"));
    }

    #[test]
    fn the_contrast_check_catches_a_poor_theme() {
        let t = Theme::from_file_text("x", "[dark]\ntext_dim = #3a2f27\n").unwrap();
        let p = problems(&t);
        assert!(p.iter().any(|p| p.contains("dim text on panel")), "{p:?}");
    }

    #[test]
    fn contrast_ratios_match_the_wcag_examples() {
        let (b, w) = (Color32::BLACK, Color32::WHITE);
        assert!((ratio(b, w, w) - 21.0).abs() < 0.01);
        assert!((ratio(w, w, w) - 1.0).abs() < 0.01);
        // #777 on white is the usual just-failing grey, 4.48:1.
        assert!((ratio(Color32::from_gray(0x77), w, w) - 4.48).abs() < 0.01);
    }
}
