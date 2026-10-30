// SPDX-License-Identifier: GPL-3.0-or-later
//! Checking how well an installed plugin works here (roadmap 2.4), for
//! the compatibility list in `docs/plugin-compatibility.md`.
//!
//! Each plugin is checked in a helper process of its own ([`Host`]), so
//! one that crashes or hangs is noted and the rest are still checked.
//! The steps: it loads; it plays a second of sound in blocks of the usual
//! size and in odd ones, giving out finite samples; its settings come
//! back in a second copy; and, where it has them, its sidechain input
//! and extra outputs can be switched on. [`markdown`] writes the results
//! as rows for the list.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use daw_engine::graph::Insert;

use crate::host::Host;
use crate::module::ClassInfo;
use crate::plugin::{Buses, Layout};

/// How one step went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass(String),
    Fail(String),
    /// Not tried: an earlier step failed, or it doesn't apply.
    Skipped(String),
}

impl Outcome {
    pub fn passed(&self) -> bool {
        matches!(self, Outcome::Pass(_))
    }

    fn cell(&self) -> String {
        let (mark, text) = match self {
            Outcome::Pass(t) => ("yes", t),
            Outcome::Fail(t) => ("**no**", t),
            Outcome::Skipped(t) => ("–", t),
        };
        let text = text.replace('|', "/");
        if text.is_empty() {
            mark.to_string()
        } else {
            format!("{mark}: {text}")
        }
    }
}

/// What checking one plugin found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub vendor: String,
    pub bundle: PathBuf,
    pub loads: Outcome,
    pub plays: Outcome,
    pub settings: Outcome,
    pub sidechain: Outcome,
    pub outputs: Outcome,
}

impl Report {
    /// Every step that applies passed.
    pub fn ok(&self) -> bool {
        [
            &self.loads,
            &self.plays,
            &self.settings,
            &self.sidechain,
            &self.outputs,
        ]
        .iter()
        .all(|o| !matches!(o, Outcome::Fail(_)))
    }
}

const RATE: f64 = 48_000.0;
const BLOCK: usize = 512;

/// A second of a quiet tone, as test input.
fn tone(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| 0.25 * (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin())
        .collect()
}

/// Plays a second through `fx`, in usual and odd block sizes, with a
/// sidechain if `keyed`. The output's peak, or why it is no good.
fn play(fx: &mut dyn Insert, keyed: bool) -> Result<f32, String> {
    let input = tone(RATE as usize);
    let (mut l, mut r) = (input.clone(), input.clone());
    let mut done = 0;
    // Usual blocks, then odd ones: one frame, a prime, the most allowed.
    let sizes = [BLOCK, BLOCK, 1, 17, BLOCK, 333, BLOCK];
    let mut i = 0;
    while done < l.len() {
        let n = sizes[i % sizes.len()].min(l.len() - done);
        let (bl, br) = (&mut l[done..done + n], &mut r[done..done + n]);
        if keyed {
            let k = &input[done..done + n];
            fx.process_keyed(bl, br, (k, k));
        } else {
            fx.process(bl, br);
        }
        done += n;
        i += 1;
    }
    if l.iter().chain(&r).any(|x| !x.is_finite()) {
        return Err("gave out samples that are not numbers".into());
    }
    Ok(l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs())))
}

fn describe(buses: &Buses, latency: u32, params: usize) -> String {
    let side_ins = buses.inputs.len().saturating_sub(1);
    let extra_outs = buses.outputs.len().saturating_sub(1);
    let mut s = format!("{params} settings, delay {latency}");
    if side_ins > 0 {
        let _ = write!(s, ", {side_ins} side input(s)");
    }
    if extra_outs > 0 {
        let _ = write!(s, ", {extra_outs} extra output(s)");
    }
    s
}

/// Checks `class` from `bundle`, running it in copies of `exe` started
/// as plugin helpers.
pub fn check(exe: &Path, bundle: &Path, class: &ClassInfo) -> Report {
    let skipped = || Outcome::Skipped(String::new());
    let mut report = Report {
        name: class.name.clone(),
        vendor: class.vendor.clone(),
        bundle: bundle.to_owned(),
        loads: skipped(),
        plays: skipped(),
        settings: skipped(),
        sidechain: Outcome::Skipped("none".into()),
        outputs: Outcome::Skipped("none".into()),
    };
    let start = || Host::spawn(exe);

    // Loads, and plays.
    let host = match start() {
        Ok(h) => h,
        Err(e) => {
            report.loads = Outcome::Fail(e);
            return report;
        }
    };
    let (plugin, mut fx) = match host.instantiate(bundle, &class.cid, RATE, BLOCK, None) {
        Ok(x) => x,
        Err(e) => {
            report.loads = Outcome::Fail(e);
            return report;
        }
    };
    let buses = plugin.buses().clone();
    report.loads = Outcome::Pass(describe(&buses, plugin.latency(), plugin.params().len()));
    report.plays = match play(&mut fx, false) {
        Ok(_) if host.failure().is_some() => Outcome::Fail(host.failure().unwrap_or_default()),
        Ok(peak) if peak == 0.0 => Outcome::Pass("silent with a tone in".into()),
        Ok(_) => Outcome::Pass(String::new()),
        Err(e) => Outcome::Fail(e),
    };
    if let Some(why) = host.failure() {
        report.plays = Outcome::Fail(why);
        return report;
    }

    // Settings: saved from one copy, loaded into another.
    report.settings = (|| -> Result<Outcome, String> {
        let state = plugin.save_state()?;
        let (other, _fx) = host.instantiate(bundle, &class.cid, RATE, BLOCK, None)?;
        other.load_state(&state)?;
        let again = other.save_state()?;
        Ok(Outcome::Pass(if again == state {
            String::new()
        } else {
            "saved differently once loaded, which can be harmless".into()
        }))
    })()
    .unwrap_or_else(|e| Outcome::Fail(host.failure().unwrap_or(e)));
    drop((plugin, fx));

    // Side buses, each in a fresh helper.
    let with = |layout: Layout, keyed: bool| -> Outcome {
        let host = match start() {
            Ok(h) => h,
            Err(e) => return Outcome::Fail(e),
        };
        match host.instantiate_with(bundle, &class.cid, RATE, BLOCK, None, &layout) {
            Ok((_p, mut fx)) => match play(&mut fx, keyed) {
                Ok(_) => match host.failure() {
                    Some(why) => Outcome::Fail(why),
                    None => Outcome::Pass(String::new()),
                },
                Err(e) => Outcome::Fail(e),
            },
            Err(e) => Outcome::Fail(host.failure().unwrap_or(e)),
        }
    };
    if buses.sidechain().is_some() {
        report.sidechain = with(
            Layout {
                sidechain: true,
                outputs: Vec::new(),
            },
            true,
        );
    }
    let extra: Vec<u32> = buses.extra_outputs().map(|(i, _)| i as u32).collect();
    if !extra.is_empty() {
        report.outputs = with(
            Layout {
                sidechain: false,
                outputs: extra,
            },
            false,
        );
    }
    report
}

/// The reports as rows of the compatibility list's table, with its
/// heading.
pub fn markdown(reports: &[Report], host_version: &str, platform: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Checked with version {host_version} on {platform}.\n");
    s.push_str("| Plugin | Vendor | Loads | Plays | Settings kept | Sidechain | Extra outputs |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    for r in reports {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.name.replace('|', "/"),
            r.vendor.replace('|', "/"),
            r.loads.cell(),
            r.plays.cell(),
            r.settings.cell(),
            r.sidechain.cell(),
            r.outputs.cell(),
        );
    }
    s
}
