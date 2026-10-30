// SPDX-License-Identifier: GPL-3.0-or-later
//! The messages between the app and a plugin helper process (roadmap
//! 2.4), and how they are written down the pipe.
//!
//! Each message is a little-endian `u32` length followed by that many
//! bytes: a tag byte, then the fields in order. Nothing here allocates
//! more than the message itself, and nothing in it is executed; a
//! message the reader does not understand is an error that ends the
//! conversation, which the app treats like a crash.

use std::io::{self, Read, Write};

use daw_engine::midi::{Event, Expression, Kind};

use crate::plugin::{Bus, Buses, Layout, Param, PluginState};

/// Largest message either side accepts, so a damaged length cannot
/// make it allocate without limit. Plugin state is the biggest thing
/// sent; some plugins keep samples in it.
pub const MAX_MESSAGE: u32 = 256 << 20;

/// What the app asks the helper to do. `id` names a plugin instance
/// the helper holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Loads class `cid` from `bundle`, ready for blocks of up to
    /// `max_block` frames at `rate`, with the buses `layout` asks for.
    Load {
        id: u32,
        bundle: String,
        cid: [u8; 16],
        rate: f64,
        max_block: u32,
        layout: Layout,
    },
    Unload {
        id: u32,
    },
    /// One block of stereo audio to run through the plugin, with its
    /// sidechain if it listens to one.
    Process {
        id: u32,
        left: Vec<f32>,
        right: Vec<f32>,
        key: Option<(Vec<f32>, Vec<f32>)>,
    },
    /// One block of an instrument: `frames` long, playing `events`
    /// (in frame order).
    Play {
        id: u32,
        frames: u32,
        events: Vec<Event>,
    },
    SetParam {
        id: u32,
        param: u32,
        value: f64,
    },
    GetParam {
        id: u32,
        param: u32,
    },
    ParamText {
        id: u32,
        param: u32,
        value: f64,
    },
    Params {
        id: u32,
    },
    SaveState {
        id: u32,
    },
    LoadState {
        id: u32,
        state: PluginState,
    },
    /// Anything the plugin has flagged since last asked.
    Poll {
        id: u32,
    },
    OpenEditor {
        id: u32,
        title: String,
    },
    CloseEditor {
        id: u32,
    },
    /// The plugin's own program lists.
    Programs {
        id: u32,
    },
    /// The id the plugin uses for parameter `param` of the older class
    /// `old` it replaces.
    RemapParam {
        id: u32,
        old: [u8; 16],
        param: u32,
    },
    Ping,
    Quit,
}

/// What the helper answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Ok,
    Error(String),
    Loaded {
        latency: u32,
        params: Vec<Param>,
        buses: Buses,
    },
    Audio {
        left: Vec<f32>,
        right: Vec<f32>,
    },
    /// An instrument's block: each output switched on, by bus index
    /// (the main one, 0, first), left then right.
    Played(Vec<(u32, Vec<f32>, Vec<f32>)>),
    Value(Option<f64>),
    Text(Option<String>),
    Params(Vec<Param>),
    State(PluginState),
    Status(Status),
    Programs(Vec<ProgramList>),
    ParamId(Option<u32>),
}

/// What [`Request::Poll`] reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub latency: u32,
    /// The plugin asked to be restarted (its latency or parameters changed).
    pub restart: bool,
    /// Its buses changed, or it asked to be loaded again: the app makes
    /// it again, with its settings.
    pub reload: bool,
    /// The user changed something in the plugin's editor.
    pub edited: bool,
    pub editor_open: bool,
}

/// One of a plugin's own program (preset) lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramList {
    pub id: i32,
    pub name: String,
    pub programs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Writing

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn samples(&mut self, s: &[f32]) {
        self.u32(s.len() as u32);
        for x in s {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
    }
    fn opt_f64(&mut self, v: Option<f64>) {
        match v {
            Some(v) => {
                self.bool(true);
                self.f64(v);
            }
            None => self.bool(false),
        }
    }
    fn opt_bytes(&mut self, b: Option<&[u8]>) {
        match b {
            Some(b) => {
                self.bool(true);
                self.bytes(b);
            }
            None => self.bool(false),
        }
    }
    fn state(&mut self, s: &PluginState) {
        self.bytes(&s.component);
        self.opt_bytes(s.controller.as_deref());
    }
    fn events(&mut self, events: &[Event]) {
        self.u32(events.len() as u32);
        for e in events {
            self.u32(e.frame);
            self.u8(e.channel);
            let (tag, a, b, id) = match e.kind {
                Kind::NoteOn {
                    pitch,
                    velocity,
                    id,
                } => (0, pitch, velocity, id),
                Kind::NoteOff {
                    pitch,
                    velocity,
                    id,
                } => (1, pitch, velocity, id),
                Kind::Control { number, value } => (2, number, value, 0),
                Kind::PitchBend(v) => {
                    let [a, b] = v.to_le_bytes();
                    (3, a, b, 0)
                }
                Kind::Pressure(v) => (4, v, 0, 0),
                Kind::KeyPressure { pitch, value } => (5, pitch, value, 0),
                Kind::Program(p) => (6, p, 0, 0),
                Kind::AllNotesOff => (7, 0, 0, 0),
                Kind::NoteExpression {
                    id,
                    pitch,
                    kind,
                    value: _,
                } => (8, pitch, expression_tag(kind), id),
            };
            let value = match e.kind {
                Kind::NoteExpression { value, .. } => value,
                _ => 0.0,
            };
            self.u8(tag);
            self.u8(a);
            self.u8(b);
            self.u32(id);
            self.u32(value.to_bits());
        }
    }
    fn opt_samples(&mut self, k: &Option<(Vec<f32>, Vec<f32>)>) {
        match k {
            Some((l, r)) => {
                self.bool(true);
                self.samples(l);
                self.samples(r);
            }
            None => self.bool(false),
        }
    }
    fn layout(&mut self, l: &Layout) {
        self.bool(l.sidechain);
        self.u32(l.outputs.len() as u32);
        for &o in &l.outputs {
            self.u32(o);
        }
    }
    fn buses(&mut self, b: &Buses) {
        for list in [&b.inputs, &b.outputs] {
            self.u32(list.len() as u32);
            for bus in list {
                self.str(&bus.name);
                self.u32(bus.channels);
                self.bool(bus.aux);
            }
        }
    }
    fn params(&mut self, ps: &[Param]) {
        self.u32(ps.len() as u32);
        for p in ps {
            self.u32(p.id);
            self.str(&p.title);
            self.str(&p.units);
            self.bool(p.automatable);
            self.bool(p.bypass);
            self.bool(p.program_change);
            self.i32(p.steps);
        }
    }
}

// ---------------------------------------------------------------------------
// Reading

struct R<'a>(&'a [u8]);

type Res<T> = Result<T, String>;

impl R<'_> {
    fn take(&mut self, n: usize) -> Res<&[u8]> {
        if self.0.len() < n {
            return Err("message cut short".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> Res<bool> {
        Ok(self.u8()? != 0)
    }
    fn u32(&mut self) -> Res<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Res<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Res<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Res<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Res<String> {
        String::from_utf8(self.bytes()?).map_err(|_| "text is not UTF-8".to_string())
    }
    fn samples(&mut self) -> Res<Vec<f32>> {
        let n = self.u32()? as usize;
        let b = self.take(n * 4)?;
        Ok(b.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect())
    }
    fn opt_f64(&mut self) -> Res<Option<f64>> {
        Ok(if self.bool()? {
            Some(self.f64()?)
        } else {
            None
        })
    }
    fn opt_bytes(&mut self) -> Res<Option<Vec<u8>>> {
        Ok(if self.bool()? {
            Some(self.bytes()?)
        } else {
            None
        })
    }
    fn state(&mut self) -> Res<PluginState> {
        Ok(PluginState {
            component: self.bytes()?,
            controller: self.opt_bytes()?,
        })
    }
    fn events(&mut self) -> Res<Vec<Event>> {
        let n = self.u32()? as usize;
        // Each event is 16 bytes: refuse a count the message can't hold.
        if n > self.0.len() / 16 {
            return Err("too many events".into());
        }
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            let frame = self.u32()?;
            let channel = self.u8()? & 15;
            let (tag, a, b, id) = (self.u8()?, self.u8()?, self.u8()?, self.u32()?);
            let value = f32::from_bits(self.u32()?);
            let kind = match tag {
                0 => Kind::NoteOn {
                    pitch: a,
                    velocity: b,
                    id,
                },
                1 => Kind::NoteOff {
                    pitch: a,
                    velocity: b,
                    id,
                },
                2 => Kind::Control {
                    number: a,
                    value: b,
                },
                3 => Kind::PitchBend(i16::from_le_bytes([a, b])),
                4 => Kind::Pressure(a),
                5 => Kind::KeyPressure { pitch: a, value: b },
                6 => Kind::Program(a),
                7 => Kind::AllNotesOff,
                8 => Kind::NoteExpression {
                    id,
                    pitch: a,
                    kind: match b {
                        0 => Expression::Tuning,
                        1 => Expression::Pressure,
                        2 => Expression::Timbre,
                        k => return Err(format!("unknown expression {k}")),
                    },
                    value,
                },
                t => return Err(format!("unknown event {t}")),
            };
            events.push(Event {
                frame,
                channel,
                kind,
            });
        }
        Ok(events)
    }
    fn opt_samples(&mut self) -> Res<Option<(Vec<f32>, Vec<f32>)>> {
        Ok(if self.bool()? {
            Some((self.samples()?, self.samples()?))
        } else {
            None
        })
    }
    fn layout(&mut self) -> Res<Layout> {
        let sidechain = self.bool()?;
        let n = self.u32()?;
        Ok(Layout {
            sidechain,
            outputs: (0..n).map(|_| self.u32()).collect::<Res<_>>()?,
        })
    }
    fn buses(&mut self) -> Res<Buses> {
        let mut list = || -> Res<Vec<Bus>> {
            let n = self.u32()?;
            (0..n)
                .map(|_| {
                    Ok(Bus {
                        name: self.str()?,
                        channels: self.u32()?,
                        aux: self.bool()?,
                    })
                })
                .collect()
        };
        let inputs = list()?;
        Ok(Buses {
            inputs,
            outputs: list()?,
        })
    }
    fn params(&mut self) -> Res<Vec<Param>> {
        let n = self.u32()?;
        (0..n)
            .map(|_| {
                Ok(Param {
                    id: self.u32()?,
                    title: self.str()?,
                    units: self.str()?,
                    automatable: self.bool()?,
                    bypass: self.bool()?,
                    program_change: self.bool()?,
                    steps: self.i32()?,
                })
            })
            .collect()
    }
    fn done(self) -> Res<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("message has trailing bytes".into())
        }
    }
}

// ---------------------------------------------------------------------------

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::new());
        match self {
            Request::Load {
                id,
                bundle,
                cid,
                rate,
                max_block,
                layout,
            } => {
                w.u8(1);
                w.u32(*id);
                w.str(bundle);
                w.0.extend_from_slice(cid);
                w.f64(*rate);
                w.u32(*max_block);
                w.layout(layout);
            }
            Request::Unload { id } => {
                w.u8(2);
                w.u32(*id);
            }
            Request::Process {
                id,
                left,
                right,
                key,
            } => {
                w.u8(3);
                w.u32(*id);
                w.samples(left);
                w.samples(right);
                w.opt_samples(key);
            }
            Request::SetParam { id, param, value } => {
                w.u8(4);
                w.u32(*id);
                w.u32(*param);
                w.f64(*value);
            }
            Request::GetParam { id, param } => {
                w.u8(5);
                w.u32(*id);
                w.u32(*param);
            }
            Request::ParamText { id, param, value } => {
                w.u8(6);
                w.u32(*id);
                w.u32(*param);
                w.f64(*value);
            }
            Request::Params { id } => {
                w.u8(7);
                w.u32(*id);
            }
            Request::SaveState { id } => {
                w.u8(8);
                w.u32(*id);
            }
            Request::LoadState { id, state } => {
                w.u8(9);
                w.u32(*id);
                w.state(state);
            }
            Request::Poll { id } => {
                w.u8(10);
                w.u32(*id);
            }
            Request::OpenEditor { id, title } => {
                w.u8(11);
                w.u32(*id);
                w.str(title);
            }
            Request::CloseEditor { id } => {
                w.u8(12);
                w.u32(*id);
            }
            Request::Programs { id } => {
                w.u8(13);
                w.u32(*id);
            }
            Request::Ping => w.u8(14),
            Request::Quit => w.u8(15),
            Request::RemapParam { id, old, param } => {
                w.u8(16);
                w.u32(*id);
                w.0.extend_from_slice(old);
                w.u32(*param);
            }
            Request::Play { id, frames, events } => {
                w.u8(17);
                w.u32(*id);
                w.u32(*frames);
                w.events(events);
            }
        }
        w.0
    }

    pub fn decode(bytes: &[u8]) -> Res<Request> {
        let mut r = R(bytes);
        let req = match r.u8()? {
            1 => {
                let id = r.u32()?;
                let bundle = r.str()?;
                let cid: [u8; 16] = r.take(16)?.try_into().unwrap();
                Request::Load {
                    id,
                    bundle,
                    cid,
                    rate: r.f64()?,
                    max_block: r.u32()?,
                    layout: r.layout()?,
                }
            }
            2 => Request::Unload { id: r.u32()? },
            3 => Request::Process {
                id: r.u32()?,
                left: r.samples()?,
                right: r.samples()?,
                key: r.opt_samples()?,
            },
            4 => Request::SetParam {
                id: r.u32()?,
                param: r.u32()?,
                value: r.f64()?,
            },
            5 => Request::GetParam {
                id: r.u32()?,
                param: r.u32()?,
            },
            6 => Request::ParamText {
                id: r.u32()?,
                param: r.u32()?,
                value: r.f64()?,
            },
            7 => Request::Params { id: r.u32()? },
            8 => Request::SaveState { id: r.u32()? },
            9 => Request::LoadState {
                id: r.u32()?,
                state: r.state()?,
            },
            10 => Request::Poll { id: r.u32()? },
            11 => Request::OpenEditor {
                id: r.u32()?,
                title: r.str()?,
            },
            12 => Request::CloseEditor { id: r.u32()? },
            13 => Request::Programs { id: r.u32()? },
            14 => Request::Ping,
            15 => Request::Quit,
            16 => Request::RemapParam {
                id: r.u32()?,
                old: r.take(16)?.try_into().unwrap(),
                param: r.u32()?,
            },
            17 => Request::Play {
                id: r.u32()?,
                frames: r.u32()?,
                events: r.events()?,
            },
            t => return Err(format!("unknown request {t}")),
        };
        r.done()?;
        Ok(req)
    }
}

impl Reply {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::new());
        match self {
            Reply::Ok => w.u8(0),
            Reply::Error(e) => {
                w.u8(1);
                w.str(e);
            }
            Reply::Loaded {
                latency,
                params,
                buses,
            } => {
                w.u8(2);
                w.u32(*latency);
                w.params(params);
                w.buses(buses);
            }
            Reply::Audio { left, right } => {
                w.u8(3);
                w.samples(left);
                w.samples(right);
            }
            Reply::Value(v) => {
                w.u8(4);
                w.opt_f64(*v);
            }
            Reply::Text(t) => {
                w.u8(5);
                match t {
                    Some(t) => {
                        w.bool(true);
                        w.str(t);
                    }
                    None => w.bool(false),
                }
            }
            Reply::Params(ps) => {
                w.u8(6);
                w.params(ps);
            }
            Reply::State(s) => {
                w.u8(7);
                w.state(s);
            }
            Reply::Status(s) => {
                w.u8(8);
                w.u32(s.latency);
                w.bool(s.restart);
                w.bool(s.reload);
                w.bool(s.edited);
                w.bool(s.editor_open);
            }
            Reply::Programs(lists) => {
                w.u8(9);
                w.u32(lists.len() as u32);
                for l in lists {
                    w.i32(l.id);
                    w.str(&l.name);
                    w.u32(l.programs.len() as u32);
                    for p in &l.programs {
                        w.str(p);
                    }
                }
            }
            Reply::ParamId(p) => {
                w.u8(10);
                w.bool(p.is_some());
                w.u32(p.unwrap_or(0));
            }
            Reply::Played(buses) => {
                w.u8(11);
                w.u32(buses.len() as u32);
                for (i, l, r) in buses {
                    w.u32(*i);
                    w.samples(l);
                    w.samples(r);
                }
            }
        }
        w.0
    }

    pub fn decode(bytes: &[u8]) -> Res<Reply> {
        let mut r = R(bytes);
        let reply = match r.u8()? {
            0 => Reply::Ok,
            1 => Reply::Error(r.str()?),
            2 => Reply::Loaded {
                latency: r.u32()?,
                params: r.params()?,
                buses: r.buses()?,
            },
            3 => Reply::Audio {
                left: r.samples()?,
                right: r.samples()?,
            },
            4 => Reply::Value(r.opt_f64()?),
            5 => Reply::Text(if r.bool()? { Some(r.str()?) } else { None }),
            6 => Reply::Params(r.params()?),
            7 => Reply::State(r.state()?),
            8 => Reply::Status(Status {
                latency: r.u32()?,
                restart: r.bool()?,
                reload: r.bool()?,
                edited: r.bool()?,
                editor_open: r.bool()?,
            }),
            9 => {
                let n = r.u32()?;
                let mut lists = Vec::new();
                for _ in 0..n {
                    let id = r.i32()?;
                    let name = r.str()?;
                    let count = r.u32()?;
                    let programs = (0..count).map(|_| r.str()).collect::<Res<_>>()?;
                    lists.push(ProgramList { id, name, programs });
                }
                Reply::Programs(lists)
            }
            10 => {
                let some = r.bool()?;
                let id = r.u32()?;
                Reply::ParamId(some.then_some(id))
            }
            11 => {
                let n = r.u32()?;
                if n as usize > r.0.len() / 12 {
                    return Err("too many outputs".into());
                }
                let buses = (0..n)
                    .map(|_| Ok((r.u32()?, r.samples()?, r.samples()?)))
                    .collect::<Res<_>>()?;
                Reply::Played(buses)
            }
            t => return Err(format!("unknown reply {t}")),
        };
        r.done()?;
        Ok(reply)
    }
}

/// Writes one length-prefixed message.
fn expression_tag(kind: Expression) -> u8 {
    match kind {
        Expression::Tuning => 0,
        Expression::Pressure => 1,
        Expression::Timbre => 2,
    }
}

pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// Reads one length-prefixed message. `Ok(None)` at a clean end of
/// stream.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::other("message too long"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_come_back_as_sent() {
        let param = Param {
            id: 7,
            title: "Drive".into(),
            units: "dB".into(),
            automatable: true,
            bypass: false,
            program_change: false,
            steps: 0,
        };
        let state = PluginState {
            component: vec![1, 2, 3],
            controller: Some(vec![4]),
        };
        let requests = vec![
            Request::Load {
                id: 1,
                bundle: "/x/y.vst3".into(),
                cid: [9; 16],
                rate: 48_000.0,
                max_block: 512,
                layout: Layout {
                    sidechain: true,
                    outputs: vec![1, 3],
                },
            },
            Request::Unload { id: 1 },
            Request::Play {
                id: 1,
                frames: 64,
                events: vec![
                    Event::note_on(3, 9, 36, 127, 4),
                    Event::note_off(9, 9, 36, 4),
                    Event {
                        frame: 10,
                        channel: 1,
                        kind: Kind::PitchBend(-8000),
                    },
                    Event {
                        frame: 11,
                        channel: 1,
                        kind: Kind::Control {
                            number: 64,
                            value: 127,
                        },
                    },
                    Event {
                        frame: 12,
                        channel: 2,
                        kind: Kind::NoteExpression {
                            id: 4,
                            pitch: 36,
                            kind: Expression::Tuning,
                            value: -1.5,
                        },
                    },
                    Event::all_notes_off(63),
                ],
            },
            Request::Process {
                id: 1,
                left: vec![0.5, -0.5],
                right: vec![1.0, 0.0],
                key: None,
            },
            Request::Process {
                id: 1,
                left: vec![0.5],
                right: vec![1.0],
                key: Some((vec![0.25], vec![-0.25])),
            },
            Request::SetParam {
                id: 1,
                param: 7,
                value: 0.25,
            },
            Request::GetParam { id: 1, param: 7 },
            Request::ParamText {
                id: 1,
                param: 7,
                value: 0.5,
            },
            Request::Params { id: 1 },
            Request::SaveState { id: 1 },
            Request::LoadState {
                id: 1,
                state: state.clone(),
            },
            Request::Poll { id: 1 },
            Request::OpenEditor {
                id: 1,
                title: "Amp".into(),
            },
            Request::CloseEditor { id: 1 },
            Request::Programs { id: 1 },
            Request::RemapParam {
                id: 1,
                old: [3; 16],
                param: 9,
            },
            Request::Ping,
            Request::Quit,
        ];
        for r in requests {
            assert_eq!(Request::decode(&r.encode()).unwrap(), r);
        }
        let replies = vec![
            Reply::Ok,
            Reply::Error("no".into()),
            Reply::Loaded {
                latency: 64,
                params: vec![param.clone()],
                buses: Buses {
                    inputs: vec![Bus {
                        name: "Side".into(),
                        channels: 1,
                        aux: true,
                    }],
                    outputs: vec![],
                },
            },
            Reply::Audio {
                left: vec![0.1],
                right: vec![0.2],
            },
            Reply::Played(vec![(0, vec![0.1], vec![0.2]), (2, vec![0.3], vec![-0.3])]),
            Reply::Value(Some(0.5)),
            Reply::Value(None),
            Reply::Text(Some("3 dB".into())),
            Reply::Text(None),
            Reply::Params(vec![param]),
            Reply::State(state),
            Reply::Status(Status {
                latency: 1,
                restart: true,
                reload: false,
                edited: false,
                editor_open: true,
            }),
            Reply::ParamId(Some(4)),
            Reply::ParamId(None),
            Reply::Programs(vec![ProgramList {
                id: 0,
                name: "Factory".into(),
                programs: vec!["Clean".into(), "Crunch".into()],
            }]),
        ];
        for r in replies {
            assert_eq!(Reply::decode(&r.encode()).unwrap(), r);
        }
    }

    #[test]
    fn damaged_messages_are_errors_not_panics() {
        assert!(Request::decode(&[]).is_err());
        assert!(Request::decode(&[99]).is_err());
        assert!(Request::decode(&[3, 1, 0, 0, 0, 200, 0, 0, 0]).is_err());
        let mut ok = Request::Ping.encode();
        ok.push(0);
        assert!(Request::decode(&ok).is_err());
        assert!(Reply::decode(&[2, 0, 0]).is_err());
    }

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"abc").unwrap();
        write_frame(&mut buf, b"").unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read_frame(&mut r).unwrap().as_deref(), Some(&b"abc"[..]));
        assert_eq!(read_frame(&mut r).unwrap().as_deref(), Some(&b""[..]));
        assert_eq!(read_frame(&mut r).unwrap(), None);
        let huge = (MAX_MESSAGE + 1).to_le_bytes();
        assert!(read_frame(&mut huge.as_slice()).is_err());
    }
}
