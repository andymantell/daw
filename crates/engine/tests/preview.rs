// SPDX-License-Identifier: GPL-3.0-or-later
//! The content browser's preview voice (roadmap 3.6): heard while
//! stopped or playing, replaced or stopped by command, and handed back to
//! be freed off the audio thread.

use daw_engine::engine::{self, Command};
use daw_engine::preview::PreviewSound;

fn sound(len: usize, looped: bool) -> Box<PreviewSound> {
    Box::new(PreviewSound {
        left: vec![0.5; len],
        right: vec![0.25; len],
        gain: 0.5,
        looped,
    })
}

#[test]
fn plays_while_stopped_then_finishes_and_is_freed() {
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::Preview(Some(sound(300, false))))
        .ok()
        .unwrap();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process(&mut l, &mut r);
    assert!(l.iter().all(|&x| x == 0.25), "{l:?}");
    assert!(r.iter().all(|&x| x == 0.125));
    assert_eq!(ctl.status().preview(), Some((256, 300)));
    eng.process(&mut l, &mut r);
    assert!(l[..44].iter().all(|&x| x == 0.25));
    assert!(l[44..].iter().all(|&x| x == 0.0));
    assert_eq!(ctl.status().preview(), None);
    // Its buffer came back to be freed here.
    assert_eq!(ctl.collect_garbage(), 1);
}

#[test]
fn loops_until_stopped_or_replaced() {
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::Preview(Some(sound(100, true))))
        .ok()
        .unwrap();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process(&mut l, &mut r);
    assert!(l.iter().all(|&x| x == 0.25));
    assert_eq!(ctl.status().preview(), Some((56, 100)));
    ctl.send(Command::Preview(Some(sound(1000, true))))
        .ok()
        .unwrap();
    eng.process(&mut l, &mut r);
    assert_eq!(ctl.status().preview(), Some((256, 1000)));
    ctl.send(Command::Preview(None)).ok().unwrap();
    eng.process(&mut l, &mut r);
    assert!(l.iter().all(|&x| x == 0.0));
    assert_eq!(ctl.status().preview(), None);
    assert_eq!(ctl.collect_garbage(), 2);
}

#[test]
fn mixes_over_playback() {
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::Play).ok().unwrap();
    ctl.send(Command::Preview(Some(sound(10_000, false))))
        .ok()
        .unwrap();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process(&mut l, &mut r);
    assert!(ctl.status().playing());
    assert!(l.iter().all(|&x| x == 0.25));
}
