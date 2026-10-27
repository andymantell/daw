// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin hosting (roadmap 1.3): VST 3 effects as track inserts.
//!
//! Built directly on the `vst3` Rust bindings (MIT/Apache-2.0), following
//! spike 0.1 B. [`scan`] finds plugins, each module in a child process so
//! a bad one cannot crash the app. [`plugin::instantiate`] loads one and
//! splits it into a UI side and an audio side; the audio side is an
//! engine [`Insert`](daw_engine::graph::Insert).

// VST 3 enum types are signed on some platforms and unsigned on others,
// so casts that are no-ops here are needed elsewhere.
#![allow(clippy::unnecessary_cast)]

pub mod com;
#[cfg(windows)]
pub mod editor_win;
pub mod module;
pub mod plugin;
pub mod scan;

pub use module::{ClassInfo, Module, cid_from_hex, cid_to_hex};
pub use plugin::{Param, Plugin, PluginState, Processor, instantiate};
