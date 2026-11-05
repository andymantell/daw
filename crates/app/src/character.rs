// SPDX-License-Identifier: GPL-3.0-or-later
//! Each built-in synth's own visual character (UI polish step 5,
//! `docs/plans/ui-polish.md` section 5: "the nine read as nine
//! instruments"), laid over the panel kit's shared frame so each reads
//! as its own piece of hardware in the one rack.
//!
//! A character is:
//!
//! - its own **palette**: the front plate, its lettering and dim
//!   lettering, an accent, a screen's glass and ink, the knob caps and
//!   their pointers, and side cheeks where the hardware would have them;
//!   the theme still draws the rack around it;
//! - a **knob style**: the cap the kit draws, a skirted knob with a
//!   pointer, a wedge pointer, a dot on a flat cap, or a flat ring;
//! - a **switch style**: the kit's segments, lit buttons with a lamp, or
//!   a slide switch;
//! - a **module style**: the kit's framed block, bare lines on the
//!   plate, a filled heading band, or a thin boxed frame;
//! - a **screen style**: a scope, a lamp display with a glow, or a
//!   liquid-crystal panel;
//! - the **lettering** of the name plate, and a **band** across the top
//!   of the plate.
//!
//! The choices evoke each synth's era and type; none copies a real
//! product's livery, badge or lettering. The character in force is kept
//! in egui's frame data while a unit draws, and the theme's colours are
//! swapped for the character's for everything drawn inside it, so the
//! kit's widgets and the panels' own drawings pick it up without naming
//! it.

use std::sync::Arc;

use daw_instruments::Kind;
use eframe::egui::{self, Color32};

use crate::theme::{self, Colours, Look};

/// How a knob is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KnobStyle {
    /// The kit's own: a cap with a pointer line and an arc around it.
    Cap,
    /// A wide skirt with tick marks under a smaller cap, as on a 1970s
    /// desk.
    Skirted,
    /// A wedge pointer across the cap, as on a modular or an early
    /// polysynth.
    Pointer,
    /// A flat cap with a dot near the rim: 1980s plastic.
    Dot,
    /// A thin ring and a line, no cap: a digital unit's data knob.
    Flat,
}

/// How a switch is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SwitchStyle {
    /// The kit's own: a row of segments, one chosen.
    Segments,
    /// A button per choice with a lamp that lights when chosen.
    Lit,
    /// A slide switch: a slot with a block at the chosen position.
    Slide,
}

/// How a module is framed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModuleStyle {
    /// The kit's own: a filled block with a heading and a rule.
    Framed,
    /// No block: the heading and a rule straight on the plate.
    Lines,
    /// The heading in a filled band across the top of the block.
    Band,
    /// A thin frame in the accent, the heading set into its top edge.
    Boxed,
}

/// How a screen is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenStyle {
    /// The kit's own: dark glass, a faint grid, a traced line.
    Scope,
    /// Dark glass with the trace glowing like lamps.
    Lamp,
    /// A liquid-crystal panel: pale glass, dark ink, a dot grid.
    Lcd,
}

/// How the name plate is lettered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lettering {
    /// Capitals in the theme's heading face.
    Caps,
    /// Capitals spaced wide.
    Wide,
    /// Lower case, as a script-like wordmark.
    Lower,
    /// Capitals in the theme's readout face.
    Readout,
    /// Capitals cut out of a filled badge.
    Badge,
}

/// A band across the top of the plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Band {
    None,
    /// A stripe in the accent.
    Stripe,
    /// Four section colours in a row, as a colour-coded panel.
    Sections,
    /// Two thin lines.
    Pinstripe,
    /// A row of patch points.
    Jacks,
}

/// A synth's character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Character {
    /// The plain kit look: the theme's own colours and widgets.
    pub plain: bool,
    pub plate: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    /// Lettering on the accent.
    pub on_accent: Color32,
    /// A screen's glass and its ink.
    pub screen_bg: Color32,
    pub screen_ink: Color32,
    /// A knob's cap and its pointer.
    pub cap: Color32,
    pub pointer: Color32,
    /// Side cheeks at the plate's ends, where the hardware had wood.
    pub cheeks: Option<Color32>,
    pub knob: KnobStyle,
    pub switch: SwitchStyle,
    pub module: ModuleStyle,
    pub screen: ScreenStyle,
    pub lettering: Lettering,
    pub band: Band,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Character {
    /// The kit's plain look: the theme as it is.
    pub(crate) const PLAIN: Character = Character {
        plain: true,
        plate: Color32::TRANSPARENT,
        text: Color32::TRANSPARENT,
        text_dim: Color32::TRANSPARENT,
        accent: Color32::TRANSPARENT,
        on_accent: Color32::TRANSPARENT,
        screen_bg: Color32::TRANSPARENT,
        screen_ink: Color32::TRANSPARENT,
        cap: Color32::TRANSPARENT,
        pointer: Color32::TRANSPARENT,
        cheeks: None,
        knob: KnobStyle::Cap,
        switch: SwitchStyle::Segments,
        module: ModuleStyle::Framed,
        screen: ScreenStyle::Scope,
        lettering: Lettering::Caps,
        band: Band::None,
    };

    /// The character of a built-in instrument; the plain look for one
    /// without its own.
    pub(crate) fn of(kind: Option<Kind>) -> Character {
        let own = Character {
            plain: false,
            ..Character::PLAIN
        };
        match kind {
            // The three-oscillator ladder monosynth: a black desk with
            // walnut cheeks, cream skirted knobs, brass lettering spaced
            // wide, slide switches and amber lamps.
            Some(Kind::ModelMono) => Character {
                plate: rgb(0x1b, 0x1a, 0x1d),
                text: rgb(0xef, 0xe6, 0xd2),
                text_dim: rgb(0xb7, 0xad, 0x98),
                accent: rgb(0xe4, 0xc9, 0x8b),
                on_accent: rgb(0x1b, 0x1a, 0x1d),
                screen_bg: rgb(0x0d, 0x0c, 0x0e),
                screen_ink: rgb(0xf0, 0xb4, 0x57),
                cap: rgb(0xef, 0xe6, 0xd2),
                pointer: rgb(0x1b, 0x1a, 0x1d),
                cheeks: Some(rgb(0x6e, 0x44, 0x28)),
                knob: KnobStyle::Skirted,
                switch: SwitchStyle::Slide,
                module: ModuleStyle::Lines,
                screen: ScreenStyle::Lamp,
                lettering: Lettering::Wide,
                band: Band::None,
                ..own
            },
            // The bass line box: silver, black knobs with a white dot,
            // red lettering on black heading bands, lamp buttons, a
            // pinstripe.
            Some(Kind::AcidThree) => Character {
                plate: rgb(0xc8, 0xcb, 0xcf),
                text: rgb(0x15, 0x16, 0x1a),
                text_dim: rgb(0x4d, 0x51, 0x58),
                accent: rgb(0xb8, 0x34, 0x20),
                on_accent: rgb(0xff, 0xff, 0xff),
                screen_bg: rgb(0x1a, 0x1b, 0x1e),
                screen_ink: rgb(0xff, 0x6b, 0x4a),
                cap: rgb(0x22, 0x22, 0x26),
                pointer: rgb(0xf2, 0xf2, 0xf2),
                knob: KnobStyle::Dot,
                switch: SwitchStyle::Lit,
                module: ModuleStyle::Band,
                screen: ScreenStyle::Lamp,
                lettering: Lettering::Caps,
                band: Band::Pinstripe,
                ..own
            },
            // The one-oscillator monosynth: grey-blue plastic, coral
            // accent, white knobs, slide switches, a lower-case wordmark.
            Some(Kind::OhOne) => Character {
                plate: rgb(0x4e, 0x58, 0x66),
                text: rgb(0xf1, 0xf4, 0xf7),
                text_dim: rgb(0xc2, 0xc9, 0xd1),
                accent: rgb(0xff, 0xa3, 0x85),
                on_accent: rgb(0x2a, 0x12, 0x0a),
                screen_bg: rgb(0x11, 0x16, 0x1c),
                screen_ink: rgb(0x7f, 0xd1, 0xff),
                cap: rgb(0xe9, 0xec, 0xef),
                pointer: rgb(0xe8, 0x50, 0x3a),
                knob: KnobStyle::Dot,
                switch: SwitchStyle::Slide,
                module: ModuleStyle::Framed,
                screen: ScreenStyle::Scope,
                lettering: Lettering::Lower,
                band: Band::Stripe,
                ..own
            },
            // The six-voice polysynth: dark grey, a row of section
            // colours, white caps, lamp buttons, bare module lines.
            Some(Kind::June) => Character {
                plate: rgb(0x37, 0x39, 0x3f),
                text: rgb(0xf2, 0xf2, 0xf2),
                text_dim: rgb(0xb9, 0xbc, 0xc4),
                accent: rgb(0x5a, 0xa9, 0xe6),
                on_accent: rgb(0x0e, 0x11, 0x16),
                screen_bg: rgb(0x14, 0x15, 0x19),
                screen_ink: rgb(0xff, 0xb3, 0x47),
                cap: rgb(0xe6, 0xe6, 0xe6),
                pointer: rgb(0x37, 0x39, 0x3f),
                knob: KnobStyle::Cap,
                switch: SwitchStyle::Lit,
                module: ModuleStyle::Lines,
                screen: ScreenStyle::Scope,
                lettering: Lettering::Caps,
                band: Band::Sections,
                ..own
            },
            // The eight-voice two-channel polysynth: navy with dark wood
            // cheeks, gold lettering spaced wide, cream wedge pointers,
            // heading bands, amber lamps.
            Some(Kind::Eighty) => Character {
                plate: rgb(0x24, 0x33, 0x52),
                text: rgb(0xf6, 0xe9, 0xc9),
                text_dim: rgb(0xb9, 0xad, 0x90),
                accent: rgb(0xf0, 0xb6, 0x4a),
                on_accent: rgb(0x24, 0x33, 0x52),
                screen_bg: rgb(0x0f, 0x15, 0x22),
                screen_ink: rgb(0xff, 0xd2, 0x7a),
                cap: rgb(0xf6, 0xe9, 0xc9),
                pointer: rgb(0x24, 0x33, 0x52),
                cheeks: Some(rgb(0x5a, 0x3a, 0x22)),
                knob: KnobStyle::Pointer,
                switch: SwitchStyle::Slide,
                module: ModuleStyle::Band,
                screen: ScreenStyle::Lamp,
                lettering: Lettering::Wide,
                band: Band::Stripe,
                ..own
            },
            // The two-filter polysynth: cream, maroon, black skirted
            // knobs, red lamps, its name on a badge.
            Some(Kind::Oracle) => Character {
                plate: rgb(0xe8, 0xdf, 0xc6),
                text: rgb(0x2d, 0x24, 0x20),
                text_dim: rgb(0x6b, 0x5d, 0x53),
                accent: rgb(0x8b, 0x27, 0x40),
                on_accent: rgb(0xff, 0xf7, 0xea),
                screen_bg: rgb(0x1f, 0x1a, 0x18),
                screen_ink: rgb(0xff, 0x4d, 0x4d),
                cap: rgb(0x2d, 0x24, 0x20),
                pointer: rgb(0xe8, 0xdf, 0xc6),
                knob: KnobStyle::Skirted,
                switch: SwitchStyle::Lit,
                module: ModuleStyle::Framed,
                screen: ScreenStyle::Lamp,
                lettering: Lettering::Badge,
                band: Band::Pinstripe,
                ..own
            },
            // The semi-modular: olive, pale yellow stencilled lettering,
            // cream pointers, slide switches, a green scope, patch points
            // along the top.
            Some(Kind::PatchTwenty) => Character {
                plate: rgb(0x5f, 0x6a, 0x5c),
                text: rgb(0xf1, 0xe9, 0xc6),
                text_dim: rgb(0xcc, 0xc6, 0xa6),
                accent: rgb(0xe8, 0xd2, 0x7a),
                on_accent: rgb(0x2b, 0x2f, 0x27),
                screen_bg: rgb(0x0c, 0x11, 0x0c),
                screen_ink: rgb(0x8c, 0xff, 0x7a),
                cap: rgb(0xe8, 0xe3, 0xcf),
                pointer: rgb(0x2b, 0x2f, 0x27),
                knob: KnobStyle::Pointer,
                switch: SwitchStyle::Slide,
                module: ModuleStyle::Lines,
                screen: ScreenStyle::Scope,
                lettering: Lettering::Readout,
                band: Band::Jacks,
                ..own
            },
            // The eight-bit wavetable synth: blue, cyan readout
            // lettering, flat data knobs, a liquid-crystal panel, boxed
            // modules.
            Some(Kind::BlueWave) => Character {
                plate: rgb(0x2b, 0x4c, 0x8e),
                text: rgb(0xee, 0xf3, 0xff),
                text_dim: rgb(0xb6, 0xc4, 0xe6),
                accent: rgb(0x7f, 0xe0, 0xff),
                on_accent: rgb(0x0e, 0x1f, 0x3f),
                screen_bg: rgb(0x9f, 0xb5, 0x9a),
                screen_ink: rgb(0x1b, 0x2a, 0x1a),
                cap: rgb(0xdf, 0xe6, 0xf5),
                pointer: rgb(0x2b, 0x4c, 0x8e),
                knob: KnobStyle::Flat,
                switch: SwitchStyle::Lit,
                module: ModuleStyle::Boxed,
                screen: ScreenStyle::Lcd,
                lettering: Lettering::Readout,
                band: Band::Stripe,
                ..own
            },
            // The six-operator FM synth: near-black, teal lines, a
            // membrane-panel badge, flat knobs, a liquid-crystal panel.
            Some(Kind::LuckySeven) => Character {
                plate: rgb(0x1c, 0x1b, 0x1e),
                text: rgb(0xe9, 0xec, 0xef),
                text_dim: rgb(0x9a, 0xa0, 0xa8),
                accent: rgb(0x3f, 0xd3, 0xc4),
                on_accent: rgb(0x0b, 0x15, 0x14),
                screen_bg: rgb(0xa6, 0xb4, 0xa0),
                screen_ink: rgb(0x1b, 0x23, 0x1a),
                cap: rgb(0x2c, 0x2b, 0x30),
                pointer: rgb(0xe9, 0xec, 0xef),
                knob: KnobStyle::Flat,
                switch: SwitchStyle::Lit,
                module: ModuleStyle::Boxed,
                screen: ScreenStyle::Lcd,
                lettering: Lettering::Badge,
                band: Band::Pinstripe,
                ..own
            },
            _ => Character::PLAIN,
        }
    }

    /// Whether the plate is dark, for the shades drawn on it.
    fn dark(&self) -> bool {
        let p = self.plate;
        0.2126 * f32::from(p.r()) + 0.7152 * f32::from(p.g()) + 0.0722 * f32::from(p.b()) < 128.0
    }

    /// The theme's colours with the character's own in place of those
    /// drawn inside a unit: the plate and its edges, lettering, accent,
    /// screens, knob caps, buttons and lines.
    pub(crate) fn colours(&self, base: &Colours) -> Colours {
        if self.plain {
            return base.clone();
        }
        let dark = self.dark();
        let plate = self.plate;
        let lum = 0.2126 * f32::from(plate.r())
            + 0.7152 * f32::from(plate.g())
            + 0.0722 * f32::from(plate.b());
        // Buttons stand a shade off the plate: lighter on a black one,
        // darker on a mid or light one.
        let raised = if lum < 70.0 {
            theme::mix(plate, Color32::WHITE, 0.1)
        } else if dark {
            theme::mix(plate, Color32::BLACK, 0.22)
        } else {
            theme::mix(plate, Color32::WHITE, 0.5)
        };
        Colours {
            panel: plate,
            raised,
            line: theme::mix(self.text_dim, plate, 0.55),
            text: self.text,
            text_dim: self.text_dim,
            accent: self.accent,
            on_accent: self.on_accent,
            selection: theme::mix(self.accent, plate, 0.45),
            meter_bg: self.screen_bg,
            desk: plate,
            fader_slot: theme::mix(plate, Color32::BLACK, 0.45),
            fader_cap: self.cap,
            fader_line: self.pointer,
            shadow: Color32::from_black_alpha(if dark { 160 } else { 70 }),
            plate,
            plate_edge: theme::mix(plate, Color32::WHITE, if dark { 0.18 } else { 0.6 }),
            plate_shadow: theme::mix(plate, Color32::BLACK, if dark { 0.5 } else { 0.35 }),
            screw: theme::mix(self.text_dim, plate, 0.3),
            badge: self.accent,
            ..base.clone()
        }
    }

    /// The accent in `look`: the character's own while it is in force,
    /// the theme's for the plain character.
    pub(crate) fn accent(&self, look: &Look) -> Color32 {
        look.c().accent
    }

    /// The name plate's lettering colour.
    pub(crate) fn badge(&self, look: &Look) -> Color32 {
        look.c().badge
    }

    /// The plate's colour in `look`.
    pub(crate) fn plate(&self, look: &Look) -> Color32 {
        look.c().plate
    }

    /// The section colours a [`Band::Sections`] band shows.
    pub(crate) fn sections(&self) -> [Color32; 4] {
        [
            rgb(0xe0, 0x52, 0x3c),
            rgb(0xf2, 0xc0, 0x4b),
            rgb(0x7f, 0xc8, 0x6a),
            rgb(0x6f, 0xb3, 0xe8),
        ]
    }

    /// The character, as an assistant reads it.
    pub(crate) fn describe(&self) -> serde_json::Value {
        let lower = |d: &dyn std::fmt::Debug| format!("{d:?}").to_lowercase();
        serde_json::json!({
            "knob": lower(&self.knob),
            "switch": lower(&self.switch),
            "module": lower(&self.module),
            "screen": lower(&self.screen),
            "lettering": lower(&self.lettering),
            "band": lower(&self.band),
            "plate": hex(self.plate),
            "accent": hex(self.accent),
            "cheeks": self.cheeks.is_some(),
        })
    }
}

/// A colour as `#rrggbb`.
fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn id() -> egui::Id {
    egui::Id::unique("panel character")
}

fn colours_id() -> egui::Id {
    egui::Id::unique("panel character colours")
}

/// Sets the character the kit draws with until [`clear`]: the character
/// itself, and the theme's colours with its own swapped in, which
/// [`theme::look`] returns while it is in force.
pub(crate) fn set(ctx: &egui::Context, ch: Character) {
    let own = if ch.plain {
        None
    } else {
        let look = theme::look(ctx);
        Some(Arc::new(ch.colours(look.theme.colours(look.dark))))
    };
    ctx.data_mut(|d| {
        d.insert_temp(id(), ch);
        match own {
            Some(c) => {
                d.insert_temp(colours_id(), c);
            }
            None => d.remove::<Arc<Colours>>(colours_id()),
        }
    });
}

pub(crate) fn clear(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove::<Character>(id());
        d.remove::<Arc<Colours>>(colours_id());
    });
}

/// The character in force: the plain look outside a unit.
pub(crate) fn current(ctx: &egui::Context) -> Character {
    ctx.data(|d| d.get_temp::<Character>(id()))
        .unwrap_or(Character::PLAIN)
}

/// The colours in force, if a character with its own is set.
pub(crate) fn colours(ctx: &egui::Context) -> Option<Arc<Colours>> {
    ctx.data(|d| d.get_temp::<Arc<Colours>>(colours_id()))
}

/// Egui's widgets inside the unit in the character's colours: its text,
/// its buttons raised from its plate, its accent on hover.
pub(crate) fn style(ui: &mut egui::Ui) {
    let Some(c) = colours(ui.ctx()) else {
        return;
    };
    let v = ui.visuals_mut();
    v.override_text_color = Some(c.text);
    let line = egui::Stroke::new(1.0, c.line);
    let hover = theme::mix(c.raised, c.accent, 0.25);
    let press = theme::mix(c.raised, c.accent, 0.4);
    let w = &mut v.widgets;
    for (wv, bg, stroke) in [
        (&mut w.noninteractive, c.panel, line),
        (&mut w.inactive, c.raised, line),
        (&mut w.hovered, hover, egui::Stroke::new(1.0, c.accent)),
        (&mut w.active, press, egui::Stroke::new(1.0, c.accent)),
        (&mut w.open, c.raised, line),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = stroke;
        wv.fg_stroke.color = c.text;
    }
    v.selection.bg_fill = c.selection;
    v.selection.stroke = egui::Stroke::new(1.0, c.text);
    v.weak_text_color = Some(c.text_dim);
    v.faint_bg_color = c.raised;
    v.extreme_bg_color = c.meter_bg;
    v.panel_fill = c.panel;
    v.window_fill = c.panel;
    v.hyperlink_color = c.accent;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::contrast;

    /// Every character's lettering, dim lettering, accent and screen ink
    /// read on what they are drawn on, whatever the theme around them.
    #[test]
    fn every_character_reads_on_its_own_plate() {
        let mut problems = Vec::new();
        for k in Kind::ALL {
            let ch = Character::of(Some(k));
            if ch.plain {
                continue;
            }
            let mut check = |what: &str, fg: Color32, bg: Color32, least: f32| {
                let r = contrast::ratio(fg, bg, bg);
                if r < least {
                    problems.push(format!("{k:?}: {what} {r:.2}"));
                }
            };
            check("text on plate", ch.text, ch.plate, 4.5);
            check("dim text on plate", ch.text_dim, ch.plate, 3.0);
            check("accent on plate", ch.accent, ch.plate, 3.0);
            check("lettering on accent", ch.on_accent, ch.accent, 3.0);
            check("ink on screen", ch.screen_ink, ch.screen_bg, 3.0);
            check("pointer on cap", ch.pointer, ch.cap, 3.0);
            let theme = theme::built_in_themes()[0].clone();
            let c = ch.colours(theme.colours(true));
            check("text on button", c.text, c.raised, 4.5);
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn the_plain_character_keeps_the_themes_colours() {
        let theme = theme::built_in_themes()[0].clone();
        let base = theme.colours(true);
        let c = Character::PLAIN.colours(base);
        assert_eq!(c.plate, base.plate);
        assert_eq!(c.text, base.text);
    }
}
