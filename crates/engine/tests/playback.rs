// SPDX-License-Identifier: GPL-3.0-or-later
//! Playback, mixing and transport, checked sample by sample.

#[path = "common/rt_check.rs"]
mod rt_check;

use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, CompileError, Mix, Session, TrackSpec};
use daw_engine::metronome::Metronome;
use daw_engine::time::{Frames, PPQ, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

fn mono(samples: Vec<f32>) -> Arc<AudioData> {
    Arc::new(AudioData {
        channels: vec![samples],
        rate: RATE,
    })
}

fn stereo(l: Vec<f32>, r: Vec<f32>) -> Arc<AudioData> {
    Arc::new(AudioData {
        channels: vec![l, r],
        rate: RATE,
    })
}

/// Audio whose sample `i` is `(i + 1) / 1e6`, so any output sample says
/// exactly which input sample produced it.
fn counting(n: usize) -> Arc<AudioData> {
    mono((0..n).map(|i| (i + 1) as f32 / 1e6).collect())
}

fn render(s: &Session, frames: usize, block: usize) -> [Vec<f32>; 2] {
    engine::render_offline(s.compile(block).unwrap(), Frames(0), frames, block)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn clip_starts_on_its_exact_frame() {
    let mut s = session();
    // Bar 2 at 120 BPM in 4/4 starts at 2 s = 96,000 frames.
    let bar2 = s.tempo.bar_start(2);
    let mut t = TrackSpec::new(vec![ClipSpec::new(bar2, mono(vec![1.0; 10]))]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    // A block size that does not divide 96,000.
    let [l, r] = render(&s, 96_020, 333);
    assert_eq!(l[95_999], 0.0);
    assert_eq!(l[96_000], 1.0);
    assert_eq!(l[96_009], 1.0);
    assert_eq!(l[96_010], 0.0);
    assert!(
        r.iter().all(|&x| x == 0.0),
        "hard left leaves nothing right"
    );
}

#[test]
fn pan_laws() {
    let mut s = session();
    s.tracks.push(TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        mono(vec![1.0; 4]),
    )]));
    let [l, r] = render(&s, 4, 64);
    // Mono in the centre: −3 dB each side.
    assert!(close(l[0], std::f32::consts::FRAC_1_SQRT_2));
    assert!(close(r[0], std::f32::consts::FRAC_1_SQRT_2));

    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        stereo(vec![0.5; 4], vec![0.25; 4]),
    )]);
    t.mix.pan = 0.5;
    s.tracks.push(t);
    let [l, r] = render(&s, 4, 64);
    // Stereo balance: the centre is untouched; half right halves the left.
    assert!(close(l[0], 0.25));
    assert!(close(r[0], 0.25));
}

#[test]
fn gain_offset_and_length() {
    let mut s = session();
    let mut c = ClipSpec::new(Ticks(0), counting(100));
    c.offset = 10;
    c.len = Some(5);
    c.gain_db = -6.0;
    let mut t = TrackSpec::new(vec![c]);
    t.mix.gain_db = 6.0;
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let [l, _] = render(&s, 8, 64);
    // −6 dB and +6 dB cancel to within float rounding.
    assert!(close(l[0], 11e-6));
    assert!(close(l[4], 15e-6));
    assert_eq!(l[5], 0.0);
}

#[test]
fn mute_and_solo() {
    let mut s = session();
    for v in [0.1, 0.2, 0.4] {
        let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), mono(vec![v; 4]))]);
        t.mix.pan = -1.0;
        s.tracks.push(t);
    }
    s.tracks[0].mix.mute = true;
    let [l, _] = render(&s, 4, 64);
    assert!(close(l[0], 0.6), "muted track 1: {}", l[0]);
    s.tracks[1].mix.solo = true;
    let [l, _] = render(&s, 4, 64);
    assert!(close(l[0], 0.2), "only the soloed track: {}", l[0]);
}

#[test]
fn loop_is_sample_accurate() {
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), counting(10_000))]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::SetGraph(Box::new(s.compile(256).unwrap())))
        .ok()
        .unwrap();
    ctl.send(Command::SetLoop(Some((Frames(1000), Frames(1300)))))
        .ok()
        .unwrap();
    ctl.send(Command::Seek(Frames(900))).ok().unwrap();
    ctl.send(Command::Play).ok().unwrap();
    // Blocks of 97 frames: loop boundaries land mid-block.
    let mut out = Vec::new();
    for _ in 0..20 {
        let (mut l, mut r) = (vec![0.0; 97], vec![0.0; 97]);
        eng.process(&mut l, &mut r);
        out.extend(l);
    }
    // Expected input sample for each output sample.
    let mut want = Vec::new();
    let mut p = 900;
    while want.len() < out.len() {
        want.push(p);
        p += 1;
        if p == 1300 {
            p = 1000;
        }
    }
    for (i, (&o, &w)) in out.iter().zip(&want).enumerate() {
        let got = (o * 1e6).round() as i64 - 1;
        assert_eq!(got, w, "output sample {i}");
    }
    assert_eq!(ctl.status().position(), Frames(*want.last().unwrap() + 1));
}

#[test]
fn stop_and_seek() {
    let mut s = session();
    s.tracks.push(TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        mono(vec![1.0; 1000]),
    )]));
    let (mut ctl, mut eng) = engine::new(128);
    ctl.send(Command::SetGraph(Box::new(s.compile(128).unwrap())))
        .ok()
        .unwrap();
    let (mut l, mut r) = (vec![0.0; 128], vec![0.0; 128]);
    eng.process(&mut l, &mut r);
    assert!(l.iter().all(|&x| x == 0.0), "silent until play");
    assert_eq!(ctl.status().position(), Frames(0));
    ctl.send(Command::Play).ok().unwrap();
    eng.process(&mut l, &mut r);
    assert!(ctl.status().playing());
    assert_eq!(ctl.status().position(), Frames(128));
    ctl.send(Command::Stop).ok().unwrap();
    ctl.send(Command::Seek(Frames(PPQ))).ok().unwrap();
    eng.process(&mut l, &mut r);
    assert!(!ctl.status().playing());
    assert_eq!(ctl.status().position(), Frames(PPQ));
}

#[test]
fn fader_moves_ramp_instead_of_jumping() {
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), mono(vec![1.0; 4096]))]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::SetGraph(Box::new(s.compile(256).unwrap())))
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process(&mut l, &mut r);
    let mix = Mix {
        mute: true,
        pan: -1.0,
        ..Mix::default()
    };
    ctl.send(Command::SetMix { track: 0, mix }).ok().unwrap();
    let last = l[255];
    eng.process(&mut l, &mut r);
    // From 1.0 down to 0 across the block, never jumping.
    let mut prev = last;
    for &x in &l {
        assert!((prev - x).abs() < 0.01, "jump from {prev} to {x}");
        prev = x;
    }
    assert_eq!(l[255], 0.0);
}

#[test]
fn meters_report_peaks_and_reset() {
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        mono(vec![0.0, -0.8, 0.3, 0.0]),
    )]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    s.master_db = -6.0;
    let (mut ctl, mut eng) = engine::new(64);
    ctl.send(Command::SetGraph(Box::new(s.compile(64).unwrap())))
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    eng.process(&mut l, &mut r);
    let (tl, tr) = ctl.status().take_track_peak(0);
    assert!(close(tl, 0.8) && tr == 0.0);
    let (ml, _) = ctl.status().take_master_peak();
    assert!((ml - 0.8 * 0.501).abs() < 1e-3, "master after −6 dB: {ml}");
    assert_eq!(ctl.status().take_track_peak(0), (0.0, 0.0), "reset on read");
}

#[test]
fn compile_rejects_bad_audio() {
    let mut s = session();
    s.tracks.push(TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        Arc::new(AudioData {
            channels: vec![vec![0.0; 4]],
            rate: 44_100,
        }),
    )]));
    assert_eq!(
        s.compile(64).err(),
        Some(CompileError::RateMismatch { track: 0 })
    );
}

#[test]
fn audio_thread_never_allocates() {
    let mut s = session();
    for i in 0..16 {
        s.tracks.push(TrackSpec::new(vec![
            ClipSpec::new(Ticks(i * 100), counting(50_000)),
            ClipSpec::new(Ticks(i * 100 + PPQ * 8), counting(50_000)),
        ]));
    }
    let (mut ctl, mut eng) = engine::new(512);
    ctl.send(Command::SetMetronome(Metronome {
        enabled: true,
        subdivision: 4,
        ..Metronome::default()
    }))
    .ok()
    .unwrap();
    let first = s.compile(512).unwrap();
    s.tracks.pop();
    let second = s.compile(512).unwrap();
    ctl.send(Command::SetGraph(Box::new(first))).ok().unwrap();
    ctl.send(Command::SetLoop(Some((Frames(1000), Frames(90_000)))))
        .ok()
        .unwrap();
    ctl.send(Command::Play).ok().unwrap();

    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let (half_tx, half_rx) = std::sync::mpsc::channel::<()>();
    let audio = std::thread::spawn(move || {
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        for half in 0..2 {
            let rt = rt_check::RtGuard::enter();
            for _ in 0..200 {
                eng.process(&mut l, &mut r);
            }
            drop(rt);
            if half == 0 {
                half_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }
        }
        (rt_check::violations(), eng)
    });
    // Halfway through: a mixer change and a whole new graph.
    half_rx.recv().unwrap();
    ctl.send(Command::SetMix {
        track: 3,
        mix: Mix {
            gain_db: -12.0,
            ..Mix::default()
        },
    })
    .ok()
    .unwrap();
    ctl.send(Command::SetGraph(Box::new(second))).ok().unwrap();
    go_tx.send(()).unwrap();
    let (violations, _eng) = audio.join().unwrap();
    assert_eq!(violations, 0, "allocations on the audio thread");
    assert_eq!(
        ctl.collect_garbage(),
        1,
        "the old graph comes back to be freed"
    );
}

fn first_sound(x: &[f32], from: usize) -> Option<usize> {
    x[from..].iter().position(|&s| s != 0.0).map(|i| i + from)
}

#[test]
fn metronome_clicks_on_each_beat() {
    let s = session();
    let (mut ctl, mut eng) = engine::new(500);
    ctl.send(Command::SetGraph(Box::new(s.compile(500).unwrap())))
        .ok()
        .unwrap();
    ctl.send(Command::SetMetronome(Metronome {
        enabled: true,
        ..Metronome::default()
    }))
    .ok()
    .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let mut out = Vec::new();
    for _ in 0..100 {
        let (mut l, mut r) = (vec![0.0; 500], vec![0.0; 500]);
        eng.process(&mut l, &mut r);
        assert_eq!(l, r, "clicks are centred");
        out.extend(l);
    }
    // Each click is a sine starting at zero, so its first non-zero
    // sample is one frame after the beat.
    assert_eq!(first_sound(&out, 0), Some(1));
    assert!(
        out[2_000..24_000].iter().all(|&x| x == 0.0),
        "silence between clicks"
    );
    assert_eq!(first_sound(&out, 2_000), Some(24_001));
    // The first beat of the bar is the accented (louder) click.
    let peak = |a: usize| out[a..a + 1_000].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak(0) > peak(24_000));
}

#[test]
fn count_in_plays_clicks_then_starts_on_the_frame() {
    let mut s = session();
    let mut t = TrackSpec::new(vec![ClipSpec::new(Ticks(0), mono(vec![0.5; 100_000]))]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let (mut ctl, mut eng) = engine::new(512);
    ctl.send(Command::SetGraph(Box::new(s.compile(512).unwrap())))
        .ok()
        .unwrap();
    ctl.send(Command::CountIn { bars: 1 }).ok().unwrap();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let mut counting = Vec::new();
    for _ in 0..200 {
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        eng.process(&mut l, &mut r);
        counting.push(ctl.status().counting_in());
        left.extend(l);
        right.extend(r);
    }
    // One bar of 4/4 at 120 BPM is 96,000 frames of clicks only: the
    // right channel (where the hard-left track is silent) has clicks.
    assert_eq!(first_sound(&right, 0), Some(1));
    assert_eq!(first_sound(&right, 2_000), Some(24_001));
    assert!(
        left[..96_000].iter().all(|&x| x.abs() < 0.5),
        "no track audio yet"
    );
    // Then the track starts on exactly the right frame.
    assert!(left[95_999].abs() < 0.5);
    assert_eq!(left[96_000], 0.5);
    assert!(counting[0] && !counting[199]);
    assert_eq!(ctl.status().position(), Frames(200 * 512 - 96_000));
}

#[test]
fn count_in_uses_the_tempo_and_time_signature_where_play_starts() {
    let mut map = TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON);
    let bar3 = map.bar_start(3);
    map.add_tempo(bar3, Tempo::bpm(60)).unwrap();
    map.add_meter(bar3, TimeSignature::new(3, 4)).unwrap();
    let mut s = Session::new(map);
    let mut t = TrackSpec::new(vec![ClipSpec::new(bar3, mono(vec![0.5; 10_000]))]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let (mut ctl, mut eng) = engine::new(512);
    ctl.send(Command::SetGraph(Box::new(s.compile(512).unwrap())))
        .ok()
        .unwrap();
    // Bar 3 starts after two bars at 120: 4 seconds.
    ctl.send(Command::Seek(Frames(4 * RATE as i64)))
        .ok()
        .unwrap();
    ctl.send(Command::CountIn { bars: 1 }).ok().unwrap();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for _ in 0..300 {
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        eng.process(&mut l, &mut r);
        left.extend(l);
        right.extend(r);
    }
    // One bar of 3/4 at 60, the tempo bar 3 is played at: three clicks
    // a second apart, the first accented, then the track on the frame.
    let beat = RATE as usize;
    assert_eq!(first_sound(&right, 0), Some(1));
    assert_eq!(first_sound(&right, 2_000), Some(beat + 1));
    assert_eq!(first_sound(&right, beat + 2_000), Some(2 * beat + 1));
    let peak = |a: usize| {
        right[a..a + 1_000]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    };
    assert!(peak(0) > peak(beat));
    assert!(left[3 * beat - 1].abs() < 0.5);
    assert_eq!(left[3 * beat], 0.5);
}

#[test]
fn clicks_can_go_to_their_own_output() {
    let s = session();
    let (mut ctl, mut eng) = engine::new(256);
    ctl.send(Command::SetGraph(Box::new(s.compile(256).unwrap())))
        .ok()
        .unwrap();
    ctl.send(Command::SetMetronome(Metronome {
        enabled: true,
        separate_output: true,
        ..Metronome::default()
    }))
    .ok()
    .unwrap();
    ctl.send(Command::Play).ok().unwrap();
    let (mut l, mut r, mut c) = (vec![0.0; 256], vec![0.0; 256], vec![0.0; 256]);
    eng.process_split(&mut l, &mut r, Some(&mut c));
    assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    assert_eq!(first_sound(&c, 0), Some(1));
}
