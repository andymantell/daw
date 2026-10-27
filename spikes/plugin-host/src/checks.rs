// SPDX-License-Identifier: GPL-3.0-or-later
//! The spike 0.1 B checks for one plugin: process audio, report latency,
//! take automation, save and restore state, and (on Windows) open, resize
//! and close the editor embedded in our own window.

use std::fmt::Write as _;
use std::path::Path;

use vst3::ComWrapper;

use crate::com::ParamChanges;
use crate::plugin::{Instance, Module, Result};

pub const RATE: f64 = 48_000.0;
pub const BLOCK: usize = 256;

#[derive(Default)]
pub struct Report {
    pub name: String,
    pub vendor_classes: usize,
    pub kind: String,
    pub inputs: String,
    pub outputs: String,
    pub latency: u32,
    pub params: usize,
    pub processed: bool,
    pub output_peak: f32,
    pub bad_samples: usize,
    pub automation: Option<String>,
    pub state_bytes: usize,
    pub state_restores_identically: Option<bool>,
    pub editor: String,
    pub failures: Vec<String>,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "plugin: {}", self.name);
        let _ = writeln!(s, "  kind: {}", self.kind);
        let _ = writeln!(
            s,
            "  audio buses: in [{}], out [{}]",
            self.inputs, self.outputs
        );
        let _ = writeln!(s, "  latency: {} samples", self.latency);
        let _ = writeln!(s, "  parameters: {}", self.params);
        let _ = writeln!(
            s,
            "  processing: {} (output peak {:.3}, non-finite samples {})",
            if self.processed { "ok" } else { "failed" },
            self.output_peak,
            self.bad_samples
        );
        let _ = writeln!(
            s,
            "  automation: {}",
            self.automation
                .as_deref()
                .unwrap_or("no automatable parameter")
        );
        let _ = writeln!(
            s,
            "  state: {} bytes, restore {}",
            self.state_bytes,
            match self.state_restores_identically {
                Some(true) => "reproduces the same output",
                Some(false) => "gives different output (may be fine for plugins with randomness)",
                None => "not checked",
            }
        );
        let _ = writeln!(s, "  editor: {}", self.editor);
        let _ = writeln!(
            s,
            "  result: {}",
            if self.passed() {
                "PASS".to_string()
            } else {
                format!("FAIL: {}", self.failures.join("; "))
            }
        );
        s
    }
}

fn sine(block: usize, start: usize) -> Vec<Vec<f32>> {
    let ch: Vec<f32> = (0..block)
        .map(|i| 0.25 * (std::f32::consts::TAU * 440.0 * (start + i) as f32 / RATE as f32).sin())
        .collect();
    vec![ch.clone(), ch]
}

fn run_blocks(inst: &Instance, blocks: usize) -> Result<Vec<Vec<f32>>> {
    let mut all = Vec::new();
    for b in 0..blocks {
        let out = inst.process(&sine(BLOCK, b * BLOCK), None)?;
        if all.is_empty() {
            all = vec![Vec::new(); out.len()];
        }
        for (a, o) in all.iter_mut().zip(out) {
            a.extend(o);
        }
    }
    Ok(all)
}

/// Runs every check. `editor` opens the plugin's editor window where the
/// platform supports it.
pub fn check(bundle: &Path, editor: bool) -> Report {
    let mut r = Report {
        name: bundle
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        ..Default::default()
    };
    if let Err(e) = check_inner(bundle, editor, &mut r) {
        r.failures.push(e);
    }
    r
}

fn check_inner(bundle: &Path, editor: bool, r: &mut Report) -> Result<()> {
    let module = Module::load(bundle)?;
    let classes = module.classes();
    r.vendor_classes = classes.len();
    let class = classes
        .iter()
        .find(|c| c.category == "Audio Module Class")
        .ok_or("no audio module class in factory")?;
    r.name = class.name.clone();

    let inst = Instance::new(&module, &class.cid, RATE, BLOCK)?;
    r.kind = if inst.inputs.is_empty() || inst.has_event_input {
        "instrument or generator (no audio input, or takes notes)".into()
    } else {
        "effect".into()
    };
    let fmt = |b: &[crate::plugin::Bus]| {
        b.iter()
            .map(|b| b.channels.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    r.inputs = fmt(&inst.inputs);
    r.outputs = fmt(&inst.outputs);
    r.latency = inst.latency();
    let params = inst.params();
    r.params = params.len();

    // 1. Process about a second of audio.
    let out = run_blocks(&inst, 200)?;
    r.processed = true;
    r.output_peak =
        out.iter().flatten().fold(
            0.0f32,
            |m, &x| if x.is_finite() { m.max(x.abs()) } else { m },
        );
    r.bad_samples = out.iter().flatten().filter(|x| !x.is_finite()).count();
    if r.bad_samples > 0 {
        r.failures
            .push(format!("{} non-finite output samples", r.bad_samples));
    }

    // 2. Automation: sweep the first automatable, non-bypass parameter.
    if let Some(p) = params.iter().find(|p| p.automatable && !p.bypass) {
        for (i, v) in [0.0, 0.25, 0.5, 0.75, 1.0].iter().enumerate() {
            let changes = ComWrapper::new(ParamChanges::with_point(p.id, 0, *v));
            inst.process(&sine(BLOCK, i * BLOCK), Some(&changes))?;
        }
        r.automation = Some(format!(
            "swept \"{}\" (id {}) 0→1 over 5 blocks without error",
            p.title, p.id
        ));
    }

    // 3. State: save, restore into two fresh instances, compare them.
    let (state, ctl_state) = inst.save_state()?;
    r.state_bytes = state.len();
    let a = Instance::new(&module, &class.cid, RATE, BLOCK)?;
    let b = Instance::new(&module, &class.cid, RATE, BLOCK)?;
    a.load_state(&state, ctl_state.as_deref())?;
    b.load_state(&state, ctl_state.as_deref())?;
    let (oa, ob) = (run_blocks(&a, 50)?, run_blocks(&b, 50)?);
    r.state_restores_identically = Some(oa == ob);
    drop((a, b));

    // 4. Editor.
    r.editor = match inst.create_editor() {
        None => "none".into(),
        Some(view) => {
            #[cfg(windows)]
            {
                if editor {
                    match crate::editor_win::exercise(&view, 3.0) {
                        Ok(e) => e,
                        Err(e) => {
                            r.failures.push(format!("editor: {e}"));
                            format!("failed: {e}")
                        }
                    }
                } else {
                    "available (run with --editor to open it)".into()
                }
            }
            #[cfg(not(windows))]
            {
                let _ = (editor, view);
                "available (opening it is only implemented on Windows so far)".into()
            }
        }
    };
    drop(inst);
    drop(module);
    Ok(())
}
