// SPDX-License-Identifier: GPL-3.0-or-later
//! The phone remote's desktop side (ROADMAP 2.2): a control protocol
//! over the local network, pairing by a code shown as a QR image, and a
//! server that runs beside the app.
//!
//! The desktop stays in charge. A phone only sends requests
//! ([`protocol::FromPhone`]) and shows what the desktop says is true
//! ([`protocol::ToPhone`]). Nothing here touches the audio thread, and a
//! phone that disappears mid-take changes nothing: the recording was
//! started by the desktop and carries on there.
//!
//! The same protocol serves the Android app (`android/`), and later the
//! looper pages and a served web page (ROADMAP 5).

pub mod net;
pub mod pairing;
pub mod protocol;
pub mod server;

pub use pairing::{Devices, Paired, QrImage, new_code, new_token, qr_image, qr_text};
pub use protocol::{Clip, FromPhone, Marker, ToPhone, Track, VERSION};
pub use server::{Event, Server};
