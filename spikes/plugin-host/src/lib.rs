// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 0.1 B: hosting VST 3 plugins from Rust.

// VST 3 enum types are signed on some platforms and unsigned on others,
// so casts that are no-ops here are needed elsewhere.
#![allow(clippy::unnecessary_cast)]

pub mod checks;
pub mod com;
#[cfg(windows)]
pub mod editor_win;
pub mod oop;
pub mod oop_bench;
pub mod plugin;
