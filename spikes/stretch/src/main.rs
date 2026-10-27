// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmarks and checks the speed and pitch candidates, and writes WAV
//! files for listening tests.
//!
//! Usage:
//!   stretch-bench [--seconds 30] [--block 256] [--input guitar.wav]
//!                 [--out stretch-listen] [--only own-2048]
//!
//! Writes stretch-report.txt and one folder of WAV files per candidate.

use std::path::PathBuf;
use std::time::Instant;

use spike_engine::rt_check::{self, RtGuard};
use stretch_bench::signals::{self, attack, cents, estimate_f0};
use stretch_bench::{CANDIDATES, Feed, RATE, build};

const SETTINGS: [(f64, f64); 9] = [
    (0.5, 0.0),
    (0.75, 0.0),
    (1.0, 0.0),
    (1.5, 0.0),
    (1.0, -12.0),
    (1.0, -1.0),
    (1.0, 1.0),
    (1.0, 12.0),
    (0.75, -1.0),
];

fn label(speed: f64, semis: f64) -> String {
    format!("speed {:>3.0}%, pitch {:+3.0} st", speed * 100.0, semis)
}

fn file_label(speed: f64, semis: f64) -> String {
    format!("speed{:03.0}_pitch{:+03.0}", speed * 100.0, semis)
}

/// Renders `frames` of output in blocks.
fn render(
    name: &str,
    block: usize,
    input: &[f32],
    speed: f64,
    semis: f64,
    frames: usize,
) -> Option<Vec<f32>> {
    let mut s = build(name, block);
    if !s.set(speed, semis) {
        return None;
    }
    let mut feed = Feed::new(input);
    let mut out = vec![0.0; frames.div_ceil(block) * block];
    for chunk in out.chunks_mut(block) {
        s.process(&mut feed, chunk);
    }
    out.truncate(frames);
    Some(out)
}

struct Cpu {
    mean_us: f64,
    p99_us: f64,
    max_us: f64,
    allocs: usize,
    rate: f64,
}

fn cpu_run(
    name: &str,
    block: usize,
    input: &[f32],
    speed: f64,
    semis: f64,
    seconds: f64,
) -> Option<Cpu> {
    let mut s = build(name, block);
    if !s.set(speed, semis) {
        return None;
    }
    let mut feed = Feed::new(input);
    let mut out = vec![0.0f32; block];
    // Warm up for two seconds, untimed.
    let warm = (2 * RATE as usize) / block;
    for _ in 0..warm {
        s.process(&mut feed, &mut out);
    }
    let c0 = feed.consumed;
    let blocks = (seconds * RATE as f64) as usize / block;
    let mut times = Vec::with_capacity(blocks);
    let before = rt_check::violations();
    {
        let _rt = RtGuard::enter();
        for _ in 0..blocks {
            let t = Instant::now();
            s.process(&mut feed, &mut out);
            times.push(t.elapsed().as_secs_f64() * 1e6);
        }
    }
    let allocs = rt_check::violations() - before;
    let rate = (feed.consumed - c0) / (blocks * block) as f64;
    times.sort_by(f64::total_cmp);
    Some(Cpu {
        mean_us: times.iter().sum::<f64>() / times.len() as f64,
        p99_us: times[(times.len() - 1) * 99 / 100],
        max_us: *times.last().unwrap(),
        allocs,
        rate,
    })
}

fn main() {
    let mut seconds = 30.0;
    let mut block = 256;
    let mut input: Option<PathBuf> = None;
    let mut out_dir = PathBuf::from("stretch-listen");
    let mut only: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next();
        match (a.as_str(), v) {
            ("--seconds", Some(v)) => seconds = v.parse().expect("--seconds"),
            ("--block", Some(v)) => block = v.parse().expect("--block"),
            ("--input", Some(v)) => input = Some(v.into()),
            ("--out", Some(v)) => out_dir = v.into(),
            ("--only", Some(v)) => only = Some(v),
            _ => {
                eprintln!(
                    "usage: stretch-bench [--seconds N] [--block N] [--input file.wav] [--out DIR] [--only NAME]"
                );
                std::process::exit(2);
            }
        }
    }
    let names: Vec<&str> = CANDIDATES
        .iter()
        .copied()
        .filter(|n| only.as_deref().is_none_or(|o| o == *n))
        .collect();

    let phrase = match &input {
        Some(p) => signals::read_wav(p).unwrap_or_else(|e| {
            eprintln!("{}: {e}", p.display());
            std::process::exit(1)
        }),
        None => signals::guitar_phrase(),
    };
    let budget_us = block as f64 / RATE as f64 * 1e6;
    let mut r = String::new();
    let mut line = |s: String| {
        println!("{s}");
        r.push_str(&s);
        r.push('\n');
    };
    line(format!(
        "Speed and pitch spike (0.6)\n  {} Hz mono, {block}-frame blocks ({budget_us:.0} µs each), {seconds:.0} s timed per setting\n  material: {}\n",
        RATE,
        input
            .as_ref()
            .map_or("synthetic guitar phrase".into(), |p| p
                .display()
                .to_string())
    ));

    line("1. CPU per track (one core; lower is better)".into());
    line(format!(
        "  {:<12} {:<26} {:>9} {:>9} {:>9} {:>8} {:>10} {:>7} {:>8}",
        "candidate",
        "setting",
        "mean µs",
        "99th µs",
        "worst µs",
        "% core",
        "tracks/core",
        "allocs",
        "rate err"
    ));
    for name in &names {
        for &(sp, st) in &SETTINGS {
            match cpu_run(name, block, &phrase, sp, st, seconds) {
                Some(c) => line(format!(
                    "  {:<12} {:<26} {:>9.1} {:>9.1} {:>9.1} {:>7.2}% {:>10.0} {:>7} {:>7.2}%",
                    name,
                    label(sp, st),
                    c.mean_us,
                    c.p99_us,
                    c.max_us,
                    100.0 * c.mean_us / budget_us,
                    budget_us / c.mean_us,
                    c.allocs,
                    100.0 * (c.rate / sp - 1.0)
                )),
                None => line(format!(
                    "  {:<12} {:<26} not supported",
                    name,
                    label(sp, st)
                )),
            }
        }
    }

    line("\n2. Pitch accuracy on a steady 220 Hz tone (cents off the target; 5 cents is about the smallest audible)".into());
    let tone = signals::harmonic_tone(8.0);
    for name in &names {
        let mut cells = Vec::new();
        for &(sp, st) in &SETTINGS {
            let want = 220.0 * 2f32.powf(st as f32 / 12.0);
            let cell =
                render(name, block, &tone, sp, st, 4 * RATE as usize).map_or("  n/a".into(), |o| {
                    let seg = &o[(RATE as usize * 3 / 2)..(RATE as usize * 5 / 2)];
                    format!("{:+6.1}", cents(estimate_f0(seg), want))
                });
            cells.push(format!(
                "{}%/{:+}st {}",
                (sp * 100.0) as i32,
                st as i32,
                cell
            ));
        }
        line(format!("  {:<12} {}", name, cells.join(" | ")));
    }

    line("\n3. Pick attack on a single pluck (ms; original first)".into());
    let pl = signals::single_pluck();
    let (r0, s0) = attack(&pl);
    line(format!(
        "  {:<12} 10–90% rise {:>5.1}, smear before peak {:>5.1}",
        "original", r0, s0
    ));
    for name in &names {
        let mut cells = Vec::new();
        for &(sp, st) in &[(1.0, 0.0), (0.5, 0.0), (1.0, -1.0)] {
            let frames = (pl.len() as f64 / sp) as usize;
            let cell = render(name, block, &pl, sp, st, frames).map_or("n/a".into(), |o| {
                let (rise, smear) = attack(&o);
                format!("rise {rise:>5.1}, smear {smear:>5.1}")
            });
            cells.push(format!("{}: {cell}", label(sp, st)));
        }
        line(format!("  {:<12} {}", name, cells.join(" | ")));
    }

    line(format!(
        "\n4. Listening files in {}/<candidate>/",
        out_dir.display()
    ));
    let _ = std::fs::create_dir_all(&out_dir);
    let _ = signals::write_wav(&out_dir.join("original.wav"), &phrase);
    for name in &names {
        let dir = out_dir.join(name);
        let _ = std::fs::create_dir_all(&dir);
        let mut written = 0;
        for &(sp, st) in &SETTINGS {
            let frames = (phrase.len() as f64 / sp) as usize;
            if let Some(o) = render(name, block, &phrase, sp, st, frames)
                && signals::write_wav(&dir.join(format!("{}.wav", file_label(sp, st))), &o).is_ok()
            {
                written += 1;
            }
        }
        line(format!("  {name}: {written} files"));
    }

    let _ = std::fs::write("stretch-report.txt", &r);
}
