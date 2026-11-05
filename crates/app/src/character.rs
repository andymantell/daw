// SPDX-License-Identifier: GPL-3.0-or-later
//! Each built-in synth's own visual character (UI polish step 5,
//! `docs/plans/ui-polish.md` section 5: "the nine read as nine
//! instruments"), laid over the panel kit's shared frame.
//!
//! A character is a handful of choices, each drawn by the kit from the
//! theme's tokens, so it fits every theme and stays a hint:
//!
//! - a **tint** on the front plate: the theme's plate colour moved a
//!   little toward the instrument's own, so a silver box, a navy desk and
//!   a cream panel sit in one rack and still look like the theme's gear;
//! - an **accent** for the knob arcs, the lettering and the module
//!   headings, one colour for dark plates and one for light;
//! - a **knob style**: the cap the kit draws, a skirted knob with a
//!   pointer, a wedge pointer, a dot on a flat cap, or a flat ring;
//! - the **lettering** of the name plate: plain capitals, wide-spaced
//!   capitals, lower-case, a readout face, or a filled badge;
//! - a **band** across the top of the plate: none, a stripe in the
//!   accent, a row of section colours, a pinstripe pair, or a row of
//!   patch points.
//!
//! The choices evoke each synth's era and type; none copies a real
//! product's livery, badge or lettering. The character in force is kept
//! in egui's frame data while a unit draws, so the kit's widgets pick
//! it up without the panels naming it.

use daw_instruments::Kind;
use eframe::egui::{self, Color32};

use crate::theme::{self, Look};

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
    /// The plate's own colour, and how far the theme's plate moves
    /// toward it (0 keeps the theme's plate).
    pub tint: Color32,
    pub tint_strength: f32,
    /// The accent on a dark plate, and on a light one.
    pub accent_dark: Color32,
    pub accent_light: Color32,
    pub knob: KnobStyle,
    pub lettering: Lettering,
    pub band: Band,
}

impl Character {
    /// The kit's plain look: the theme as it is.
    pub(crate) const PLAIN: Character = Character {
        tint: Color32::TRANSPARENT,
        tint_strength: 0.0,
        accent_dark: Color32::TRANSPARENT,
        accent_light: Color32::TRANSPARENT,
        knob: KnobStyle::Cap,
        lettering: Lettering::Caps,
        band: Band::None,
    };

    /// The character of a built-in instrument; the plain look for one
    /// without its own.
    pub(crate) fn of(kind: Option<Kind>) -> Character {
        let rgb = Color32::from_rgb;
        match kind {
            // The three-oscillator ladder monosynth: a black desk with
            // cream skirted knobs and wide lettering, a warm walnut
            // stripe standing in for its end cheeks.
            Some(Kind::ModelMono) => Character {
                tint: rgb(0x1c, 0x1a, 0x18),
                tint_strength: 0.35,
                accent_dark: rgb(0xe8, 0xdc, 0xc0),
                accent_light: rgb(0x5a, 0x4a, 0x30),
                knob: KnobStyle::Skirted,
                lettering: Lettering::Wide,
                band: Band::Stripe,
            },
            // The bass line box: silver, small black knobs with a dot,
            // a pinstripe, and a red-orange accent.
            Some(Kind::AcidThree) => Character {
                tint: rgb(0xb9, 0xbc, 0xc0),
                tint_strength: 0.3,
                accent_dark: rgb(0xe0, 0x52, 0x3c),
                accent_light: rgb(0xb5, 0x3a, 0x26),
                knob: KnobStyle::Dot,
                lettering: Lettering::Caps,
                band: Band::Pinstripe,
            },
            // The one-oscillator monosynth: cool grey, coral accent,
            // a lower-case wordmark.
            Some(Kind::OhOne) => Character {
                tint: rgb(0x6d, 0x75, 0x80),
                tint_strength: 0.25,
                accent_dark: rgb(0xe4, 0x65, 0x4a),
                accent_light: rgb(0xb4, 0x43, 0x2c),
                knob: KnobStyle::Dot,
                lettering: Lettering::Lower,
                band: Band::Stripe,
            },
            // The six-voice polysynth: dark grey, with a row of section
            // colours as on a colour-coded panel.
            Some(Kind::June) => Character {
                tint: rgb(0x3b, 0x3b, 0x3f),
                tint_strength: 0.3,
                accent_dark: rgb(0x6f, 0xb3, 0xe8),
                accent_light: rgb(0x2a, 0x5f, 0xa0),
                knob: KnobStyle::Cap,
                lettering: Lettering::Caps,
                band: Band::Sections,
            },
            // The eight-voice two-channel polysynth: navy, gold
            // lettering spaced wide, wedge pointers.
            Some(Kind::Eighty) => Character {
                tint: rgb(0x1f, 0x2f, 0x4f),
                tint_strength: 0.35,
                accent_dark: rgb(0xf2, 0xb8, 0x4b),
                accent_light: rgb(0x8a, 0x5a, 0x10),
                knob: KnobStyle::Pointer,
                lettering: Lettering::Wide,
                band: Band::Stripe,
            },
            // The two-filter polysynth: cream, maroon, a skirted knob,
            // its name on a badge.
            Some(Kind::Oracle) => Character {
                tint: rgb(0xd8, 0xc9, 0xa8),
                tint_strength: 0.3,
                accent_dark: rgb(0xd8, 0x6a, 0x84),
                accent_light: rgb(0x7a, 0x25, 0x38),
                knob: KnobStyle::Skirted,
                lettering: Lettering::Badge,
                band: Band::Pinstripe,
            },
            // The semi-modular: olive, pale yellow stencilled lettering,
            // pointers, and patch points along the top.
            Some(Kind::PatchTwenty) => Character {
                tint: rgb(0x5f, 0x6b, 0x5a),
                tint_strength: 0.35,
                accent_dark: rgb(0xe8, 0xd2, 0x7a),
                accent_light: rgb(0x6a, 0x5a, 0x10),
                knob: KnobStyle::Pointer,
                lettering: Lettering::Readout,
                band: Band::Jacks,
            },
            // The eight-bit wavetable synth: blue, pale cyan readout
            // lettering, flat data knobs.
            Some(Kind::BlueWave) => Character {
                tint: rgb(0x2a, 0x4a, 0x8a),
                tint_strength: 0.35,
                accent_dark: rgb(0x8f, 0xd8, 0xff),
                accent_light: rgb(0x0b, 0x5f, 0x8a),
                knob: KnobStyle::Flat,
                lettering: Lettering::Readout,
                band: Band::Stripe,
            },
            // The six-operator FM synth: near-black, teal, membrane-panel
            // badge, flat knobs.
            Some(Kind::LuckySeven) => Character {
                tint: rgb(0x1a, 0x16, 0x14),
                tint_strength: 0.35,
                accent_dark: rgb(0x4f, 0xd1, 0xc5),
                accent_light: rgb(0x0e, 0x6f, 0x66),
                knob: KnobStyle::Flat,
                lettering: Lettering::Badge,
                band: Band::Pinstripe,
            },
            _ => Character::PLAIN,
        }
    }

    /// The plate's colour in `look`: the theme's, moved toward the tint.
    /// The tint is first brought to the theme plate's lightness (with a
    /// little drift toward its own), so a silver box in a dark theme is a
    /// lighter grey, not a grey slab, and a navy desk in a light theme
    /// is a pale blue: the hue changes, the theme's contrast stays.
    pub(crate) fn plate(&self, look: &Look) -> Color32 {
        if self.tint_strength == 0.0 {
            return look.c().plate;
        }
        let plate = look.c().plate;
        let y = |c: Color32| {
            0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())
        };
        let (yp, yt) = (y(plate), y(self.tint).max(1.0));
        let target = yp + (yt - yp) * 0.15;
        let k = target / yt;
        let ch = |v: u8| (f32::from(v) * k).round().clamp(0.0, 255.0) as u8;
        let tint = Color32::from_rgb(ch(self.tint.r()), ch(self.tint.g()), ch(self.tint.b()));
        theme::mix(plate, tint, self.tint_strength)
    }

    /// Whether the plate reads as dark, so the accent for a dark plate
    /// is the one to use.
    fn plate_is_dark(&self, look: &Look) -> bool {
        let p = self.plate(look);
        let lum = 0.2126 * f32::from(p.r()) + 0.7152 * f32::from(p.g()) + 0.0722 * f32::from(p.b());
        lum < 128.0
    }

    /// The accent in `look`: the theme's, for the plain character; else
    /// the one for a dark or a light plate, moved toward the theme's
    /// text colour until it reads on the plate.
    pub(crate) fn accent(&self, look: &Look) -> Color32 {
        if self.tint_strength == 0.0 {
            return look.c().accent;
        }
        let plate = self.plate(look);
        let mut a = if self.plate_is_dark(look) {
            self.accent_dark
        } else {
            self.accent_light
        };
        for _ in 0..8 {
            if theme::contrast::ratio(a, plate, plate) >= 3.0 {
                break;
            }
            a = theme::mix(a, look.c().text, 0.2);
        }
        a
    }

    /// The name plate's lettering colour: the theme's badge colour for
    /// the plain character, the accent otherwise.
    pub(crate) fn badge(&self, look: &Look) -> Color32 {
        if self.tint_strength == 0.0 {
            look.c().badge
        } else {
            self.accent(look)
        }
    }

    /// The section colours a [`Band::Sections`] band shows.
    pub(crate) fn sections(&self, look: &Look) -> [Color32; 4] {
        let rgb = Color32::from_rgb;
        let dark = self.plate_is_dark(look);
        if dark {
            [
                rgb(0xe0, 0x52, 0x3c),
                rgb(0xf2, 0xc0, 0x4b),
                rgb(0x7f, 0xc8, 0x6a),
                rgb(0x6f, 0xb3, 0xe8),
            ]
        } else {
            [
                rgb(0xb5, 0x3a, 0x26),
                rgb(0xa0, 0x78, 0x10),
                rgb(0x3f, 0x7d, 0x30),
                rgb(0x2a, 0x5f, 0xa0),
            ]
        }
    }

    /// The character's name, as an assistant reads it.
    pub(crate) fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "knob": format!("{:?}", self.knob).to_lowercase(),
            "lettering": format!("{:?}", self.lettering).to_lowercase(),
            "band": format!("{:?}", self.band).to_lowercase(),
            "tinted": self.tint_strength > 0.0,
        })
    }
}

fn id() -> egui::Id {
    egui::Id::unique("panel character")
}

/// Sets the character the kit draws with until [`clear`].
pub(crate) fn set(ctx: &egui::Context, ch: Character) {
    ctx.data_mut(|d| d.insert_temp(id(), ch));
}

pub(crate) fn clear(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Character>(id()));
}

/// The character in force: the plain look outside a unit.
pub(crate) fn current(ctx: &egui::Context) -> Character {
    ctx.data(|d| d.get_temp::<Character>(id()))
        .unwrap_or(Character::PLAIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::contrast;

    /// Every character's lettering and accent read on its plate, in
    /// every theme, dark and light.
    #[test]
    fn every_character_reads_on_its_plate_in_every_theme() {
        let mut problems = Vec::new();
        for t in theme::built_in_themes() {
            for dark in [true, false] {
                let look = Look {
                    theme: t.clone(),
                    dark,
                };
                for k in Kind::ALL {
                    let ch = Character::of(Some(k));
                    let plate = ch.plate(&look);
                    let mut check = |what: &str, fg: Color32, least: f32| {
                        let r = contrast::ratio(fg, plate, plate);
                        if r < least {
                            problems.push(format!("{} {dark} {k:?}: {what} on plate {r:.2}", t.id));
                        }
                    };
                    check("text", look.c().text, 4.5);
                    check("dim text", look.c().text_dim, 3.0);
                    check("accent", ch.accent(&look), 3.0);
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
