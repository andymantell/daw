// SPDX-License-Identifier: GPL-3.0-or-later
//! The vintage toolkit: the parts the classic-synth instruments share
//! (`docs/plans/classic-synths.md`, step 0). Each is written from a
//! published description of how the circuit behaves, not from anyone's
//! code, and every nonlinear part is modelled where the circuit has it:
//! inside the filters' stages and feedback paths, in the mixer and the
//! amplifier, not bolted on afterwards.
//!
//! - [`saturate`]: the soft clipping the rest uses, for mixer overdrive,
//!   amplifiers and the output.
//! - [`oversample`]: runs a nonlinear part at twice the sample rate so
//!   its distortion products fold back less.
//! - [`onepole`]: the one-pole building block of every filter here, by
//!   the topology-preserving (zero-delay feedback) method.
//! - [`ladder`]: the four-pole transistor ladder with saturating stages.
//! - [`diode`]: the four-pole diode ladder, stages coupled both ways, a
//!   high-pass in the resonance loop.
//! - [`cascade`]: the four-pole transconductance cascade with a two-pole
//!   tap, self-oscillating.
//! - [`svf`]: the two-pole state-variable filter with a low → notch →
//!   high sweep and a band-pass switch, saturating in its loop.
//! - [`sallen_key`]: the two-pole Sallen-Key pair, high-pass and
//!   low-pass, whose resonance path distorts on purpose.
//! - [`highpass`]: the plain non-resonant high-pass.
//! - [`osc`]: drifting band-limited oscillators with sync, pulse-width
//!   modulation and sub-oscillators.
//! - [`env`]: envelopes with analogue curves and quirks, and the
//!   rate/level envelope of the FM synth.
//! - [`chorus`]: the bucket-brigade stereo chorus.
//! - [`voice`]: voice handling shared by all: polyphony, unison, mono
//!   priorities, legato and glide.
//!
//! Sources: Huovilainen, "Non-Linear Digital Implementation of the Moog
//! Ladder Filter" (DAFx 2004) for the saturating ladder stages;
//! Zavalishin, "The Art of VA Filter Design" for the zero-delay feedback
//! method and the filter topologies; Välimäki and Huovilainen (2007) for
//! polyBLEP; the circuit descriptions in the plan's sources note. Each
//! piece's own documentation says which it follows.

pub mod cascade;
pub mod chorus;
pub mod diode;
pub mod env;
pub mod highpass;
pub mod ladder;
pub mod onepole;
pub mod osc;
pub mod oversample;
pub mod sallen_key;
pub mod saturate;
pub mod svf;
pub mod voice;

pub use cascade::{Cascade, Poles};
pub use chorus::{Chorus, ChorusMode};
pub use diode::Diode;
pub use env::{Analog, Curve, RateLevel, RateLevelEnv, Shape, Steps};
pub use highpass::Highpass;
pub use ladder::Ladder;
pub use onepole::OnePole;
pub use osc::{Drift, Osc, Sub, SubShape, Wave};
pub use oversample::Over2;
pub use sallen_key::{SallenKey, SkMode};
pub use saturate::{Drive, flush, soft, soft_pair, soft_slope};
pub use svf::{Svf, SvfOut};
pub use voice::{Action, Allocator, Glide, GlideMode, Priority, VoiceSettings};

#[cfg(test)]
pub(crate) mod measure {
    //! Measuring tools the tests share: the level a filter passes at a
    //! frequency, and the frequency a signal rings at.
    use std::f32::consts::TAU;

    /// Runs `f` on a sine at `hz` for a second and returns the
    /// amplitude at `hz` in the second half, after the filter has
    /// settled: what it passes, leaving out any distortion it adds.
    pub fn level(hz: f32, rate: f32, mut f: impl FnMut(f32) -> f32) -> f32 {
        let n = rate as usize;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for i in 0..n {
            let p = TAU * hz * i as f32 / rate;
            let y = f(p.sin());
            if i >= n / 2 {
                re += (y * p.cos()) as f64;
                im += (y * p.sin()) as f64;
            }
        }
        (re * re + im * im).sqrt() as f32 * 2.0 / (n - n / 2) as f32
    }

    /// `level` in decibels.
    pub fn db(hz: f32, rate: f32, f: impl FnMut(f32) -> f32) -> f32 {
        20.0 * level(hz, rate, f).max(1e-9).log10()
    }

    /// The frequency a signal crosses zero at, from `x`'s second half.
    pub fn ring_hz(x: &[f32], rate: f32) -> f32 {
        let half = &x[x.len() / 2..];
        let crossings = half
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f32 / 2.0 * rate / half.len() as f32
    }

    /// The peak of `x`.
    pub fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }
}
