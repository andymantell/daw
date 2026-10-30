// SPDX-License-Identifier: GPL-3.0-or-later
//! Notation engine (roadmap 3.8): tab for any string count, tuning and
//! capo, standard staff and drum staff, laid out by our own engine and drawn
//! with a music font in the standard music-font layout (SMuFL). It reads and
//! writes MusicXML 4.0, writes plain-text tab, SVG and PDF, and assigns
//! strings and frets to notes that come without them.
//!
//! The crate has its own score types (see [`score`]) and no dependency on
//! the project model; the timeline's notation lanes convert between them.
//! Built at risk on decision 0009, which is still proposed.

pub mod drums;
pub mod fretting;
pub mod glyphs;
pub mod layout;
pub mod musicxml;
pub mod render;
pub mod score;
pub mod text_tab;
pub mod tuning;
