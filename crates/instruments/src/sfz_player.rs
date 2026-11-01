// SPDX-License-Identifier: GPL-3.0-or-later
//! Playing an SFZ library ([`crate::library`]) on the audio thread: the
//! regions a note falls in (by key, velocity, controllers, round robin,
//! a random number and key-switches) each start a voice, with its own
//! envelope, tuning, level, pan and filters; choke groups stop the
//! voices they name; controllers move levels, pans and tuning while the
//! voices sound; letting a key go starts its release regions.
//!
//! Controllers start where the instrument's settings put them (one
//! setting for each, `ccN`), so a library's mic levels are faders in
//! the window and settings an assistant can set. A controller played on
//! a keyboard moves it from there until the setting next changes; the
//! same goes for the key-switch.

use std::sync::Arc;

use daw_engine::midi::{Event, Kind};

use crate::dsp::{Mode, SVF_ZERO, Svf, fall};
use crate::library::{Library, Source};
use crate::params::Params;
use crate::sample::hermite;
use crate::sfz::{Fader, FilterKind, LoopMode, OffMode, Region, Target, Trigger};
use crate::stream::{Cursor, Streams};

pub const VOICES: usize = 96;
const CHUNK: usize = 32;
/// Seconds a choked voice takes to stop when its file says "fast".
const FAST: f32 = 0.006;

/// Where the player's settings are, by index.
#[derive(Clone, Debug)]
pub struct Knobs {
    pub level: usize,
    pub normalise: Option<usize>,
    pub keyswitch: Option<usize>,
    /// The setting for each controller, if it has one.
    pub cc: Vec<(u8, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Attack,
    Hold,
    Decay,
    Release,
    Idle,
}

/// A voice's amplitude envelope, its times worked out at its start.
#[derive(Clone, Copy, Debug)]
struct Env {
    stage: Stage,
    level: f32,
    attack: f32,
    hold: u32,
    decay: f32,
    sustain: f32,
    release: f32,
    count: u32,
}

impl Env {
    const IDLE: Env = Env {
        stage: Stage::Idle,
        level: 0.0,
        attack: 1.0,
        hold: 0,
        decay: 0.0,
        sustain: 1.0,
        release: 0.0,
        count: 0,
    };

    #[inline]
    fn next(&mut self) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level += self.attack;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Hold;
                    self.count = 0;
                }
            }
            Stage::Hold => {
                self.count += 1;
                if self.count >= self.hold {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.sustain + (self.level - self.sustain) * self.decay;
                if self.sustain <= 0.0 && self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Release => {
                self.level *= self.release;
                if self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Idle => {}
        }
        self.level
    }

    fn release(&mut self, seconds: f32, rate: f32) {
        if self.stage != Stage::Idle {
            self.release = fall(seconds.max(0.0005), rate);
            self.stage = Stage::Release;
        }
    }
}

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    region: u32,
    source: u32,
    key: u8,
    channel: u8,
    velocity: u8,
    held: bool,
    /// Released while the sustain pedal is down.
    pedalled: bool,
    pos: f64,
    /// The source's rate over the output's.
    ratio: f64,
    /// Cents from its key and fixed tuning.
    cents: f32,
    end: f64,
    start: f64,
    looping: Option<(f64, f64)>,
    reverse: bool,
    gain: f32,
    /// The level last used, for moving smoothly to the next.
    last: (f32, f32),
    env: Env,
    /// Frames before it sounds.
    wait: u32,
    /// Fading out after being choked: the step down each frame.
    fade: f32,
    fading: f32,
    cursor: Option<Cursor>,
    stalled: u32,
    age: u64,
    filters: [[Svf; 2]; 2],
}

impl Voice {
    const OFF: Voice = Voice {
        on: false,
        region: 0,
        source: 0,
        key: 0,
        channel: 0,
        velocity: 0,
        held: false,
        pedalled: false,
        pos: 0.0,
        ratio: 1.0,
        cents: 0.0,
        end: 0.0,
        start: 0.0,
        looping: None,
        reverse: false,
        gain: 0.0,
        last: (0.0, 0.0),
        env: Env::IDLE,
        wait: 0,
        fade: 0.0,
        fading: 1.0,
        cursor: None,
        stalled: 0,
        age: 0,
        filters: [[SVF_ZERO; 2]; 2],
    };
}

/// A library playing.
pub struct SfzPlayer {
    lib: Arc<Library>,
    params: Arc<Params>,
    knobs: Knobs,
    rate: f32,
    voices: Box<[Voice; VOICES]>,
    streams: Option<Streams>,
    /// Controllers' positions, and the settings they were last read
    /// from (to tell when a setting moves).
    cc: [f32; 128],
    cc_set: [f32; 128],
    keyswitch: Option<u8>,
    keyswitch_set: f32,
    down: [bool; 128],
    /// Each key's last velocity and when it went down, for its release
    /// regions.
    key_velocity: [u8; 128],
    key_time: [u64; 128],
    previous: Option<u8>,
    /// How many times each region has been in line to play.
    seq: Vec<u32>,
    bend: f32,
    pedal: bool,
    clock: u64,
    frames: u64,
    random: u32,
    stalls: u64,
    /// Rendering offline: a voice waits for the disk rather than hold
    /// still.
    offline: bool,
}

impl SfzPlayer {
    /// Allocates (and starts the disk loader for a library that plays
    /// from disk); not for the audio thread.
    pub fn new(rate: u32, lib: Arc<Library>, params: Arc<Params>, knobs: Knobs) -> SfzPlayer {
        let streams = lib.streamed.then(|| Streams::new(lib.clone(), VOICES));
        let mut cc = [0.0; 128];
        for (&c, &v) in &lib.sfz.cc_start {
            cc[c as usize] = v;
        }
        let mut p = SfzPlayer {
            seq: vec![0; lib.sfz.regions.len()],
            keyswitch: lib.sfz.sw_default,
            lib,
            params,
            knobs,
            rate: rate.max(1) as f32,
            voices: Box::new([Voice::OFF; VOICES]),
            streams,
            cc,
            cc_set: [f32::NAN; 128],
            keyswitch_set: f32::NAN,
            down: [false; 128],
            key_velocity: [0; 128],
            key_time: [0; 128],
            previous: None,
            bend: 0.0,
            pedal: false,
            clock: 0,
            frames: 0,
            random: 0x9e37_79b9,
            stalls: 0,
            offline: false,
        };
        p.read_settings();
        p
    }

    /// For rendering offline, faster than the disk might keep up with:
    /// a voice waits for it.
    pub fn offline(mut self) -> SfzPlayer {
        self.offline = true;
        self
    }

    /// Voices sounding now.
    pub fn sounding(&self) -> usize {
        self.voices.iter().filter(|v| v.on).count()
    }

    /// Samples voices have waited for the disk.
    pub fn stalls(&self) -> u64 {
        self.stalls
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.random;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Takes the controllers and key-switch from the settings that have
    /// moved since last read.
    fn read_settings(&mut self) {
        for &(c, i) in &self.knobs.cc {
            let v = self.params.read(i);
            if v != self.cc_set[c as usize] {
                self.cc_set[c as usize] = v;
                self.cc[c as usize] = v;
            }
        }
        if let Some(i) = self.knobs.keyswitch {
            let v = self.params.read(i);
            if v != self.keyswitch_set {
                self.keyswitch_set = v;
                self.keyswitch = Some(v.clamp(0.0, 127.0) as u8);
            }
        }
    }

    fn region(&self, i: u32) -> &Region {
        &self.lib.sfz.regions[i as usize]
    }

    fn cc_ok(&self, r: &Region) -> bool {
        r.cc.iter().all(|&(c, lo, hi)| {
            let v = self.cc[c as usize];
            v >= lo && v <= hi
        })
    }

    fn switches_ok(&self, r: &Region) -> bool {
        if let Some((lo, hi)) = r.sw_last
            && !self.keyswitch.is_some_and(|k| (lo..=hi).contains(&k))
        {
            return false;
        }
        if r.sw_down.is_some_and(|k| !self.down[k as usize]) {
            return false;
        }
        if r.sw_up.is_some_and(|k| self.down[k as usize]) {
            return false;
        }
        if r.sw_previous.is_some() && r.sw_previous != self.previous {
            return false;
        }
        true
    }

    fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        let key = key.min(127);
        let velocity = velocity.clamp(1, 127);
        // A key-switch.
        if self.lib.switches[key as usize] {
            self.keyswitch = Some(key);
        }
        let others = self.down.iter().filter(|d| **d).count();
        let r = self.rand();
        self.clock += 1;
        let stamp = self.clock;
        let lib = self.lib.clone();
        for &ri in &lib.by_key[key as usize] {
            let reg = &lib.sfz.regions[ri as usize];
            if !(reg.vel.0..=reg.vel.1).contains(&velocity)
                || !(reg.chan.0..=reg.chan.1).contains(&(channel + 1))
            {
                continue;
            }
            match reg.trigger {
                Trigger::First if others > 0 => continue,
                Trigger::Legato if others == 0 => continue,
                _ => {}
            }
            if !self.cc_ok(reg) || !self.switches_ok(reg) {
                continue;
            }
            if r < reg.rand.0 || r >= reg.rand.1 {
                continue;
            }
            // Round robin.
            let n = self.seq[ri as usize];
            self.seq[ri as usize] = n.wrapping_add(1);
            if reg.seq_length > 1 && n % reg.seq_length != reg.seq_position - 1 {
                continue;
            }
            self.start(ri, channel, key, velocity, 1.0, stamp);
        }
        self.down[key as usize] = true;
        self.key_velocity[key as usize] = velocity;
        self.key_time[key as usize] = self.frames;
        self.previous = Some(key);
    }

    fn note_off(&mut self, channel: u8, key: u8) {
        let key = key.min(127);
        self.down[key as usize] = false;
        for i in 0..VOICES {
            let v = self.voices[i];
            if !(v.on && v.held && v.key == key && v.channel == channel) {
                continue;
            }
            self.voices[i].held = false;
            if self.pedal {
                self.voices[i].pedalled = true;
            } else {
                self.let_go(i);
            }
        }
        // Release regions.
        let velocity = self.key_velocity[key as usize].max(1);
        let held = (self.frames - self.key_time[key as usize]) as f32 / self.rate;
        let r = self.rand();
        self.clock += 1;
        let stamp = self.clock;
        let lib = self.lib.clone();
        for &ri in &lib.releases[key as usize] {
            let reg = &lib.sfz.regions[ri as usize];
            if !(reg.vel.0..=reg.vel.1).contains(&velocity)
                || !self.cc_ok(reg)
                || !self.switches_ok(reg)
                || r < reg.rand.0
                || r >= reg.rand.1
            {
                continue;
            }
            let n = self.seq[ri as usize];
            self.seq[ri as usize] = n.wrapping_add(1);
            if reg.seq_length > 1 && n % reg.seq_length != reg.seq_position - 1 {
                continue;
            }
            let g = gain_db(-reg.rt_decay * held);
            self.start(ri, channel, key, velocity, g, stamp);
            // It plays on, whatever the key does now.
            for v in self.voices.iter_mut().filter(|v| v.on && v.age == stamp) {
                v.held = false;
            }
        }
    }

    /// The voice lets go: its release, unless it plays to its end.
    fn let_go(&mut self, i: usize) {
        let v = &mut self.voices[i];
        let r = &self.lib.sfz.regions[v.region as usize];
        match r.loop_mode {
            LoopMode::OneShot => {}
            LoopMode::Sustain => {
                v.looping = None;
                v.env.release(r.eg.release, self.rate);
            }
            _ => v.env.release(r.eg.release, self.rate),
        }
    }

    /// Stops voice `i` as `mode` says.
    fn choke(&mut self, i: usize, mode: OffMode) {
        let rate = self.rate;
        let release = self.region(self.voices[i].region).eg.release;
        let v = &mut self.voices[i];
        match mode {
            OffMode::Normal => v.env.release(release, rate),
            OffMode::Fast => v.fade = 1.0 / (FAST * rate),
            OffMode::Time(t) => v.fade = 1.0 / (t.max(0.0005) * rate),
        }
        if v.fade > 0.0 {
            v.looping = None;
        }
    }

    fn mods(&self, r: &Region, t: Target) -> f32 {
        r.mods
            .iter()
            .filter(|m| m.target == t)
            .map(|m| m.amount * self.lib.sfz.curve(m.curve, self.cc[m.cc as usize]))
            .sum()
    }

    fn amplitude(&self, r: &Region) -> f32 {
        let mut a = r.amplitude / 100.0;
        for m in r.mods.iter().filter(|m| m.target == Target::Amplitude) {
            a *= m.amount / 100.0 * self.lib.sfz.curve(m.curve, self.cc[m.cc as usize]);
        }
        a
    }

    fn start(&mut self, ri: u32, channel: u8, key: u8, velocity: u8, extra: f32, stamp: u64) {
        let lib = self.lib.clone();
        let reg = &lib.sfz.regions[ri as usize];
        let Some(si) = lib.region_source[ri as usize] else {
            return;
        };
        let src: &Source = &lib.sources[si];
        // Chokes: voices this region's group stops.
        for i in 0..VOICES {
            let v = self.voices[i];
            if v.on && v.age != stamp && v.fade == 0.0 {
                let other = &lib.sfz.regions[v.region as usize];
                if other.off_by == Some(reg.group) {
                    self.choke(i, other.off_mode);
                }
            }
        }
        // Polyphony in its group, and on its key.
        for (limit, same_key) in [(reg.polyphony, false), (reg.note_polyphony, true)] {
            let Some(limit) = limit else {
                continue;
            };
            loop {
                let mut count = 0;
                let mut oldest: Option<usize> = None;
                for (i, v) in self.voices.iter().enumerate() {
                    if v.on
                        && v.fade == 0.0
                        && lib.sfz.regions[v.region as usize].group == reg.group
                        && (!same_key || v.key == key)
                    {
                        count += 1;
                        if oldest.is_none_or(|o| self.voices[o].age > v.age) {
                            oldest = Some(i);
                        }
                    }
                }
                match oldest {
                    Some(o) if count >= limit as usize => self.choke(o, OffMode::Fast),
                    _ => break,
                }
            }
        }
        // Its level.
        let mut gain = reg.velocity_gain(velocity) * extra;
        gain *= gain_db(reg.amp_keytrack * (key as f32 - reg.amp_keycenter as f32));
        if reg.amp_random > 0.0 {
            gain *= gain_db(-self.rand() * reg.amp_random);
        }
        for (f, fade) in &reg.xfades {
            let at = match f {
                Fader::Key => key as f32,
                Fader::Velocity => velocity as f32,
                Fader::Cc(c) => self.cc[*c as usize],
            };
            gain *= fade.gain(at);
        }
        if gain <= 0.0 {
            return;
        }
        // Its pitch.
        let mut cents = (key as f32 - reg.keycenter as f32) * reg.keytrack
            + reg.transpose as f32 * 100.0
            + reg.tune
            + reg.pitch_veltrack * velocity as f32 / 127.0;
        if reg.pitch_random > 0.0 {
            cents += (self.rand() * 2.0 - 1.0) * reg.pitch_random;
        }
        // Where it plays.
        let frames = src.frames as f64;
        let end = reg.end.map_or(frames, |e| (e as f64 + 1.0).min(frames));
        let mut offset = reg.offset as f64 + self.mods(reg, Target::Offset) as f64;
        if reg.offset_random > 0 {
            offset += (self.rand() * reg.offset_random as f32).floor() as f64;
        }
        let offset = offset.clamp(0.0, (end - 1.0).max(0.0));
        let looping = match reg.loop_mode {
            LoopMode::Continuous | LoopMode::Sustain => {
                let a = reg.loop_start.map_or(0.0, |a| a as f64);
                let b = reg.loop_end.map_or(end, |b| (b as f64 + 1.0).min(end));
                (b - a >= 2.0).then_some((a, b))
            }
            _ => None,
        };
        // Its envelope.
        let vel = velocity as f32 / 127.0;
        let e = &reg.eg;
        let rate = self.rate;
        let attack = e.attack + e.vel2attack * vel + self.mods(reg, Target::EgAttack);
        let hold = e.hold + e.vel2hold * vel + self.mods(reg, Target::EgHold);
        let decay = e.decay + e.vel2decay * vel + self.mods(reg, Target::EgDecay);
        let sustain = ((e.sustain + e.vel2sustain * vel + self.mods(reg, Target::EgSustain))
            / 100.0)
            .clamp(0.0, 1.0);
        let start = ((e.start + self.mods(reg, Target::EgStart)) / 100.0).clamp(0.0, 1.0);
        let env = Env {
            stage: if attack > 0.0 {
                Stage::Attack
            } else {
                Stage::Hold
            },
            level: if attack > 0.0 { start } else { 1.0 },
            attack: 1.0 / (attack.max(1e-6) * rate),
            hold: (hold.max(0.0) * rate) as u32,
            decay: if decay > 0.0 { fall(decay, rate) } else { 0.0 },
            sustain,
            release: 0.0,
            count: 0,
        };
        let delay = (reg.delay
            + e.delay
            + self.mods(reg, Target::Delay)
            + self.mods(reg, Target::EgDelay)
            + if reg.delay_random > 0.0 {
                self.rand() * reg.delay_random
            } else {
                0.0
            })
        .max(0.0);
        // A slot: a free one, else the oldest.
        let slot = (self.voices.iter().position(|v| !v.on))
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
        if self.voices[slot].on
            && let Some(s) = self.streams.as_mut()
        {
            s.stop(slot);
        }
        let (pos, reverse) = if reg.reverse {
            (end - 1.0, true)
        } else {
            (offset, false)
        };
        let cursor = match (&mut self.streams, src.streamed) {
            (Some(s), true) => Some(s.start(slot, si, pos as u64, src.pcm.frames() as u64)),
            _ => None,
        };
        self.voices[slot] = Voice {
            on: true,
            region: ri,
            source: si as u32,
            key,
            channel,
            velocity,
            held: true,
            pedalled: false,
            pos,
            ratio: src.rate / rate as f64,
            cents,
            end,
            start: offset,
            looping,
            reverse,
            gain,
            last: (f32::NAN, 0.0),
            env,
            wait: (delay * rate) as u32,
            fade: 0.0,
            fading: 1.0,
            cursor,
            stalled: 0,
            age: stamp,
            filters: [[SVF_ZERO; 2]; 2],
        };
    }

    fn control(&mut self, number: u8, value: u8) {
        let before = self.cc[number as usize];
        self.cc[number as usize] = value as f32;
        match number {
            64 => {
                self.pedal = value >= 64;
                if !self.pedal {
                    for i in 0..VOICES {
                        if self.voices[i].on && self.voices[i].pedalled {
                            self.voices[i].pedalled = false;
                            self.let_go(i);
                        }
                    }
                }
            }
            120 | 123 => self.all_off(),
            121 => {
                self.cc_set = [f32::NAN; 128];
                self.read_settings();
            }
            _ => {}
        }
        // Regions a controller starts.
        let lib = self.lib.clone();
        let v = value as f32;
        self.clock += 1;
        let stamp = self.clock;
        for &ri in &lib.by_cc {
            let reg = &lib.sfz.regions[ri as usize];
            if let Some((c, lo, hi)) = reg.on_cc
                && c == number
                && v >= lo
                && v <= hi
                && !(before >= lo && before <= hi)
                && self.cc_ok(reg)
                && self.switches_ok(reg)
            {
                let key = reg.keycenter;
                self.start(ri, 0, key, 100, 1.0, stamp);
                for x in self.voices.iter_mut().filter(|x| x.on && x.age == stamp) {
                    x.held = false;
                }
            }
        }
    }

    fn all_off(&mut self) {
        self.pedal = false;
        for i in 0..VOICES {
            if self.voices[i].on {
                self.voices[i].held = false;
                self.voices[i].pedalled = false;
                self.let_go(i);
            }
        }
        self.down = [false; 128];
    }

    pub fn event(&mut self, e: &Event) {
        match e.kind {
            Kind::NoteOn {
                pitch, velocity, ..
            } if velocity > 0 => self.note_on(e.channel, pitch, velocity),
            Kind::NoteOn { pitch, .. } | Kind::NoteOff { pitch, .. } => {
                self.note_off(e.channel, pitch)
            }
            Kind::PitchBend(b) => self.bend = b as f32 / 8192.0,
            Kind::Control { number, value } => self.control(number, value),
            Kind::AllNotesOff => self.all_off(),
            _ => {}
        }
    }

    /// Adds `from..to` of the output into `left` and `right`.
    pub fn run(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        let mut at = from;
        while at < to {
            let end = (at + CHUNK).min(to);
            self.chunk(at, end, left, right);
            self.frames += (end - at) as u64;
            at = end;
        }
    }

    fn chunk(&mut self, from: usize, to: usize, left: &mut [f32], right: &mut [f32]) {
        self.read_settings();
        let lib = self.lib.clone();
        let mut level = crate::dsp::db(self.params.read(self.knobs.level));
        if self
            .knobs
            .normalise
            .is_some_and(|i| self.params.read(i) >= 0.5)
        {
            level *= lib.normal_gain();
        }
        let n = (to - from) as f32;
        for vi in 0..VOICES {
            if !self.voices[vi].on {
                continue;
            }
            let reg = &lib.sfz.regions[self.voices[vi].region as usize];
            let src = &lib.sources[self.voices[vi].source as usize];
            // What the controllers make of it now.
            let g = self.voices[vi].gain
                * self.amplitude(reg)
                * gain_db(reg.volume + self.mods(reg, Target::Volume))
                * level;
            let pan = ((reg.pan + self.mods(reg, Target::Pan)) / 100.0).clamp(-1.0, 1.0);
            let (pl, pr) = crate::dsp::pan(pan);
            let target = (g * pl, g * pr);
            let bend = if self.bend >= 0.0 {
                self.bend * reg.bend_up
            } else {
                -self.bend * reg.bend_down
            };
            let cents = self.voices[vi].cents + self.mods(reg, Target::Tune) + bend;
            let mut filters: [Option<(Mode, f32, f32)>; 2] = [None, None];
            for (k, f) in reg.filters.iter().enumerate() {
                let Some(f) = f else {
                    continue;
                };
                let t = if k == 0 {
                    Target::Cutoff
                } else {
                    Target::Cutoff2
                };
                let c = self.mods(reg, t)
                    + f.veltrack * self.voices[vi].velocity as f32 / 127.0
                    + f.keytrack * (self.voices[vi].key as f32 - f.keycenter as f32);
                let hz = f.cutoff * (c / 1200.0).exp2();
                let mode = match f.kind {
                    FilterKind::Low => Mode::Low,
                    FilterKind::High => Mode::High,
                    FilterKind::Band => Mode::Band,
                };
                filters[k] = Some((mode, hz, (f.resonance / 30.0).clamp(0.0, 0.95)));
            }
            let rate = self.rate;
            let wait = self.offline;
            let streams = &mut self.streams;
            let v = &mut self.voices[vi];
            for (k, f) in filters.iter().enumerate() {
                if let Some((_, hz, res)) = f {
                    for s in &mut v.filters[k] {
                        s.set(*hz, *res, rate);
                    }
                }
            }
            let step = v.ratio * (cents as f64 / 1200.0).exp2();
            let from_gain = if v.last.0.is_nan() { target } else { v.last };
            v.last = target;
            let mut stopped = false;
            for (j, i) in (from..to).enumerate() {
                if v.wait > 0 {
                    v.wait -= 1;
                    continue;
                }
                // Read four frames round the position.
                let p = v.pos;
                let base = p.floor();
                let t = (p - base) as f32;
                let b = base as i64;
                let mut f = [(0.0f32, 0.0f32); 4];
                let mut missing = false;
                for (o, slot) in f.iter_mut().enumerate() {
                    let k = b - 1 + o as i64;
                    if k < 0 || k as f64 >= v.end {
                        continue;
                    }
                    *slot = match (&mut v.cursor, streams.as_mut()) {
                        (Some(c), Some(s)) => {
                            let mut got = c.frame(k as u64, src, s.ring(vi));
                            // Offline, there is time to wait for the disk.
                            let mut tries = 0;
                            while got.is_none() && wait && tries < 20_000 {
                                std::thread::sleep(std::time::Duration::from_micros(100));
                                got = c.frame(k as u64, src, s.ring(vi));
                                tries += 1;
                            }
                            match got {
                                Some(x) => x,
                                None => {
                                    missing = true;
                                    break;
                                }
                            }
                        }
                        _ => src.pcm.frame(k as usize),
                    };
                }
                if missing {
                    v.stalled += 1;
                    self.stalls += 1;
                    if v.stalled > rate as u32 {
                        stopped = true;
                        break;
                    }
                    continue;
                }
                let e = v.env.next();
                if v.env.stage == Stage::Idle {
                    stopped = true;
                    break;
                }
                if v.fade > 0.0 {
                    v.fading -= v.fade;
                    if v.fading <= 0.0 {
                        stopped = true;
                        break;
                    }
                }
                let mut a = hermite(f[0].0, f[1].0, f[2].0, f[3].0, t);
                let mut c = hermite(f[0].1, f[1].1, f[2].1, f[3].1, t);
                for (k, fl) in filters.iter().enumerate() {
                    if let Some((mode, _, _)) = fl {
                        a = v.filters[k][0].run(a, *mode);
                        c = v.filters[k][1].run(c, *mode);
                    }
                }
                let x = (j as f32 + 1.0) / n;
                let gl = from_gain.0 + (target.0 - from_gain.0) * x;
                let gr = from_gain.1 + (target.1 - from_gain.1) * x;
                let e = e * v.fading;
                left[i] += a * gl * e;
                right[i] += c * gr * e;
                // On.
                if v.reverse {
                    v.pos -= step;
                    if v.pos < v.start {
                        stopped = true;
                        break;
                    }
                } else {
                    v.pos += step;
                    if let Some((lo, hi)) = v.looping
                        && v.pos >= hi
                    {
                        v.pos = lo + (v.pos - hi) % (hi - lo);
                    }
                    if v.pos >= v.end {
                        stopped = true;
                        break;
                    }
                }
            }
            if stopped {
                v.on = false;
                if let Some(s) = streams.as_mut()
                    && v.cursor.is_some()
                {
                    s.stop(vi);
                }
            }
        }
    }
}

/// Decibels as a gain, down to nothing at −144 dB.
#[inline]
pub fn gain_db(v: f32) -> f32 {
    if v <= -144.0 {
        0.0
    } else {
        10f32.powf(v / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use daw_engine::midi::Instrument;

    use super::*;
    use crate::library::tests::scratch;
    use crate::{Builtin, Kind};

    /// A WAV of `frames` frames all at `level`.
    fn flat(dir: &Path, name: &str, frames: usize, level: f32) {
        let mut w = hound::WavWriter::create(
            dir.join(name),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..frames {
            w.write_sample(level).unwrap();
        }
        w.finalize().unwrap();
    }

    fn kit(
        name: &str,
        text: &str,
        sounds: &[(&str, usize, f32)],
        memory: usize,
    ) -> (PathBuf, Builtin) {
        let dir = scratch(name);
        for (n, f, l) in sounds {
            flat(&dir, n, *f, *l);
        }
        let path = dir.join("kit.sfz");
        std::fs::write(&path, text).unwrap();
        let lib = Arc::new(Library::read(&path, memory).unwrap());
        assert!(lib.problems.is_empty(), "{:?}", lib.problems);
        let mut b = Builtin::new(Kind::Sampler);
        b.set_library(&path.to_string_lossy(), lib);
        // Levels as the file has them.
        b.params().set_key("normalise", 0.0);
        (dir, b)
    }

    /// The left channel's level 1000 frames after playing `events`.
    fn level(i: &mut Box<dyn Instrument>, events: &[Event]) -> f32 {
        let (mut l, mut r) = (vec![0.0; 2000], vec![0.0; 2000]);
        i.process(events, &mut l, &mut r);
        l[1000]
    }

    fn on(key: u8, vel: u8) -> Event {
        Event::note_on(0, 0, key, vel, 0)
    }

    #[test]
    fn velocity_layers_and_round_robins_pick_their_sounds() {
        let text = "<global> loop_mode=one_shot amp_veltrack=0 seq_length=2\n\
                    <group> key=60 hivel=64\n<region> sample=s1.wav\n<region> sample=s2.wav seq_position=2\n\
                    <group> key=60 lovel=65\n<region> sample=h1.wav\n<region> sample=h2.wav seq_position=2\n";
        let sounds = [
            ("s1.wav", 48_000, 0.1),
            ("s2.wav", 48_000, 0.2),
            ("h1.wav", 48_000, 0.3),
            ("h2.wav", 48_000, 0.4),
        ];
        let (_d, b) = kit("layers", text, &sounds, usize::MAX);
        let mut i = b.make(48_000);
        // Each sound alone, then with the last still ringing.
        let mut heard = Vec::new();
        let mut before = 0.0;
        for vel in [30, 30, 100, 100, 30] {
            let now = level(&mut i, &[on(60, vel)]);
            heard.push(((now - before) * 10.0).round() / 10.0);
            before = now;
        }
        assert_eq!(heard, [0.1, 0.2, 0.3, 0.4, 0.1]);
    }

    #[test]
    fn a_choke_group_stops_the_open_hat_and_controllers_set_the_mics() {
        let text = "<control> label_cc70=Close mic set_cc70=127\n\
                    <global> loop_mode=one_shot amp_veltrack=0 off_mode=fast\n\
                    <region> sample=open.wav key=46 group=2 off_by=1\n\
                    <region> sample=closed.wav key=42 group=1\n\
                    <region> sample=kick.wav key=36 amplitude_cc70=100 locc70=1\n";
        let sounds = [
            ("open.wav", 96_000, 0.5),
            ("closed.wav", 96_000, 0.25),
            ("kick.wav", 96_000, 1.0),
        ];
        let (_d, b) = kit("choke", text, &sounds, usize::MAX);
        assert_eq!(
            b.params()
                .list()
                .iter()
                .find(|p| p.key == "cc70")
                .map(|p| p.name.as_str()),
            Some("Close mic")
        );
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(46, 100)]) - 0.5).abs() < 1e-3);
        // The closed hat cuts the open one off within a few milliseconds.
        assert!((level(&mut i, &[on(42, 100)]) - 0.25).abs() < 1e-3);
        level(
            &mut i,
            &[Event::note_off(0, 0, 42, 0), Event::note_off(0, 0, 46, 0)],
        );
        // The kick's mic follows its setting, and is gone at 0.
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(36, 100)]) - 1.0).abs() < 1e-3);
        b.params().set_key("cc70", 0.0);
        let mut i = b.make(48_000);
        assert_eq!(level(&mut i, &[on(36, 100)]), 0.0);
        b.params().set_key("cc70", 127.0);
        // A controller played moves it too.
        let mut i = b.make(48_000);
        let half = Event {
            frame: 0,
            channel: 0,
            kind: daw_engine::midi::Kind::Control {
                number: 70,
                value: 0,
            },
        };
        assert_eq!(level(&mut i, &[half, on(36, 100)]), 0.0);
    }

    #[test]
    fn key_switches_choose_and_release_regions_play_when_let_go() {
        let text = "<global> loop_mode=one_shot amp_veltrack=0\n\
                    <group> sw_lokey=24 sw_hikey=25 sw_default=24\n\
                    <region> sample=a.wav key=60 sw_last=24 sw_label=Plucked\n\
                    <region> sample=b.wav key=60 sw_last=25 sw_label=Bowed\n\
                    <group>\n<region> sample=rel.wav key=62 trigger=release\n";
        let sounds = [
            ("a.wav", 48_000, 0.1),
            ("b.wav", 48_000, 0.2),
            ("rel.wav", 48_000, 0.3),
        ];
        let (_d, b) = kit("switch", text, &sounds, usize::MAX);
        let ks = b.library().unwrap().sfz.key_switches();
        assert_eq!(ks, [(24, "Plucked".into()), (25, "Bowed".into())]);
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(60, 100)]) - 0.1).abs() < 1e-3);
        // Played on the keyboard.
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(25, 100), on(60, 100)]) - 0.2).abs() < 1e-3);
        // Or set.
        b.params().set_key("keyswitch", 25.0);
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(60, 100)]) - 0.2).abs() < 1e-3);
        // Nothing on the key until it is let go.
        let mut i = b.make(48_000);
        assert_eq!(level(&mut i, &[on(62, 100)]), 0.0);
        assert!((level(&mut i, &[Event::note_off(0, 0, 62, 0)]) - 0.3).abs() < 1e-3);
    }

    #[test]
    fn a_library_played_from_disk_sounds_as_it_does_from_memory() {
        let text = "<region> sample=long.wav loop_mode=one_shot pitch_keycenter=60\n";
        let dir = scratch("disk");
        crate::library::tests::wav(&dir, "long.wav", 100_000, 0.8);
        let path = dir.join("kit.sfz");
        std::fs::write(&path, text).unwrap();
        let render = |memory: usize| {
            let lib = Arc::new(Library::read(&path, memory).unwrap());
            assert_eq!(lib.streamed, memory == 0);
            let mut b = Builtin::new(Kind::Sampler);
            b.set_library(&path.to_string_lossy(), lib);
            b.params().set_key("normalise", 0.0);
            let mut i = b.make(48_000);
            let mut out = Vec::new();
            for block in 0..200 {
                let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
                let ev = if block == 0 {
                    vec![on(62, 127)]
                } else {
                    Vec::new()
                };
                i.process(&ev, &mut l, &mut r);
                out.extend(l);
                // As a sound card would, giving the disk time.
                std::thread::sleep(std::time::Duration::from_micros(300));
            }
            out
        };
        let (a, b) = (render(usize::MAX), render(0));
        // Two semitones up, it plays faster than the start in memory lasts.
        assert!(a.iter().any(|v| v.abs() > 0.5));
        let diff = a
            .iter()
            .zip(&b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(diff < 1e-6, "{diff}");
    }

    #[test]
    fn normalising_never_turns_a_loud_library_down() {
        // Big Rusty's loudest sounds peak near 0 dBFS: normalising
        // used to take them to -6 dBFS and the drums came out quiet.
        let text = "<region> sample=loud.wav key=60 loop_mode=one_shot amp_veltrack=0\n";
        let (_d, b) = kit(
            "normal-loud",
            text,
            &[("loud.wav", 48_000, 0.9)],
            usize::MAX,
        );
        b.params().set_key("normalise", 1.0);
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(60, 100)]) - 0.9).abs() < 1e-3);
    }

    #[test]
    fn normalising_brings_a_quiet_library_up() {
        let text = "<region> sample=quiet.wav key=60 loop_mode=one_shot amp_veltrack=0\n";
        let (_d, b) = kit("normal", text, &[("quiet.wav", 48_000, 0.01)], usize::MAX);
        b.params().set_key("normalise", 1.0);
        let mut i = b.make(48_000);
        assert!((level(&mut i, &[on(60, 100)]) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn big_rusty_drums_plays_when_it_is_here() {
        // The CC0 drum library the sampler was tested with: set
        // DAW_BIG_RUSTY to its folder to run this.
        let Some(dir) = std::env::var_os("DAW_BIG_RUSTY").map(PathBuf::from) else {
            return;
        };
        let path = dir.join("Programs/01-full.sfz");
        let t = std::time::Instant::now();
        let lib = crate::library::open(&path).unwrap();
        eprintln!(
            "read in {:.1} s: {} regions, {} sounds, {} MB in memory, from disk: {}, peak {:.3}, problems {:?}",
            t.elapsed().as_secs_f32(),
            lib.sfz.regions.len(),
            lib.sources.len(),
            lib.bytes >> 20,
            lib.streamed,
            lib.peak,
            lib.problems
        );
        let mut b = Builtin::new(Kind::Sampler);
        b.set_library(&path.to_string_lossy(), lib);
        let mut i = b.make(48_000);
        for (name, key) in [
            ("kick", 36u8),
            ("snare", 38),
            ("snare again", 38),
            ("closed hat", 42),
            ("crash", 49),
        ] {
            let mut peak = 0.0f32;
            let mut sum = 0.0f64;
            for block in 0..100 {
                let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
                let ev = if block == 0 {
                    vec![on(key, 110)]
                } else {
                    Vec::new()
                };
                i.process(&ev, &mut l, &mut r);
                for (a, c) in l.iter().zip(&r) {
                    peak = peak.max(a.abs()).max(c.abs());
                    sum += (*a as f64) * (*a as f64);
                }
                std::thread::sleep(std::time::Duration::from_micros(500));
            }
            eprintln!(
                "{name}: peak {:.1} dBFS, energy {sum:.3}",
                20.0 * peak.max(1e-9).log10()
            );
            assert!(peak > 0.05, "{name} is too quiet: {peak}");
            assert!(peak < 2.0, "{name} is too loud: {peak}");
        }
    }
}
