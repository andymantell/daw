// SPDX-License-Identifier: GPL-3.0-or-later
//! A stereo echo with feedback, a tone control on the repeats and an
//! option to bounce them between left and right.

use crate::biquad::{Biquad, Coefs};
use crate::{Dsp, Param, Unit, flush};

pub const PARAMS: &[Param] = &[
    Param::new("time", "Time", 1.0, 2000.0, 375.0, Unit::Ms).log(),
    Param::new("feedback", "Repeats", 0.0, 95.0, 35.0, Unit::Percent),
    Param::new("mix", "Mix", 0.0, 100.0, 25.0, Unit::Percent),
    Param::new("tone", "Tone", 1000.0, 20000.0, 6000.0, Unit::Hz)
        .log()
        .off_at(20000.0),
    Param::new(
        "ping_pong",
        "Bounce left and right",
        0.0,
        1.0,
        0.0,
        Unit::Switch,
    ),
];

const MAX_SECONDS: f64 = 2.0;

pub struct Delay {
    rate: f64,
    lines: [Vec<f32>; 2],
    at: usize,
    /// The delay in frames now, and where it is going this chunk.
    frames: f64,
    to: f64,
    feedback: f32,
    wet: f32,
    tone: [Biquad; 2],
    ping_pong: bool,
}

impl Delay {
    pub fn new(rate: f64) -> Delay {
        let len = (MAX_SECONDS * rate) as usize + 4;
        Delay {
            rate,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            frames: -1.0,
            to: 0.0,
            feedback: 0.0,
            wet: 0.0,
            tone: Default::default(),
            ping_pong: false,
        }
    }
}

impl Dsp for Delay {
    fn update(&mut self, v: &[f64]) {
        let max = self.lines[0].len() as f64 - 3.0;
        self.to = (v[0] / 1000.0 * self.rate).clamp(1.0, max);
        if self.frames < 0.0 {
            self.frames = self.to;
        }
        self.feedback = (v[1] / 100.0) as f32;
        self.wet = (v[2] / 100.0) as f32;
        let c = if PARAMS[3].off_at == Some(v[3]) {
            Coefs::THROUGH
        } else {
            Coefs::low_pass(v[3], std::f64::consts::FRAC_1_SQRT_2, self.rate)
        };
        for t in &mut self.tone {
            t.c = c;
        }
        self.ping_pong = v[4] >= 0.5;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().max(1);
        let step = (self.to - self.frames) / n as f64;
        let len = self.lines[0].len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.frames += step;
            let el = crate::delay_read(&self.lines[0], self.at, self.frames);
            let er = crate::delay_read(&self.lines[1], self.at, self.frames);
            let fl = self.tone[0].run(el) * self.feedback;
            let fr = self.tone[1].run(er) * self.feedback;
            let (wl, wr) = if self.ping_pong {
                ((*l + *r) * 0.5 + fr, fl)
            } else {
                (*l + fl, *r + fr)
            };
            self.lines[0][self.at] = flush(wl);
            self.lines[1][self.at] = flush(wr);
            self.at = (self.at + 1) % len;
            *l = *l * (1.0 - self.wet) + el * self.wet;
            *r = *r * (1.0 - self.wet) + er * self.wet;
        }
        self.frames = self.to;
    }
}

#[cfg(test)]
mod tests {
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;

    #[test]
    fn echoes_arrive_on_time_and_die_away() {
        let e = Effect::new(Kind::Delay);
        e.set(0, 100.0);
        e.set(1, 50.0);
        e.set(2, 100.0);
        e.set(3, 20000.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 48_000];
        l[0] = 1.0;
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        // All wet: the click itself is gone, its echoes every 100 ms at
        // half the level each time.
        assert!(l[0].abs() < 1e-6);
        for k in 1..5 {
            let at = k * 4800;
            let want = 0.5f32.powi(k as i32 - 1);
            assert!((l[at] - want).abs() < 1e-3, "echo {k}: {}", l[at]);
            assert!(l[at - 10].abs() < 1e-6);
        }
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let e = Effect::new(Kind::Delay);
        e.set(0, 100.0);
        e.set(1, 50.0);
        e.set(2, 100.0);
        e.set(3, 20000.0);
        e.set(4, 1.0);
        let mut p = e.processor(48_000);
        let mut l = vec![0f32; 48_000];
        let mut r = vec![0f32; 48_000];
        l[0] = 1.0;
        r[0] = 1.0;
        p.process(&mut l, &mut r);
        assert!(l[4800] > 0.9 && r[4800].abs() < 1e-6);
        assert!(r[9600] > 0.4 && l[9600].abs() < 1e-6);
        assert!(l[14400] > 0.2 && r[14400].abs() < 1e-6);
    }
}
