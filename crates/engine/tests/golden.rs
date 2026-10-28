// SPDX-License-Identifier: GPL-3.0-or-later
//! Golden-render tests (roadmap 1.9): small sessions rendered offline and
//! compared with reference renders kept in `tests/golden/`.
//!
//! The other engine tests check one behaviour each, sample by sample.
//! These check that whole mixes still sound the same after a change, so
//! an unintended change anywhere in the render path shows up. The
//! references are short 24-bit WAV files and can be listened to.
//!
//! When a change is meant to alter the sound, listen to the new renders
//! and then rewrite the references with
//! `DAW_BLESS=1 cargo test -p daw-engine --test golden`.
//! A failing test writes what it rendered to
//! `target/tmp/golden/<name>.wav` for comparing by ear.

use std::path::PathBuf;
use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::import::resample;
use daw_engine::metronome::Metronome;
use daw_engine::time::{Frames, PPQ, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 48_000;
/// 240 BPM, so a short render still holds a few beats.
const BPM: u64 = 240;
/// Frames in a beat at [`BPM`].
const BEAT: usize = 12_000;
/// Full scale of the references' 24-bit samples.
const FULL: f32 = 8_388_607.0;

/// How far a sample may move from the reference: about −100 dBFS, which
/// allows for 24-bit rounding and the last bits of floating-point maths
/// differing between platforms, but nothing anyone could hear.
const EXACT: f32 = 1e-5;
/// The phase vocoder's FFTs take different paths on different CPUs, so
/// stretched audio gets a looser bound (about −80 dBFS).
const STRETCHED: f32 = 1e-4;

// ---------------------------------------------------------------------------
// Test signals. Made from arithmetic only, so they are the same everywhere.

/// A plucked note: a sine at `hz` with a fast attack and an exponential
/// decay, `seconds` long.
fn pluck(hz: f32, seconds: f32, rate: u32) -> Vec<f32> {
    let n = (rate as f32 * seconds) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let env = (t / 0.002).min(1.0) * (-t * 6.0).exp();
            0.6 * env * (std::f32::consts::TAU * hz * t).sin()
        })
        .collect()
}

/// White noise from a fixed linear congruential generator.
fn noise(n: usize, level: f32) -> Vec<f32> {
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            level * ((x >> 8) as f32 / (1 << 23) as f32 - 1.0)
        })
        .collect()
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

fn session(bpm: u64) -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(bpm), TimeSignature::COMMON))
}

fn beats(n: i64) -> Ticks {
    Ticks(n * PPQ)
}

// ---------------------------------------------------------------------------
// Rendering and comparing

/// Renders `frames` from the start in blocks of `block`, after sending
/// `setup` (play is sent last).
fn render(s: &Session, frames: usize, block: usize, setup: Vec<Command>) -> [Vec<f32>; 2] {
    let (mut ctl, mut eng) = engine::new(block);
    ctl.set_graph(s.compile(block).unwrap()).ok().unwrap();
    for c in setup {
        ctl.send(c).ok().unwrap();
    }
    ctl.send(Command::Play).ok().unwrap();
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let (mut l, mut r) = (vec![0.0; block], vec![0.0; block]);
    while out[0].len() < frames {
        let n = block.min(frames - out[0].len());
        eng.process(&mut l[..n], &mut r[..n]);
        out[0].extend_from_slice(&l[..n]);
        out[1].extend_from_slice(&r[..n]);
    }
    out
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.wav"))
}

fn write(path: &std::path::Path, [l, r]: &[Vec<f32>; 2]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let q = |x: f32| (x.clamp(-1.0, 1.0) * FULL).round() as i32;
    for (a, b) in l.iter().zip(r) {
        w.write_sample(q(*a)).unwrap();
        w.write_sample(q(*b)).unwrap();
    }
    w.finalize().unwrap();
}

fn read(path: &std::path::Path) -> [Vec<f32>; 2] {
    let mut r = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("no reference render at {}: {e}", path.display()));
    let spec = r.spec();
    assert_eq!((spec.channels, spec.sample_rate), (2, RATE));
    let all: Vec<f32> = r
        .samples::<i32>()
        .map(|s| s.unwrap() as f32 / FULL)
        .collect();
    [
        all.iter().step_by(2).copied().collect(),
        all.iter().skip(1).step_by(2).copied().collect(),
    ]
}

/// Compares a render with the reference called `name`, or rewrites the
/// reference when `DAW_BLESS` is set.
fn check(name: &str, got: &[Vec<f32>; 2], tolerance: f32) {
    let path = golden_path(name);
    if std::env::var_os("DAW_BLESS").is_some() {
        write(&path, got);
        return;
    }
    let want = read(&path);
    let mut worst = (0.0f32, 0, 0);
    for ch in 0..2 {
        assert_eq!(
            got[ch].len(),
            want[ch].len(),
            "{name}: rendered a different length from the reference"
        );
        for (i, (a, b)) in got[ch].iter().zip(&want[ch]).enumerate() {
            let d = (a - b).abs();
            if d > worst.0 || d.is_nan() {
                worst = (d, ch, i);
            }
        }
    }
    if worst.0.is_nan() || worst.0 > tolerance {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("golden")
            .join(format!("{name}.wav"));
        write(&out, got);
        panic!(
            "{name}: differs from the reference by {} at frame {} ({}), more than {tolerance}. \
             This render is at {}",
            worst.0,
            worst.2,
            ["left", "right"][worst.1],
            out.display()
        );
    }
    // A silent render would match a silent reference; make sure there is
    // something to compare.
    let peak = got.iter().flatten().fold(0.0f32, |p, x| p.max(x.abs()));
    assert!(peak > 0.01, "{name}: the render is silent");
}

// ---------------------------------------------------------------------------
// Sessions

/// Half a second, two beats.
const LEN: usize = 2 * BEAT;

/// A mono pluck panned left, a stereo pair, quiet noise with clip and
/// track gain, a trimmed clip placed a third of the way into a beat, a
/// muted track, and master gain.
fn band() -> Session {
    let mut s = session(BPM);
    s.master_db = -3.0;

    let mut lead = TrackSpec::new(vec![
        ClipSpec::new(Ticks(0), mono(pluck(220.0, 0.2, RATE))),
        ClipSpec::new(beats(1), mono(pluck(277.18, 0.2, RATE))),
    ]);
    lead.mix.pan = -0.6;

    let mut pad = TrackSpec::new(vec![ClipSpec::new(
        Ticks(PPQ / 2),
        stereo(pluck(110.0, 0.6, RATE), pluck(165.0, 0.6, RATE)),
    )]);
    pad.mix.gain_db = -4.5;

    let mut hiss = ClipSpec::new(Ticks(0), mono(noise(2 * LEN, 0.5)));
    hiss.gain_db = -12.0;
    let mut air = TrackSpec::new(vec![hiss]);
    air.mix.gain_db = -6.0;
    air.mix.pan = 0.8;

    // Plays 0.1 s of the note, starting 0.05 s in.
    let mut cut = ClipSpec::new(Ticks(PPQ + PPQ / 3), mono(pluck(440.0, 0.5, RATE)));
    cut.offset = RATE as usize / 20;
    cut.len = Some(RATE as usize / 10);
    let bell = TrackSpec::new(vec![cut]);

    let mut muted = TrackSpec::new(vec![ClipSpec::new(Ticks(0), mono(vec![0.9; LEN]))]);
    muted.mix.mute = true;

    s.tracks = vec![lead, pad, air, bell, muted];
    s
}

#[test]
fn a_small_band() {
    check("band", &render(&band(), LEN, 256, vec![]), EXACT);
}

/// The same mix in other block sizes, including one that does not divide
/// the beat, must give the same samples.
#[test]
fn block_size_does_not_change_the_mix() {
    for block in [64, 333, 1024] {
        check("band", &render(&band(), LEN, block, vec![]), EXACT);
    }
}

#[test]
fn solo_plays_only_soloed_tracks() {
    let mut s = band();
    s.tracks[1].mix.solo = true;
    s.tracks[3].mix.solo = true;
    check("solo", &render(&s, LEN, 256, vec![]), EXACT);
}

/// A loop across clip boundaries, wrapping mid-block.
#[test]
fn a_loop_wrapping() {
    let start = Frames(BEAT as i64 / 2);
    let end = Frames(BEAT as i64 * 3 / 2);
    let setup = vec![Command::SetLoop(Some((start, end))), Command::Seek(start)];
    check("loop", &render(&band(), LEN, 256, setup), EXACT);
}

/// The metronome with accents and subdivisions over a pluck, through a
/// one-bar count-in, at a tempo whose beats do not fall on whole frames.
#[test]
fn metronome_with_count_in() {
    let mut s = Session::new(TempoMap::new(
        RATE,
        Tempo::from_f64(277.0),
        TimeSignature::new(3, 4),
    ));
    s.tracks = vec![TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        mono(pluck(330.0, 0.3, RATE)),
    )])];
    let click = Metronome {
        enabled: true,
        subdivision: 2,
        ..Metronome::default()
    };
    let (mut ctl, mut eng) = engine::new(256);
    ctl.set_graph(s.compile(256).unwrap()).ok().unwrap();
    ctl.send(Command::SetMetronome(click)).ok().unwrap();
    ctl.send(Command::CountIn { bars: 1 }).ok().unwrap();
    let mut out = [Vec::new(), Vec::new()];
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    while out[0].len() < 2 * LEN {
        eng.process(&mut l, &mut r);
        out[0].extend_from_slice(&l);
        out[1].extend_from_slice(&r);
    }
    check("metronome", &out, EXACT);
}

/// A clip recorded at 44.1 kHz, converted to the session's 48 kHz as
/// importing does.
#[test]
fn a_clip_at_another_rate() {
    let at_44k = pluck(392.0, 0.4, 44_100);
    let converted = resample(&at_44k, 1, 44_100, RATE).unwrap();
    let mut s = session(BPM);
    s.tracks = vec![TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        mono(converted),
    )])];
    check("resampled", &render(&s, LEN, 256, vec![]), EXACT);
}

/// Practice speed and transpose through the phase vocoder.
#[test]
fn practice_speed_and_transpose() {
    for (name, speed, semitones) in [
        ("slow", 0.75, 0.0),
        ("transposed", 1.0, -2.0),
        ("slow-transposed", 0.6, 3.5),
    ] {
        let setup = vec![Command::SetPractice { speed, semitones }];
        check(name, &render(&band(), LEN, 256, setup), STRETCHED);
    }
}

/// After a tempo change, a musical clip moves with the tempo and an
/// absolute one (a recorded take) stays on its frame.
#[test]
fn a_take_and_a_musical_clip_after_a_tempo_change() {
    let mut s = session(200);
    s.tempo.add_tempo(beats(1), Tempo::bpm(300)).unwrap();
    s.tracks = vec![
        TrackSpec::new(vec![ClipSpec::new(
            beats(2),
            mono(pluck(262.0, 0.15, RATE)),
        )]),
        TrackSpec::new(vec![ClipSpec::new(
            Frames(15_000),
            mono(pluck(392.0, 0.15, RATE)),
        )]),
    ];
    check("tempo-change", &render(&s, LEN, 256, vec![]), EXACT);
}
