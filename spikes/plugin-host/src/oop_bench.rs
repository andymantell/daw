// SPDX-License-Identifier: GPL-3.0-or-later
//! `plugin-host oop`: plays a chain of plugins, each in its own process,
//! at real-time pace, measures the cost of the round trips, and checks
//! that killing one plugin's process loses only a few blocks.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::checks::RATE;
use crate::oop::{CHANNELS, Miss, Remote, realtime_thread};
use crate::plugin::{Instance, Module, Result};

pub struct Opts {
    pub bundles: Vec<PathBuf>,
    pub seconds: f64,
    pub block: usize,
    pub kill: bool,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Sleeps until `t`, spinning for the last millisecond.
fn wait_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let left = t - now;
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(1));
        } else {
            std::hint::spin_loop();
        }
    }
}

/// Pure processing cost of the same chain inside this process, unpaced.
fn in_process_us(bundles: &[PathBuf], block: usize, blocks: usize) -> Result<Vec<f64>> {
    let mut chain = Vec::new();
    for b in bundles {
        let module = Module::load(b)?;
        let class = module
            .classes()
            .into_iter()
            .find(|c| c.category == "Audio Module Class")
            .ok_or("no audio module class")?;
        let inst = Instance::new(&module, &class.cid, RATE, block)?;
        chain.push((module, inst));
    }
    let input = vec![vec![0.1f32; block]; CHANNELS];
    let mut times = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        let t = Instant::now();
        let mut buf = input.clone();
        for (_, inst) in &chain {
            buf = inst.process(&buf, None)?;
        }
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    // Instances must go before their modules.
    for (m, i) in chain.into_iter().rev() {
        drop(i);
        drop(m);
    }
    Ok(times)
}

pub fn run(exe: &Path, o: &Opts) -> String {
    let mut r = String::new();
    let period = Duration::from_secs_f64(o.block as f64 / RATE);
    let budget_us = period.as_secs_f64() * 1e6;
    let _ = writeln!(
        r,
        "Out-of-process hosting: {} plugin(s) in a chain, each in its own process\n  {} Hz, {}-frame blocks ({budget_us:.0} µs), {:.0} s at real-time pace",
        o.bundles.len(),
        RATE,
        o.block,
        o.seconds
    );

    let mut remotes = Vec::new();
    for b in &o.bundles {
        let t = Instant::now();
        match Remote::spawn(exe, b, o.block) {
            Ok(mut rem) => {
                let state = rem.save_state().map(|s| s.len());
                let _ = writeln!(
                    r,
                    "  {}: started in {:.0} ms, latency {} frames, state {}",
                    b.display(),
                    t.elapsed().as_secs_f64() * 1e3,
                    rem.latency,
                    match state {
                        Ok(n) => format!("{n} bytes"),
                        Err(e) => format!("not saved ({e})"),
                    }
                );
                remotes.push(rem);
            }
            Err(e) => {
                let _ = writeln!(r, "  {}: FAILED to start: {e}", b.display());
                return r;
            }
        }
    }

    let _ = writeln!(r, "  host thread: {}", realtime_thread());
    let blocks = (o.seconds / period.as_secs_f64()) as usize;
    let kill_at = if o.kill { Some(blocks / 2) } else { None };
    let mut chain_us = Vec::with_capacity(blocks);
    let mut hop_us: Vec<Vec<f64>> = vec![Vec::with_capacity(blocks); remotes.len()];
    let (mut late, mut died, mut over) = (0, 0, 0);
    let mut recovery: Option<(usize, Duration, std::result::Result<bool, String>)> = None;
    let mut silent_after_recovery = None;

    let mut phase = 0.0f64;
    let mut a = [vec![0.0f32; o.block], vec![0.0f32; o.block]];
    let mut b = [vec![0.0f32; o.block], vec![0.0f32; o.block]];
    let mut next = Instant::now() + period;
    for blk in 0..blocks {
        wait_until(next);
        next += period;
        if Some(blk) == kill_at {
            remotes[0].kill();
        }
        let [left, right] = &mut a;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let v = (0.25 * (phase * std::f64::consts::TAU).sin()) as f32;
            phase = (phase + 220.0 / RATE).fract();
            *l = v;
            *r = v;
        }
        let t0 = Instant::now();
        let mut restart = None;
        for (k, rem) in remotes.iter_mut().enumerate() {
            let t = Instant::now();
            let res = rem.process(&[&a[0], &a[1]], &mut b, period);
            hop_us[k].push(t.elapsed().as_secs_f64() * 1e6);
            match res {
                Ok(()) => {}
                Err(Miss::Late) => late += 1,
                Err(Miss::Died) => {
                    died += 1;
                    restart = Some(k);
                }
            }
            std::mem::swap(&mut a, &mut b);
        }
        let took = t0.elapsed().as_secs_f64() * 1e6;
        chain_us.push(took);
        if took > budget_us {
            over += 1;
        }
        if let Some(k) = restart {
            // A real engine restarts on another thread and keeps playing
            // with the plugin bypassed. Here the restart blocks, and its
            // length is reported as recovery time instead.
            let t = Instant::now();
            let res = remotes[k].restart().and_then(|_| {
                let before = remotes[k].save_state()?;
                Ok(!before.is_empty())
            });
            recovery = Some((blk, t.elapsed(), res));
            next = Instant::now() + period;
        }
        if let Some((rb, ..)) = recovery
            && blk == rb + 50
        {
            silent_after_recovery = Some(a[0].iter().all(|v| *v == 0.0));
        }
    }

    let mut chain = chain_us.clone();
    let _ = writeln!(
        r,
        "  chain round trip: median {:.0} µs, 99th {:.0} µs, worst {:.0} µs; {} of {} blocks over the {budget_us:.0} µs budget",
        pct(&mut chain, 0.5),
        pct(&mut chain, 0.99),
        pct(&mut chain, 1.0),
        over,
        blocks
    );
    for (k, h) in hop_us.iter_mut().enumerate() {
        let _ = writeln!(
            r,
            "  plugin {}: median {:.0} µs, 99th {:.0} µs, worst {:.0} µs per block",
            k + 1,
            pct(h, 0.5),
            pct(h, 0.99),
            pct(h, 1.0)
        );
    }
    let _ = writeln!(r, "  late answers: {late}; process deaths seen: {died}");
    match &recovery {
        Some((blk, d, res)) => {
            let _ = writeln!(
                r,
                "  crash test: killed plugin 1 at block {blk}; restarted and restored its state in {:.0} ms ({}), {}",
                d.as_secs_f64() * 1e3,
                match res {
                    Ok(true) => "state saved again afterwards".to_string(),
                    Ok(false) => "plugin has empty state".to_string(),
                    Err(e) => format!("FAILED: {e}"),
                },
                match silent_after_recovery {
                    Some(false) => "audio flowing again after restart",
                    Some(true) => "STILL SILENT after restart",
                    None => "run ended before checking audio",
                }
            );
        }
        None if o.kill => {
            let _ = writeln!(r, "  crash test: the kill was not noticed (FAIL)");
        }
        None => {}
    }

    match in_process_us(&o.bundles, o.block, blocks.min(4000)) {
        Ok(mut t) => {
            let base = pct(&mut t, 0.5);
            let _ = writeln!(
                r,
                "  same chain in this process: median {base:.0} µs per block; out-of-process overhead about {:.0} µs per block ({:.1}% of the budget)",
                pct(&mut chain, 0.5) - base,
                100.0 * (pct(&mut chain, 0.5) - base) / budget_us
            );
        }
        Err(e) => {
            let _ = writeln!(r, "  in-process comparison failed: {e}");
        }
    }
    let recovered = matches!(recovery, Some((_, _, Ok(_)))) && silent_after_recovery == Some(false);
    let ok = over == 0 && late == 0 && (!o.kill || recovered);
    let _ = writeln!(
        r,
        "  verdict: {}",
        if ok { "PASS" } else { "FAIL (see above)" }
    );
    r
}
