// SPDX-License-Identifier: GPL-3.0-or-later
//! Code points of the music-font glyphs we draw, from the standard
//! music-font layout (SMuFL). Names follow the layout's glyph names.

pub const G_CLEF: char = '\u{E050}';
pub const G_CLEF_8VB: char = '\u{E052}';
pub const F_CLEF: char = '\u{E062}';
pub const F_CLEF_8VB: char = '\u{E064}';
pub const PERCUSSION_CLEF: char = '\u{E069}';
pub const TAB_CLEF_6: char = '\u{E06D}';
pub const TAB_CLEF_4: char = '\u{E06E}';

/// Time signature digit 0; digits 1 to 9 follow it.
pub const TIME_SIG_0: char = '\u{E080}';

pub const NOTEHEAD_WHOLE: char = '\u{E0A2}';
pub const NOTEHEAD_HALF: char = '\u{E0A3}';
pub const NOTEHEAD_BLACK: char = '\u{E0A4}';
pub const NOTEHEAD_X_WHOLE: char = '\u{E0A7}';
pub const NOTEHEAD_X_HALF: char = '\u{E0A8}';
pub const NOTEHEAD_X_BLACK: char = '\u{E0A9}';
pub const NOTEHEAD_CIRCLE_X: char = '\u{E0B3}';
pub const NOTEHEAD_SLASHED_BLACK: char = '\u{E0CF}';
pub const NOTEHEAD_DIAMOND_BLACK: char = '\u{E0DB}';
pub const NOTEHEAD_PAREN_LEFT: char = '\u{E0F5}';
pub const NOTEHEAD_PAREN_RIGHT: char = '\u{E0F6}';

pub const AUGMENTATION_DOT: char = '\u{E1E7}';
pub const TREMOLO_3: char = '\u{E222}';
pub const GRACE_SLASH_STEM_UP: char = '\u{E564}';

/// 8th, 16th, 32nd and 64th flags, stem up then stem down.
pub const FLAGS_UP: [char; 4] = ['\u{E240}', '\u{E242}', '\u{E244}', '\u{E246}'];
pub const FLAGS_DOWN: [char; 4] = ['\u{E241}', '\u{E243}', '\u{E245}', '\u{E247}'];

pub const ACCIDENTAL_FLAT: char = '\u{E260}';
pub const ACCIDENTAL_NATURAL: char = '\u{E261}';
pub const ACCIDENTAL_SHARP: char = '\u{E262}';

pub const ACCENT_ABOVE: char = '\u{E4A0}';
pub const ACCENT_BELOW: char = '\u{E4A1}';

pub const REST_WHOLE: char = '\u{E4E3}';
pub const REST_HALF: char = '\u{E4E4}';
pub const REST_QUARTER: char = '\u{E4E5}';
/// 8th, 16th, 32nd and 64th rests.
pub const RESTS_SHORT: [char; 4] = ['\u{E4E6}', '\u{E4E7}', '\u{E4E8}', '\u{E4E9}'];

pub const MUTE_CLOSED: char = '\u{E5E5}';
pub const MUTE_OPEN: char = '\u{E5E7}';
pub const CHOKE_CYMBAL: char = '\u{E805}';
pub const STRUM_UP: char = '\u{E846}';
pub const STRUM_DOWN: char = '\u{E847}';
pub const VIBRATO_SEGMENT: char = '\u{EAB2}';

/// Every glyph above, for the font check.
pub const ALL: &[char] = &[
    G_CLEF,
    G_CLEF_8VB,
    F_CLEF,
    F_CLEF_8VB,
    PERCUSSION_CLEF,
    TAB_CLEF_6,
    TAB_CLEF_4,
    TIME_SIG_0,
    NOTEHEAD_WHOLE,
    NOTEHEAD_HALF,
    NOTEHEAD_BLACK,
    NOTEHEAD_X_WHOLE,
    NOTEHEAD_X_HALF,
    NOTEHEAD_X_BLACK,
    NOTEHEAD_CIRCLE_X,
    NOTEHEAD_SLASHED_BLACK,
    NOTEHEAD_DIAMOND_BLACK,
    NOTEHEAD_PAREN_LEFT,
    NOTEHEAD_PAREN_RIGHT,
    AUGMENTATION_DOT,
    TREMOLO_3,
    GRACE_SLASH_STEM_UP,
    FLAGS_UP[0],
    FLAGS_UP[1],
    FLAGS_UP[2],
    FLAGS_UP[3],
    FLAGS_DOWN[0],
    FLAGS_DOWN[1],
    FLAGS_DOWN[2],
    FLAGS_DOWN[3],
    ACCIDENTAL_FLAT,
    ACCIDENTAL_NATURAL,
    ACCIDENTAL_SHARP,
    ACCENT_ABOVE,
    ACCENT_BELOW,
    REST_WHOLE,
    REST_HALF,
    REST_QUARTER,
    RESTS_SHORT[0],
    RESTS_SHORT[1],
    RESTS_SHORT[2],
    RESTS_SHORT[3],
    MUTE_CLOSED,
    MUTE_OPEN,
    CHOKE_CYMBAL,
    STRUM_UP,
    STRUM_DOWN,
    VIBRATO_SEGMENT,
];

pub fn time_sig_digit(d: u32) -> char {
    char::from_u32(TIME_SIG_0 as u32 + d.min(9)).unwrap_or(TIME_SIG_0)
}
