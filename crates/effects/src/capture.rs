// SPDX-License-Identifier: GPL-3.0-or-later
//! Amp captures: a real amp or pedal's sound, learned by a small neural
//! network from recordings of it, in the open capture file format
//! (`.nam`). The user brings their own files; none come with the app.
//!
//! The network runs in the `nam-rs` crate (MIT licence; see decision
//! 0012). A capture only sounds right at the sample rate it was made at,
//! so when the project runs at another rate the sound is converted to
//! that rate and back around it ([`crate::resample`]), at the cost of a
//! short delay that the track makes up for.

use std::path::Path;

use crate::biquad::{Biquad, Coefs};
use crate::resample::Resampler;
use crate::{CHUNK, Dsp, Param, Unit};

pub const PARAMS: &[Param] = &[
    Param::new("input", "Input", -24.0, 24.0, 0.0, Unit::Db),
    Param::new("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
    Param::new("match", "Even out loudness", 0.0, 1.0, 1.0, Unit::Switch),
];

/// Where "Even out loudness" brings a capture that says how loud it is,
/// in dB (RMS, as capture files measure it).
const TARGET_DB: f32 = -18.0;

/// A capture file, read and checked.
#[derive(Debug)]
pub struct Capture {
    model: nam_rs::NamModel,
    /// The rate it was made at, in Hz.
    pub rate: f64,
    /// What its maker called it, if they did.
    pub name: Option<String>,
    /// Who made it, if it says.
    pub by: Option<String>,
    /// How loud it plays a standard input, in dB, if it says.
    pub loudness: Option<f32>,
    /// Whether a speaker cabinet is part of it, if it says.
    pub includes_cab: Option<bool>,
}

impl Capture {
    pub fn from_file(path: &Path) -> Result<Capture, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Capture::from_text(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn from_text(text: &str) -> Result<Capture, String> {
        let model = nam_rs::NamModel::from_json_str(text).map_err(|e| e.to_string())?;
        // Built once here so a file that can't be played is found now,
        // not on the audio thread.
        nam_rs::Model::from_nam(&model).map_err(|e| e.to_string())?;
        let rate = model.expected_sample_rate();
        if !(8_000.0..=384_000.0).contains(&rate) {
            return Err(format!("it says it was made at {rate} Hz"));
        }
        let meta = model.metadata_typed();
        Ok(Capture {
            rate,
            name: meta.name.clone().filter(|n| !n.trim().is_empty()),
            by: meta.modeled_by.clone().filter(|n| !n.trim().is_empty()),
            loudness: meta.loudness.filter(|l| l.is_finite()),
            includes_cab: meta.includes_cab(),
            model,
        })
    }
}

/// Converting to the capture's rate and back.
struct Convert {
    up: Resampler,
    down: Resampler,
    /// At the capture's rate.
    there: Vec<f32>,
    /// Back at the project's rate, before the queue.
    back: Vec<f32>,
    /// What has come back, waiting to be played, as a ring.
    queue: Vec<f32>,
    read: usize,
    write: usize,
    delay: usize,
}

impl Convert {
    fn new(rate: f64, capture: f64) -> Convert {
        let up = Resampler::new(rate, capture);
        let down = Resampler::new(capture, rate);
        let there = vec![0.0; up.most_out(CHUNK)];
        let back = vec![0.0; down.most_out(there.len())];
        let look = Resampler::lookahead() as f64;
        // Each converter holds back its lookahead (the second's at the
        // capture's rate), and each may be a sample behind as their
        // times fall.
        let delay = (look + look * rate / capture).ceil() as usize + 3;
        let size = (delay + 4 * CHUNK).next_power_of_two();
        Convert {
            up,
            down,
            there,
            back,
            queue: vec![0.0; size],
            read: 0,
            write: delay,
            delay,
        }
    }

    /// Runs `io` through `model` at the capture's rate.
    fn run(&mut self, io: &mut [f32], mut model: impl FnMut(&mut [f32])) {
        let n = self.up.process(io, &mut self.there);
        model(&mut self.there[..n]);
        let m = self.down.process(&self.there[..n], &mut self.back);
        let mask = self.queue.len() - 1;
        for &x in &self.back[..m] {
            self.queue[self.write & mask] = x;
            self.write += 1;
        }
        for x in io.iter_mut() {
            if self.read < self.write {
                *x = self.queue[self.read & mask];
                self.read += 1;
            } else {
                *x = 0.0;
            }
        }
    }
}

/// The capture player.
pub struct Player {
    /// None until a capture is chosen: the sound passes unchanged.
    model: Option<nam_rs::Model>,
    convert: Option<Convert>,
    input: f32,
    output: f32,
    /// Brings the capture to [`TARGET_DB`], when it says how loud it is.
    even: f32,
    loudness: Option<f32>,
    /// Takes out the steady offset some captures add.
    dc: Biquad,
    mono: [f32; CHUNK],
}

impl Player {
    /// Plays `capture` at `rate`, or passes the sound through without
    /// one. Allocates.
    pub fn new(rate: f64, capture: Option<&Capture>) -> Player {
        let mut model = capture.and_then(|c| nam_rs::Model::from_nam(&c.model).ok());
        if let Some(m) = &mut model {
            // Settles the network against silence, so it starts clean.
            let mut warm = vec![0f32; m.receptive_field()];
            m.process_buffer(&mut warm);
        }
        let convert = (capture.zip(model.as_ref()))
            .filter(|(c, _)| (c.rate - rate).abs() > 0.5)
            .map(|(c, _)| Convert::new(rate, c.rate));
        Player {
            model,
            convert,
            input: 1.0,
            output: 1.0,
            even: 1.0,
            loudness: capture.and_then(|c| c.loudness),
            dc: Biquad::new(Coefs::high_pass(10.0, 0.7, rate)),
            mono: [0.0; CHUNK],
        }
    }
}

impl Dsp for Player {
    fn update(&mut self, v: &[f64]) {
        self.input = crate::gain(v[0]) as f32;
        self.output = crate::gain(v[1]) as f32;
        self.even = match self.loudness {
            Some(l) if v[2] >= 0.5 => crate::gain((TARGET_DB - l) as f64) as f32,
            _ => 1.0,
        };
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Some(model) = &mut self.model else {
            return;
        };
        let n = left.len().min(right.len()).min(CHUNK);
        let io = &mut self.mono[..n];
        for (i, x) in io.iter_mut().enumerate() {
            *x = (left[i] + right[i]) * 0.5 * self.input;
        }
        match &mut self.convert {
            Some(c) => c.run(io, |b| model.process_buffer(b)),
            None => model.process_buffer(io),
        }
        let g = self.output * self.even;
        for (i, x) in io.iter().enumerate() {
            let y = self.dc.run(*x) * g;
            let y = if y.is_finite() { y } else { 0.0 };
            left[i] = y;
            right[i] = y;
        }
    }

    fn latency(&self) -> u32 {
        self.convert.as_ref().map_or(0, |c| c.delay as u32)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Effect, Kind};
    use daw_engine::graph::Insert;
    use std::f64::consts::PI;
    use std::sync::Arc;

    /// A small recurrent network, with weights of no meaning.
    fn small_lstm() -> String {
        let weights: Vec<String> = (0..16)
            .map(|i| format!("{:.3}", ((i * 37) % 11) as f32 / 11.0 - 0.5))
            .collect();
        format!(
            r#"{{"version":"0.5.4","architecture":"LSTM",
            "config":{{"input_size":1,"hidden_size":1,"num_layers":1}},
            "weights":[{}],"sample_rate":48000,
            "metadata":{{"name":"Small","loudness":-24.0}}}}"#,
            weights.join(",")
        )
    }

    fn play(e: &Effect, input: &[f32], rate: u32, block: usize) -> (Vec<f32>, u32) {
        let mut p = e.processor(rate);
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            p.process(a, b);
        }
        (l, p.latency())
    }

    #[test]
    fn files_are_read_and_checked() {
        let c = Capture::from_text(&small_lstm()).unwrap();
        assert_eq!(c.rate, 48_000.0);
        assert_eq!(c.name.as_deref(), Some("Small"));
        assert_eq!(c.loudness, Some(-24.0));
        assert!(Capture::from_text("{}").is_err());
        // Weights that don't fit the network.
        let bad = small_lstm().replace("\"num_layers\":1", "\"num_layers\":2");
        assert!(Capture::from_text(&bad).is_err());
    }

    #[test]
    fn without_a_capture_the_sound_passes_unchanged() {
        let e = Effect::new(Kind::Capture);
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let (out, latency) = play(&e, &input, 48_000, 100);
        assert_eq!(out, input);
        assert_eq!(latency, 0);
    }

    /// Converted to another rate and back, the sound comes out exactly
    /// as late as it says, at the right level, whatever the block size.
    #[test]
    fn converting_rates_keeps_time() {
        for (rate, made) in [
            (44_100.0, 48_000.0),
            (48_000.0, 44_100.0),
            (96_000.0, 48_000.0),
        ] {
            let hz = 440.0;
            let input: Vec<f32> = (0..rate as usize / 2)
                .map(|n| (2.0 * PI * hz * n as f64 / rate).sin() as f32 * 0.5)
                .collect();
            for block in [1, 31, 32] {
                let mut c = Convert::new(rate, made);
                let mut out = input.clone();
                for b in out.chunks_mut(block) {
                    c.run(b, |x| x.iter_mut().for_each(|v| *v *= 0.5));
                }
                let d = c.delay;
                let err = (d + 100..out.len())
                    .map(|n| (out[n] - input[n - d] * 0.5).abs())
                    .fold(0f32, f32::max);
                assert!(err < 1e-3, "{rate} from {made}, blocks of {block}: {err}");
            }
        }
    }

    #[test]
    fn loudness_is_evened_out_when_the_file_says_how_loud() {
        let e = Effect::new(Kind::Capture);
        let c = Capture::from_text(&small_lstm()).unwrap();
        e.set_capture("small.nam".into(), Some(Arc::new(c)));
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.03).sin() * 0.3).collect();
        let (on, _) = play(&e, &input, 48_000, 64);
        e.set(2, 0.0);
        let (off, latency) = play(&e, &input, 48_000, 64);
        assert_eq!(latency, 0);
        // -24 dB to -18 dB: twice as loud.
        let at = (0..on.len()).find(|&i| off[i].abs() > 0.01).unwrap();
        assert!(
            (on[at] / off[at] - 2.0).abs() < 0.01,
            "{} {}",
            on[at],
            off[at]
        );
    }

    #[test]
    fn at_another_rate_it_says_how_late_it_is() {
        let e = Effect::new(Kind::Capture);
        let c = Capture::from_text(&small_lstm()).unwrap();
        e.set_capture("small.nam".into(), Some(Arc::new(c)));
        let (_, latency) = play(&e, &[0.0; 64], 44_100, 64);
        assert!(latency > 32 && latency < 100, "{latency}");
    }

    #[test]
    fn a_real_network_plays_finite_sound() {
        let e = Effect::new(Kind::Capture);
        e.set_capture(
            "lstm.nam".into(),
            Some(Arc::new(Capture::from_text(&small_lstm()).unwrap())),
        );
        let input: Vec<f32> = (0..9600).map(|i| (i as f32 * 0.03).sin()).collect();
        let (out, _) = play(&e, &input, 44_100, 128);
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(out.iter().any(|x| x.abs() > 1e-4));
    }
}
