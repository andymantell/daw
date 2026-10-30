// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin hosting (roadmap 1.3): VST 3 effects as track inserts.
//!
//! Built directly on the `vst3` Rust bindings (MIT/Apache-2.0), following
//! spike 0.1 B. [`scan`] finds plugins, each module in a child process so
//! a bad one cannot crash the app. [`plugin::instantiate`] loads one and
//! splits it into a UI side and an audio side; the audio side is an
//! engine [`Insert`](daw_engine::graph::Insert). [`host`] runs plugins
//! in a helper process instead, so a crash cannot take the app down,
//! and [`presets`] saves and finds their settings. [`compat`] finds the
//! newer version of a plugin that stands in for an older one, and
//! [`check`] tries each plugin out for the compatibility list.
//!
//! Plugins in the open CLAP format ([`clap`]) are hosted beside VST 3
//! ones: a [`Library`] is a loaded plugin file of either format, and
//! [`Plugin`] and [`Processor`] cover both.

// VST 3 enum types are signed on some platforms and unsigned on others,
// so casts that are no-ops here are needed elsewhere.
#![allow(clippy::unnecessary_cast)]

pub mod check;
pub mod clap;
#[cfg(windows)]
pub mod clap_editor_win;
pub mod com;
pub mod compat;
#[cfg(windows)]
pub mod editor_win;
pub mod host;
pub mod module;
pub mod plugin;
pub mod presets;
pub mod proto;
pub mod scan;

pub use host::{Host, Sandbox};
pub use module::{ClassInfo, Module, cid_from_hex, cid_to_hex};
pub use plugin::{
    Bus, Buses, Layout, Library, LocalPlugin, LocalProcessor, Param, Plugin, PluginState, Processor,
    instantiate, instantiate_with,
};
