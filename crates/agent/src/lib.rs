// SPDX-License-Identifier: GPL-3.0-or-later
//! The agent interface's server (roadmap 2.11, ARCHITECTURE §7a): a
//! Model Context Protocol server on this computer's loopback address,
//! so an AI assistant can read and drive the app through its tools.
//!
//! It speaks Streamable HTTP, for both eras of the protocol: the
//! session-based revisions up to 2025-11-25, which open with
//! `initialize`, and the stateless revision 2026-07-28, where every
//! request carries its version and client in `_meta`. Every request
//! must carry the token shown in the app, as `Authorization: Bearer`.
//! A request from a web page (an `Origin` that isn't this computer) is
//! refused, as the specification asks, against DNS rebinding.
//!
//! The server knows nothing of the app. It hands each tool call to the
//! UI thread as an [`Event`] and waits for the answer; the app decides
//! what each assistant may do.

pub mod mcp;
pub mod server;

pub use mcp::{Client, Event, ToolInfo};
pub use server::{DEFAULT_PORT, Server};
