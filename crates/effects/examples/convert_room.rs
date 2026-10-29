// SPDX-License-Identifier: GPL-3.0-or-later
//! Makes a recorded room's file for `crates/effects/rooms/` from an
//! original response, the same way the app reads a user's own file:
//! B-format decoded to stereo, silence trimmed, at most ten seconds,
//! 48 kHz, 16-bit. Prints what it changed, for the room's `changes`.
//!
//! `cargo run -p daw-effects --example convert_room -- <original> <out.wav>`

use daw_effects::Response;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [from, to] = args.as_slice() else {
        return Err("usage: convert_room <original> <out.wav>".into());
    };
    let spec = hound::WavReader::open(from).map(|r| r.spec()).ok();
    let r = Response::from_file(from.as_ref())?;
    let r48 = r.resampled(48_000);
    r48.to_wav(to.as_ref())?;
    let mut changes = Vec::new();
    if let Some(s) = spec {
        match s.channels {
            1 => changes.push("mono made stereo".to_string()),
            4 => changes.push("B-format decoded to stereo".to_string()),
            _ => {}
        }
        if s.sample_rate != 48_000 {
            changes.push(format!("resampled from {} to 48 kHz", s.sample_rate));
        }
        if s.bits_per_sample != 16 {
            changes.push("16-bit".to_string());
        }
    }
    changes.push(format!("trimmed to {:.1} s", r48.seconds()));
    changes.push("level set".to_string());
    println!("{}", changes.join(", "));
    Ok(())
}
