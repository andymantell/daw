// SPDX-License-Identifier: GPL-3.0-or-later
//! Second-order filters, with coefficients from the widely published
//! "Audio EQ Cookbook" formulas (R. Bristow-Johnson), run in transposed
//! direct form II.

use std::f64::consts::PI;

/// A filter's coefficients, normalised so a0 is 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefs {
    /// Passes everything unchanged.
    pub const THROUGH: Coefs = Coefs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn new(b: [f64; 3], a: [f64; 3]) -> Coefs {
        Coefs {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// The angle and sine-over-2Q terms for `hz` at `rate`, the frequency
    /// kept below the top of the band.
    fn w(hz: f64, q: f64, rate: f64) -> (f64, f64, f64) {
        let w = 2.0 * PI * hz.clamp(1.0, rate * 0.49) / rate;
        (w.cos(), w.sin(), w.sin() / (2.0 * q))
    }

    pub fn low_pass(hz: f64, q: f64, rate: f64) -> Coefs {
        let (c, _, a) = Coefs::w(hz, q, rate);
        Coefs::new(
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
            [1.0 + a, -2.0 * c, 1.0 - a],
        )
    }

    pub fn high_pass(hz: f64, q: f64, rate: f64) -> Coefs {
        let (c, _, a) = Coefs::w(hz, q, rate);
        Coefs::new(
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
            [1.0 + a, -2.0 * c, 1.0 - a],
        )
    }

    /// Passes every frequency at the same level, turning the phase by a
    /// half turn at `hz`. With a Q of a half root two it equals the sum
    /// of the two halves of a fourth-order crossover there.
    pub fn all_pass(hz: f64, q: f64, rate: f64) -> Coefs {
        let (c, _, a) = Coefs::w(hz, q, rate);
        Coefs::new([1.0 - a, -2.0 * c, 1.0 + a], [1.0 + a, -2.0 * c, 1.0 - a])
    }

    pub fn peak(hz: f64, q: f64, db: f64, rate: f64) -> Coefs {
        let g = 10f64.powf(db / 40.0);
        let (c, _, a) = Coefs::w(hz, q, rate);
        Coefs::new(
            [1.0 + a * g, -2.0 * c, 1.0 - a * g],
            [1.0 + a / g, -2.0 * c, 1.0 - a / g],
        )
    }

    /// A shelf with a slope of 1 (the steepest without overshoot).
    pub fn low_shelf(hz: f64, db: f64, rate: f64) -> Coefs {
        let g = 10f64.powf(db / 40.0);
        let (c, s, _) = Coefs::w(hz, 1.0, rate);
        let a = s / 2.0 * 2f64.sqrt();
        let r = 2.0 * g.sqrt() * a;
        Coefs::new(
            [
                g * ((g + 1.0) - (g - 1.0) * c + r),
                2.0 * g * ((g - 1.0) - (g + 1.0) * c),
                g * ((g + 1.0) - (g - 1.0) * c - r),
            ],
            [
                (g + 1.0) + (g - 1.0) * c + r,
                -2.0 * ((g - 1.0) + (g + 1.0) * c),
                (g + 1.0) + (g - 1.0) * c - r,
            ],
        )
    }

    pub fn high_shelf(hz: f64, db: f64, rate: f64) -> Coefs {
        let g = 10f64.powf(db / 40.0);
        let (c, s, _) = Coefs::w(hz, 1.0, rate);
        let a = s / 2.0 * 2f64.sqrt();
        let r = 2.0 * g.sqrt() * a;
        Coefs::new(
            [
                g * ((g + 1.0) + (g - 1.0) * c + r),
                -2.0 * g * ((g - 1.0) + (g + 1.0) * c),
                g * ((g + 1.0) + (g - 1.0) * c - r),
            ],
            [
                (g + 1.0) - (g - 1.0) * c + r,
                2.0 * ((g - 1.0) - (g + 1.0) * c),
                (g + 1.0) - (g - 1.0) * c - r,
            ],
        )
    }

    /// The gain at `hz`, as a factor, for tests.
    #[cfg(test)]
    pub fn magnitude(&self, hz: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * hz / rate;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (
            self.b0 + self.b1 * z1.0 + self.b2 * z2.0,
            self.b1 * z1.1 + self.b2 * z2.1,
        );
        let den = (
            1.0 + self.a1 * z1.0 + self.a2 * z2.0,
            self.a1 * z1.1 + self.a2 * z2.1,
        );
        (num.0.hypot(num.1)) / (den.0.hypot(den.1))
    }
}

/// One filter for one channel.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub c: Coefs,
    s1: f64,
    s2: f64,
}

impl Default for Biquad {
    fn default() -> Biquad {
        Biquad {
            c: Coefs::THROUGH,
            s1: 0.0,
            s2: 0.0,
        }
    }
}

impl Biquad {
    pub fn new(c: Coefs) -> Biquad {
        Biquad {
            c,
            ..Default::default()
        }
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let c = &self.c;
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        if self.s1.abs() < 1e-30 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-30 {
            self.s2 = 0.0;
        }
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn db(g: f64) -> f64 {
        20.0 * g.log10()
    }

    #[test]
    fn peak_boosts_its_frequency_and_leaves_the_rest() {
        let c = Coefs::peak(1000.0, 1.0, 6.0, RATE);
        assert!((db(c.magnitude(1000.0, RATE)) - 6.0).abs() < 0.01);
        assert!(db(c.magnitude(50.0, RATE)).abs() < 0.1);
        assert!(db(c.magnitude(15000.0, RATE)).abs() < 0.2);
    }

    #[test]
    fn shelves_reach_their_gain() {
        let lo = Coefs::low_shelf(200.0, -9.0, RATE);
        assert!((db(lo.magnitude(20.0, RATE)) + 9.0).abs() < 0.2);
        assert!(db(lo.magnitude(10000.0, RATE)).abs() < 0.1);
        let hi = Coefs::high_shelf(5000.0, 6.0, RATE);
        assert!((db(hi.magnitude(20000.0, RATE)) - 6.0).abs() < 0.3);
        assert!(db(hi.magnitude(100.0, RATE)).abs() < 0.1);
    }

    #[test]
    fn cuts_are_3_db_down_at_their_frequency_with_q_of_a_half_root_two() {
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let hp = Coefs::high_pass(100.0, q, RATE);
        assert!((db(hp.magnitude(100.0, RATE)) + 3.01).abs() < 0.05);
        assert!(db(hp.magnitude(25.0, RATE)) < -20.0);
        let lp = Coefs::low_pass(5000.0, q, RATE);
        assert!((db(lp.magnitude(5000.0, RATE)) + 3.01).abs() < 0.05);
        assert!(db(lp.magnitude(100.0, RATE)).abs() < 0.01);
    }

    #[test]
    fn running_matches_the_response() {
        let mut f = Biquad {
            c: Coefs::peak(1000.0, 2.0, 12.0, RATE),
            ..Default::default()
        };
        let mut peak = 0f32;
        for n in 0..48_000 {
            let x = (2.0 * PI * 1000.0 * n as f64 / RATE).sin() as f32;
            let y = f.run(x);
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((db(peak as f64) - 12.0).abs() < 0.05, "{peak}");
    }
}
