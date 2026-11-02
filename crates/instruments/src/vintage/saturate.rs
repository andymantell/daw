// SPDX-License-Identifier: GPL-3.0-or-later
//! Soft clipping: the rounded limit a transistor pair, a mixer or an
//! amplifier stage puts on a signal. One cheap curve is used everywhere
//! so the filters' loops, which call it several times a sample, stay
//! affordable.

/// A soft limit of ±1, shaped like `tanh` (within 3% of it up to ±2,
/// and exactly 1 from ±3): a rational curve, so no transcendental
/// function in the audio loop.
#[inline]
pub fn soft(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

/// The slope of [`soft`] at `x`, for solving loops that contain it.
#[inline]
pub fn soft_slope(x: f32) -> f32 {
    soft_pair(x).1
}

/// [`soft`] and its slope together, sharing the one division, for the
/// loop solvers that need both each step.
#[inline]
pub fn soft_pair(x: f32) -> (f32, f32) {
    if x.abs() >= 3.0 {
        return (x.signum(), 0.0);
    }
    let x2 = x * x;
    let n = 27.0 + x2;
    let d = 27.0 + 9.0 * x2;
    let inv = 1.0 / d;
    let y = x * n * inv;
    // d/dx of x n / d = (n + 2x²) / d − x n · 18 x / d².
    let slope = ((27.0 + 3.0 * x2) - 18.0 * x2 * n * inv) * inv;
    (y, slope)
}

/// `v` with values too small to matter flushed to zero, so a filter's
/// states don't fall into the slow denormal range as a note dies away.
#[inline]
pub fn flush(v: &mut f32) {
    if v.abs() < 1e-18 {
        *v = 0.0;
    }
}

/// An overdriven stage: a gain into the soft limit, with the level put
/// back afterwards so turning drive up thickens a sound without making
/// it much louder. `drive` is 0 (clean, gain 1) to 1 (gain 16, hard).
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    gain: f32,
    makeup: f32,
}

impl Drive {
    pub const CLEAN: Drive = Drive {
        gain: 1.0,
        makeup: 1.0,
    };

    pub fn new(drive: f32) -> Drive {
        let gain = 16f32.powf(drive.clamp(0.0, 1.0));
        // A signal at a quarter of full scale (a typical oscillator mix)
        // comes out at the level it went in.
        let makeup = 0.25 / soft(0.25 * gain);
        Drive { gain, makeup }
    }

    #[inline]
    pub fn run(&self, x: f32) -> f32 {
        soft(x * self.gain) * self.makeup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_tracks_tanh_and_stops_at_one() {
        for i in 0..=40 {
            let x = i as f32 * 0.05;
            assert!((soft(x) - x.tanh()).abs() < 0.03, "{x}");
            assert!((soft(-x) + soft(x)).abs() < 1e-6);
        }
        assert_eq!(soft(5.0), 1.0);
        assert_eq!(soft(-9.0), -1.0);
        // The slope matches a finite difference.
        for i in 0..30 {
            let x = i as f32 * 0.1;
            let fd = (soft(x + 1e-3) - soft(x - 1e-3)) / 2e-3;
            assert!((soft_slope(x) - fd).abs() < 1e-3, "{x}");
        }
    }

    #[test]
    fn drive_keeps_a_quiet_signal_level_and_squashes_a_loud_one() {
        let d = Drive::new(0.5);
        assert!((d.run(0.25) - 0.25).abs() < 1e-3);
        assert!(d.run(1.0) < 0.6);
        assert!((Drive::new(0.0).run(0.5) - 0.5).abs() < 0.03);
    }
}
