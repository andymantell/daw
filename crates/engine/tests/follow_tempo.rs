// SPDX-License-Identifier: GPL-3.0-or-later
//! Clips that follow tempo (plan A7): a warped clip plays through the
//! graph with each tied frame on its timeline frame, from memory or
//! from disk, and round the loop.

use std::path::PathBuf;
use std::sync::Arc;

use daw_engine::engine::{self, Command};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::stream::{CHUNK, StreamSource};
use daw_engine::time::{Frames, Tempo, TempoMap, TimeSignature};
use daw_engine::warp::{Warp, WarpPoint};

const RATE: u32 = 48_000;

fn session() -> Session {
    Session::new(TempoMap::new(RATE, Tempo::bpm(120), TimeSignature::COMMON))
}

fn pt(src: i64, out: i64) -> WarpPoint {
    WarpPoint { src, out }
}

/// Clicks at the given frames, silence otherwise.
fn clicks(frames: usize, at: &[i64]) -> Vec<f32> {
    let mut v = vec![0.0; frames];
    for &a in at {
        v[a as usize] = 1.0;
    }
    v
}

fn peak_near(l: &[f32], at: usize, half: usize) -> usize {
    let (lo, hi) = (at.saturating_sub(half), (at + half).min(l.len()));
    (lo..hi)
        .max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs()))
        .unwrap()
}

fn ms(a: usize, b: usize) -> f64 {
    (a as f64 - b as f64) / RATE as f64 * 1000.0
}

#[test]
fn a_warped_clip_from_memory_lands_its_tied_frames_on_the_timeline() {
    let gap = 19_200i64;
    let at: Vec<i64> = (1..6).map(|k| k * gap).collect();
    let src = clicks(gap as usize * 7, &at);
    let data = Arc::new(AudioData {
        channels: vec![src],
        rate: RATE,
    });
    // Played from 1 s, slowed by a quarter for the first three clicks,
    // then back to time.
    let out0 = 48_000;
    let warp = Arc::new(
        Warp::new(vec![
            pt(0, out0),
            pt(3 * gap, out0 + 3 * gap * 5 / 4),
            pt(7 * gap, out0 + 3 * gap * 5 / 4 + 4 * gap),
        ])
        .unwrap(),
    );
    let mut s = session();
    let mut clip = ClipSpec::new(Frames(0), data);
    clip.warp = Some(warp.clone());
    clip.tag = 7;
    let mut t = TrackSpec::new(vec![clip]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let n = warp.out_range().end as usize + 1000;
    let [l, _] = engine::render_offline(s.compile(512).unwrap(), Frames(0), n, 512);
    for &a in &at {
        let want = warp.out_at(a as f64) as usize;
        let got = peak_near(&l, want, 4_000);
        assert!(
            ms(got, want).abs() < 12.0,
            "click {a}: {:.1} ms off",
            ms(got, want)
        );
    }
    // Silent before the clip starts and after it ends.
    assert!(l[..out0 as usize - 100].iter().all(|x| x.abs() < 1e-4));
    assert!(
        l[warp.out_range().end as usize + 200..]
            .iter()
            .all(|x| x.abs() < 1e-4)
    );
}

fn temp_wav(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-engine-follow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn a_warped_clip_from_disk_is_streamed_along_its_warp() {
    // Longer than a few chunks, so the disk thread has to follow the
    // warp to load the right ones.
    let len = CHUNK * 6;
    let at: Vec<i64> = (1..10).map(|k| k as i64 * 20_000).collect();
    let src = clicks(len, &at);
    let path = temp_wav("warped.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for x in &src {
        w.write_sample(*x).unwrap();
    }
    w.finalize().unwrap();
    let file = StreamSource::open(&path).unwrap();
    // Slowed by a third from 0.5 s: 1.5 output frames per source frame.
    let warp = Arc::new(Warp::even(0..len as i64, 24_000, 1.0 / 1.5).unwrap());
    let mut s = session();
    let mut clip = ClipSpec::new(Frames(0), file);
    clip.warp = Some(warp.clone());
    clip.tag = 1;
    let mut t = TrackSpec::new(vec![clip]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let n = warp.out_range().end as usize;
    let [l, _] = engine::render_offline(s.compile(512).unwrap(), Frames(0), n, 512);
    for &a in &at {
        let want = warp.out_at(a as f64) as usize;
        let got = peak_near(&l, want, 4_000);
        assert!(
            ms(got, want).abs() < 12.0,
            "click {a}: {:.1} ms off",
            ms(got, want)
        );
        assert!(l[got].abs() > 0.1, "click {a} is missing: {}", l[got]);
    }
}

#[test]
fn a_warped_clip_goes_round_the_loop() {
    let gap = 19_200i64;
    let at: Vec<i64> = (1..6).map(|k| k * gap).collect();
    let data = Arc::new(AudioData {
        channels: vec![clicks(gap as usize * 7, &at)],
        rate: RATE,
    });
    let warp = Arc::new(Warp::even(0..7 * gap, 0, 0.8).unwrap());
    let mut s = session();
    let mut clip = ClipSpec::new(Frames(0), data);
    clip.warp = Some(warp.clone());
    let mut t = TrackSpec::new(vec![clip]);
    t.mix.pan = -1.0;
    s.tracks.push(t);
    let (mut ctl, mut eng) = engine::new(512);
    let _disk = ctl.take_streamer().unwrap();
    ctl.set_graph(s.compile(512).unwrap()).ok().unwrap();
    // A loop from the second click's place to just past the fourth's.
    let (a, b) = (
        warp.out_at(2.0 * gap as f64) as i64 - 3_000,
        warp.out_at(4.0 * gap as f64) as i64 + 5_000,
    );
    ctl.send(Command::SetLoop(Some((Frames(a), Frames(b)))))
        .ok()
        .unwrap();
    ctl.send(Command::Seek(Frames(a))).ok().unwrap();
    eng.process(&mut [], &mut []);
    ctl.send(Command::Play).ok().unwrap();
    let mut out = Vec::new();
    while out.len() < 3 * (b - a) as usize {
        let (mut l, mut r) = (vec![0.0; 333], vec![0.0; 333]);
        eng.process(&mut l, &mut r);
        out.extend(l);
    }
    // Each pass plays clicks 2, 3 and 4 where the warp puts them.
    for pass in 0..3 {
        let base = pass * (b - a) as usize;
        for k in 2..=4 {
            let want = base + (warp.out_at(k as f64 * gap as f64) as i64 - a) as usize;
            let got = peak_near(&out, want, 2_500);
            assert!(
                ms(got, want).abs() < 12.0 && out[got].abs() > 0.1,
                "pass {pass}, click {k}: {:.1} ms off, level {}",
                ms(got, want),
                out[got]
            );
        }
    }
}
