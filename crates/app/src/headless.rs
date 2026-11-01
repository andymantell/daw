// SPDX-License-Identifier: GPL-3.0-or-later
//! `daw --headless` (roadmap 2.11, ARCHITECTURE §7a): the app with no
//! window and no sound, serving the agent interface, so an assistant
//! can exercise a build, and tests can drive the real program from
//! outside.
//!
//! ```text
//! daw --headless [--port N] [--access read|edit|export] [file ...]
//! ```
//!
//! The token is taken from `DAW_AGENT_TOKEN`, or made up and printed.
//! The first line printed is `listening <address>`; a second gives the
//! token when it was made up. There is no one to ask, so every
//! assistant is allowed `--access` (edit unless given). The app draws
//! its frames as usual, into nothing, and stops when killed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::App;
use crate::agent::Access;

pub(crate) const ARG: &str = "--headless";

static ON: AtomicBool = AtomicBool::new(false);

/// The app is running headless.
pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// How often frames are drawn: as a window would at 60 a second.
const FRAME: Duration = Duration::from_millis(16);
/// The size the panes are laid out at.
const SIZE: egui::Vec2 = egui::vec2(1300.0, 820.0);

#[derive(Debug, PartialEq)]
struct Options {
    port: u16,
    access: Access,
    files: Vec<String>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        port: daw_agent::DEFAULT_PORT,
        access: Access::Edit,
        files: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => {
                o.port = (it.next())
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port takes a number")?;
            }
            "--access" => {
                o.access = match it.next().map(String::as_str) {
                    Some("read") => Access::Read,
                    Some("edit") => Access::Edit,
                    Some("export") => Access::EditExport,
                    _ => return Err("--access takes read, edit or export".into()),
                };
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f => o.files.push(f.into()),
        }
    }
    Ok(o)
}

/// Runs until killed. Returns the exit code if it can't start.
pub(crate) fn run(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!(
                "{e}\nusage: daw --headless [--port N] [--access read|edit|export] [file ...]"
            );
            return 2;
        }
    };
    ON.store(true, Ordering::Relaxed);
    let (token, made) = match std::env::var("DAW_AGENT_TOKEN") {
        Ok(t) if !t.trim().is_empty() => (t.trim().to_string(), false),
        _ => (daw_remote::pairing::new_token(), true),
    };
    let ctx = egui::Context::default();
    let mut app = App::new(o.files);
    match app.assistants.serve(o.port, &token, o.access) {
        Ok(url) => println!("listening {url}"),
        Err(e) => {
            eprintln!("can't start the server: {e}");
            return 1;
        }
    }
    if made {
        println!("token {token}");
    }
    // The frame eframe would hand the app; only its tests make one
    // without a window, through this hidden constructor.
    let mut frame = eframe::Frame::_new_kittest();
    let start = Instant::now();
    loop {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
            time: Some(start.elapsed().as_secs_f64()),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        // Nothing draws the textures, so they're let go of.
        out.textures_delta.clear();
        std::thread::sleep(FRAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn options_are_read() {
        let o = parse(&args("--port 9000 --access read song.wav")).unwrap();
        assert_eq!(
            o,
            Options {
                port: 9000,
                access: Access::Read,
                files: vec!["song.wav".into()],
            }
        );
        assert_eq!(parse(&[]).unwrap().access, Access::Edit);
        assert!(parse(&args("--access all")).is_err());
        assert!(parse(&args("--port")).is_err());
        assert!(parse(&args("--loud")).is_err());
    }
}
