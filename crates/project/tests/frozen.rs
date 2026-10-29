// SPDX-License-Identifier: GPL-3.0-or-later
//! Frozen audio in the project document (roadmap 1.3): what it records,
//! when it still fits a track, and how it merges.

use daw_project::{Clip, Effect, EffectState, Frozen, Project, ProjectDoc, Track};

const RATE: u32 = 48_000;
const ASSET: &str = "0123456789abcdef0123456789abcdef.wav";

fn track() -> Track {
    let mut t = Track::new("t1".into(), "Guitar".into());
    t.clips.push(Clip {
        id: "c1".into(),
        asset: "ffffffffffffffffffffffffffffffff.wav".into(),
        name: "Riff".into(),
        start: 96_000,
        offset: 1_000,
        len: 48_000,
        take: None,
        muted: false,
        seq: 0,
    });
    t.effects.push(Effect {
        id: "e1".into(),
        name: "Amp".into(),
        class: vec![7; 16],
        bundle: "Amp.vst3".into(),
        bypass: false,
        state: Some(EffectState {
            component: vec![1, 2, 3],
            controller: None,
        }),
    });
    t
}

fn frozen(t: &Track) -> Frozen {
    let mut f = t.freeze_shape(RATE);
    f.asset = ASSET.into();
    f.print = t.freeze_print(RATE);
    f
}

fn project(t: Track) -> Project {
    Project {
        tracks: vec![t],
        ..Project::default()
    }
}

#[test]
fn frozen_audio_covers_the_clips_and_a_tail() {
    let f = track().freeze_shape(RATE);
    // A second before the clip, two after it.
    assert_eq!(f.start, 48_000);
    assert_eq!(f.len, 48_000 + 48_000 + 2 * 48_000);
    assert_eq!(f.chain, vec![("e1".to_string(), false)]);
    assert_eq!(f.clips.len(), 1);
}

#[test]
fn frozen_audio_is_saved_with_the_track() {
    let mut t = track();
    t.frozen = Some(frozen(&t));
    let p = project(t);
    let mut d = ProjectDoc::new(1);
    d.set(&p);
    let back = ProjectDoc::from_bytes(&d.to_bytes(), 2).unwrap().get();
    assert_eq!(back, p);
}

#[test]
fn a_project_from_before_frozen_audio_gains_no_entry() {
    let mut d = ProjectDoc::new(1);
    d.set(&project(track()));
    assert!(!d.set(&d.get()), "writing it back changes nothing");
}

#[test]
fn frozen_audio_naming_another_file_is_ignored() {
    for bad in ["../../secret.wav", "Audio/x.wav", ""] {
        let mut t = track();
        let mut f = frozen(&t);
        f.asset = bad.into();
        t.frozen = Some(f);
        let mut d = ProjectDoc::new(1);
        d.set(&project(t));
        assert_eq!(d.get().tracks[0].frozen, None, "{bad:?}");
    }
}

#[test]
fn the_fingerprint_follows_clips_and_effects_but_not_the_fader() {
    let t = track();
    let print = t.freeze_print(RATE);
    let mut u = t.clone();
    u.gain_db = -6.0;
    u.pan = 0.5;
    u.mute = true;
    assert_eq!(u.freeze_print(RATE), print);

    let changes: [fn(&mut Track); 5] = [
        |t| t.clips[0].start += 1,
        |t| t.clips[0].muted = true,
        |t| t.effects[0].bypass = true,
        |t| t.effects[0].state.as_mut().unwrap().component[0] = 9,
        |t| t.effects.push(t.effects[0].clone()),
    ];
    for change in changes {
        let mut u = t.clone();
        change(&mut u);
        assert_ne!(u.freeze_print(RATE), print);
    }
}

#[test]
fn frozen_audio_fits_until_the_clips_or_the_chain_change() {
    let t = track();
    let f = frozen(&t);
    assert!(f.fits(&t, RATE));

    // The settings of effects aren't compared, nor the fader.
    let mut u = t.clone();
    u.effects[0].state = None;
    u.gain_db = 3.0;
    assert!(f.fits(&u, RATE));

    let changes: [fn(&mut Track); 4] = [
        |t| t.clips[0].start += 4_800,
        |t| t.clips[0].len -= 100,
        |t| t.effects[0].bypass = true,
        |t| t.effects.clear(),
    ];
    for change in changes {
        let mut u = t.clone();
        change(&mut u);
        assert!(!f.fits(&u, RATE));
    }
}

#[test]
fn frozen_audio_still_fits_after_a_trip_through_another_rate() {
    let t = track();
    let f = frozen(&t);
    let scale = |v: i64, from: u32, to: u32| (v as f64 * to as f64 / from as f64).round() as i64;
    let mut u = t.clone();
    for c in &mut u.clips {
        for v in [&mut c.start, &mut c.offset, &mut c.len] {
            *v = scale(scale(*v, RATE, 44_100), 44_100, RATE);
        }
    }
    assert!(f.fits(&u, RATE));
    let mut at_44 = t.clone();
    for c in &mut at_44.clips {
        for v in [&mut c.start, &mut c.offset, &mut c.len] {
            *v = scale(*v, RATE, 44_100);
        }
    }
    assert!(f.fits(&at_44, 44_100));
}

#[test]
fn two_renders_saved_at_once_never_mix() {
    let mut a = ProjectDoc::new(1);
    a.set(&project(track()));
    let mut b = ProjectDoc::from_bytes(&a.to_bytes(), 2).unwrap();

    let mut ta = track();
    let mut fa = frozen(&ta);
    fa.asset = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.wav".into();
    fa.len += 1;
    ta.frozen = Some(fa.clone());
    a.set(&project(ta));
    let mut tb = track();
    let mut fb = frozen(&tb);
    fb.asset = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.wav".into();
    fb.start += 1;
    tb.frozen = Some(fb.clone());
    b.set(&project(tb));

    a.merge(&b.to_bytes()).unwrap();
    b.merge(&a.to_bytes()).unwrap();
    let got = a.get().tracks[0].frozen.clone().unwrap();
    assert_eq!(b.get().tracks[0].frozen.as_ref(), Some(&got));
    assert!(got == fa || got == fb, "one render or the other, whole");
}
