// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoding real files through the system's media framework. The
//! fixtures are two seconds at 12 frames a second, 64 by 48, each frame
//! one colour: red is ten times the frame's number. The Ogg file needs
//! only the framework's base decoders; the MP4 (H.264 and AAC) is what
//! most cameras and editors make.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_video::{Error, Frame, Reader, player::Player, thumbs::Thumbs};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Which frame a fixture frame is, from its colour.
fn counted(f: &Frame) -> u64 {
    let mid = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
    (f.rgba[mid] as f64 / 10.0).round() as u64
}

/// The file the system here should decode.
fn file() -> PathBuf {
    if cfg!(windows) {
        fixture("counting.mp4")
    } else {
        fixture("counting.ogv")
    }
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn reads_the_picture_and_goes_to_any_frame() {
    let mut r = Reader::open(&file(), (32, 32)).unwrap();
    let i = *r.info();
    assert_eq!((i.width, i.height), (64, 48));
    assert_eq!(i.fps(), 12.0);
    assert!((i.seconds - 2.0).abs() < 0.1, "{}", i.seconds);
    assert_eq!(i.frames(), 24);
    assert_eq!(r.size(), (32, 24), "scaled to fit, keeping the shape");

    // From the start, frame after frame.
    for n in 0..5 {
        let f = r.next_frame().unwrap().unwrap();
        assert_eq!((f.width, f.height), (32, 24));
        assert_eq!(counted(&f), n, "{f:?}");
        assert!((f.time - i.frame_time(n)).abs() < 1e-3, "{f:?}");
    }
    // Then straight to frames out of order, including mid-frame times.
    for n in [17, 3, 23, 9, 0] {
        let t = i.frame_time(n) + 0.5 / 12.0;
        let f = r.frame_at(t).unwrap().unwrap();
        assert_eq!(counted(&f), n, "at {t}: {f:?}");
    }
    // The end.
    r.seek(i.frame_time(23)).unwrap();
    assert!(r.next_frame().unwrap().is_some());
    assert_eq!(r.next_frame().unwrap(), None);
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn thumbnails_cover_every_frame_of_a_short_video() {
    let t = Thumbs::start(&file());
    let until = Instant::now() + Duration::from_secs(30);
    while t.progress() < 1.0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(t.error(), None);
    assert_eq!(t.progress(), 1.0);
    for n in 0..24 {
        let (f, exact) = t.get(n).unwrap();
        assert!(exact, "frame {n}");
        assert_eq!(counted(&f), n);
        assert_eq!(f.height, daw_video::thumbs::HEIGHT.min(48));
    }
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn the_player_follows_the_playhead() {
    let p = Player::new();
    let wait_for = |n: u64| {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let (Some(f), _) = p.frame()
                && counted(&f) == n
            {
                return;
            }
            assert!(
                Instant::now() < until,
                "frame {n} never came: {:?}",
                p.frame()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let path = file();
    // Playing: one frame after another.
    for n in 0..6 {
        p.show(Some(&path), n as f64 / 12.0 + 0.01);
        wait_for(n);
    }
    // A jump back, and one forward.
    p.show(Some(&path), 20.0 / 12.0);
    wait_for(20);
    p.show(Some(&path), 2.0 / 12.0);
    wait_for(2);
    assert_eq!(p.info().unwrap().frames(), 24);
    p.show(None, 0.0);
    let until = Instant::now() + Duration::from_secs(10);
    while p.frame().0.is_some() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_file_that_isnt_video_says_so() {
    let not = fixture("../decode.rs");
    let e = Reader::open(&not, (32, 32)).err().unwrap();
    assert!(
        matches!(e, Error::Unreadable(_) | Error::NoFramework(_)),
        "{e:?}"
    );
    let gone = fixture("not-there.mp4");
    assert!(Reader::open(&gone, (32, 32)).is_err());
}
