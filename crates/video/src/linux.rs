// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading video on Linux, through the system's media framework: a
//! pipeline from the file to decoded, scaled frames handed to us one at
//! a time. Which files open depends on the decoders installed.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

use crate::{Error, Frame, Info};

/// How long a step may take before the file is given up on.
const PATIENCE: gst::ClockTime = gst::ClockTime::from_seconds(10);

fn init() -> Result<(), Error> {
    gst::init().map_err(|e| Error::NoFramework(e.to_string()))
}

/// A pipeline decoding only the file's picture, into `sink`. Its sound
/// isn't exposed, so it is never decoded.
fn pipeline(path: &Path, caps: &str) -> Result<(gst::Pipeline, AppSink), Error> {
    init()?;
    let desc = format!(
        "filesrc name=src ! decodebin caps=video/x-raw expose-all-streams=false \
         ! videoconvert ! videoscale ! capsfilter caps=\"{caps}\" \
         ! appsink name=sink sync=false max-buffers=2 drop=false enable-last-sample=false"
    );
    let bad = |e: &dyn std::fmt::Display| Error::NoFramework(e.to_string());
    let p = gst::parse::launch(&desc)
        .map_err(|e| bad(&e))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| Error::NoFramework("not a pipeline".into()))?;
    let src = p
        .by_name("src")
        .ok_or(Error::NoFramework("no file source".into()))?;
    src.set_property("location", path.to_string_lossy().as_ref());
    let sink = (p.by_name("sink"))
        .and_then(|s| s.downcast::<AppSink>().ok())
        .ok_or(Error::NoFramework("no frame sink".into()))?;
    Ok((p, sink))
}

/// The pipeline's error, if it has one, as words.
fn failure(p: &gst::Pipeline) -> Option<String> {
    let msg = p.bus()?.pop_filtered(&[gst::MessageType::Error])?;
    match msg.view() {
        gst::MessageView::Error(e) => Some(e.error().to_string()),
        _ => None,
    }
}

/// Waits for `p` to reach the state it was set to.
fn settle(p: &gst::Pipeline) -> Result<(), Error> {
    let (r, _, _) = p.state(PATIENCE);
    match r {
        Ok(_) => Ok(()),
        Err(_) => Err(Error::Unreadable(
            failure(p).unwrap_or_else(|| "it didn't open".into()),
        )),
    }
}

pub(crate) fn probe(path: &Path) -> Result<Info, Error> {
    if !path.is_file() {
        return Err(Error::Unreadable(format!("{} isn't there", path.display())));
    }
    let (p, sink) = pipeline(path, "video/x-raw,format=RGBA")?;
    let _stop = Stop(p.clone());
    p.set_state(gst::State::Paused)
        .map_err(|_| Error::Unreadable(failure(&p).unwrap_or_else(|| "it didn't open".into())))?;
    settle(&p)?;
    let Some(sample) = sink.try_pull_preroll(PATIENCE) else {
        return Err(match failure(&p) {
            Some(e) => Error::Unreadable(e),
            None => Error::NoPicture,
        });
    };
    let caps = sample.caps().ok_or(Error::NoPicture)?;
    let s = caps.structure(0).ok_or(Error::NoPicture)?;
    let (w, h) = (
        s.get::<i32>("width").map_err(|_| Error::NoPicture)?,
        s.get::<i32>("height").map_err(|_| Error::NoPicture)?,
    );
    let par = s
        .get::<gst::Fraction>("pixel-aspect-ratio")
        .map_or((1, 1), |f| (f.numer().max(1), f.denom().max(1)));
    // A variable rate says 0/1; its fastest rate, or else 30, stands in.
    let rate = [
        s.get::<gst::Fraction>("framerate"),
        s.get::<gst::Fraction>("max-framerate"),
    ]
    .into_iter()
    .flatten()
    .map(|f| (f.numer(), f.denom()))
    .find(|&(n, d)| n > 0 && d > 0)
    .map_or((30, 1), |(n, d)| (n as u32, d as u32));
    let seconds = p
        .query_duration::<gst::ClockTime>()
        .map_or(0.0, |d| d.nseconds() as f64 / 1e9);
    Ok(Info {
        width: ((w as i64 * par.0 as i64) / par.1 as i64).max(1) as u32,
        height: h.max(1) as u32,
        rate: crate::tidy_rate(rate),
        seconds,
    })
}

/// Sets the pipeline to stop when dropped.
struct Stop(gst::Pipeline);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

pub(crate) struct Reader {
    pipeline: Stop,
    sink: AppSink,
}

impl Reader {
    pub(crate) fn open(path: &Path, _info: &Info, size: (u32, u32)) -> Result<Reader, Error> {
        let caps = format!(
            "video/x-raw,format=RGBA,width={},height={},pixel-aspect-ratio=1/1",
            size.0, size.1
        );
        let (p, sink) = pipeline(path, &caps)?;
        let pipeline = Stop(p);
        // Playing as fast as we take frames: the sink holds two and
        // waits for us.
        pipeline
            .0
            .set_state(gst::State::Playing)
            .map_err(|_| Error::Unreadable(failure(&pipeline.0).unwrap_or_default()))?;
        settle(&pipeline.0)?;
        Ok(Reader { pipeline, sink })
    }

    pub(crate) fn seek(&mut self, seconds: f64) -> Result<(), Error> {
        let at = gst::ClockTime::from_nseconds((seconds * 1e9).round() as u64);
        self.pipeline
            .0
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, at)
            .map_err(|e| Error::Unreadable(e.to_string()))?;
        settle(&self.pipeline.0)
    }

    pub(crate) fn next_frame(&mut self) -> Result<Option<Frame>, Error> {
        let Some(sample) = self.sink.try_pull_sample(PATIENCE) else {
            if self.sink.is_eos() {
                return Ok(None);
            }
            return Err(Error::Unreadable(
                failure(&self.pipeline.0).unwrap_or_else(|| "decoding stalled".into()),
            ));
        };
        let caps = sample.caps().ok_or(Error::NoPicture)?;
        let s = caps.structure(0).ok_or(Error::NoPicture)?;
        let w = s.get::<i32>("width").map_err(|_| Error::NoPicture)? as u32;
        let h = s.get::<i32>("height").map_err(|_| Error::NoPicture)? as u32;
        let buffer = sample.buffer().ok_or(Error::NoPicture)?;
        let pts = buffer.pts().unwrap_or(gst::ClockTime::ZERO);
        // Its time in the file, not since the last seek.
        let time = (sample.segment())
            .and_then(|seg| seg.downcast_ref::<gst::ClockTime>())
            .and_then(|seg| seg.to_stream_time(pts))
            .unwrap_or(pts);
        let map = buffer
            .map_readable()
            .map_err(|e| Error::Unreadable(e.to_string()))?;
        // Rows of four-byte pixels need no padding to line up.
        let len = w as usize * h as usize * 4;
        if map.len() < len {
            return Err(Error::Unreadable("a frame came out short".into()));
        }
        Ok(Some(Frame {
            width: w,
            height: h,
            rgba: map[..len].to_vec(),
            time: time.nseconds() as f64 / 1e9,
        }))
    }
}

/// The muxer for a copy named `out`, and the sound encoders it takes,
/// best first. `None` for an encoder means the sound goes in
/// uncompressed.
fn container(out: &Path) -> (&'static str, &'static [Option<&'static str>]) {
    let ext = (out.extension())
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // AAC if the system has an encoder for it, as players expect;
        // or else MP3, which nearly all of them play too.
        "mp4" | "m4v" => (
            "mp4mux",
            &[
                Some("avenc_aac"),
                Some("fdkaacenc"),
                Some("voaacenc"),
                Some("lamemp3enc"),
                Some("opusenc"),
            ],
        ),
        // Uncompressed, as video editors like.
        "mov" => ("qtmux", &[None]),
        "webm" => ("webmmux", &[Some("opusenc")]),
        "ogv" | "ogg" => ("oggmux", &[Some("opusenc"), Some("flacenc")]),
        "avi" => ("avimux", &[None]),
        _ => ("matroskamux", &[Some("flacenc"), None]),
    }
}

fn make(name: &str) -> Result<gst::Element, Error> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|_| Error::CantWrap(format!("the system's media framework has no {name}")))
}

/// Settings that suit a mix, for the encoders that need them.
fn tune(enc: &gst::Element) {
    let name = enc
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_default();
    match name.as_str() {
        "lamemp3enc" => {
            enc.set_property_from_str("target", "bitrate");
            enc.set_property("bitrate", 320i32);
            enc.set_property("cbr", true);
        }
        "opusenc" => enc.set_property("bitrate", 192_000i32),
        "avenc_aac" | "fdkaacenc" | "voaacenc" if enc.find_property("bitrate").is_some() => {
            enc.set_property_from_str("bitrate", "256000");
        }
        _ => {}
    }
}

pub(crate) fn wrap(
    video: &Path,
    sound: &Path,
    out: &Path,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    // The sound waits for the picture below, so a file without one is
    // turned away first. One this system can't decode can still be
    // copied, as its picture isn't decoded.
    init()?;
    if let Err(e @ Error::NoPicture) = probe(video) {
        return Err(e);
    }
    let failed = |e: &dyn std::fmt::Display| Error::CantWrap(e.to_string());
    let (mux_name, encoders) = container(out);
    let mux = make(mux_name)?;
    let p = gst::Pipeline::new();
    let film = make("filesrc")?;
    film.set_property("location", video.to_string_lossy().as_ref());
    let parse = make("parsebin")?;
    let sink = make("filesink")?;
    sink.set_property("location", out.to_string_lossy().as_ref());

    // The sound: the mix, converted and encoded for the container.
    let wav = make("filesrc")?;
    wav.set_property("location", sound.to_string_lossy().as_ref());
    let mut chain = vec![
        wav,
        make("wavparse")?,
        make("audioconvert")?,
        make("audioresample")?,
    ];
    let enc = encoders
        .iter()
        .find(|e| e.is_none_or(|n| gst::ElementFactory::find(n).is_some()))
        .ok_or_else(|| {
            Error::CantWrap(format!(
                "the system's media framework has no sound encoder for {mux_name}"
            ))
        })?;
    if let Some(name) = enc {
        let e = make(name)?;
        tune(&e);
        chain.push(e);
        // MP3 is framed for the container by its parser.
        if *name == "lamemp3enc" && gst::ElementFactory::find("mpegaudioparse").is_some() {
            chain.push(make("mpegaudioparse")?);
        }
    }
    chain.push(make("queue")?);
    p.add_many([&film, &parse, &mux, &sink])
        .map_err(|e| failed(&e))?;
    p.add_many(&chain).map_err(|e| failed(&e))?;
    film.link(&parse).map_err(|e| failed(&e))?;
    mux.link(&sink).map_err(|e| failed(&e))?;
    gst::Element::link_many(&chain).map_err(|e| failed(&e))?;
    // The sound's pad on the muxer: one taking what the chain ends in.
    let last = chain.last().expect("a chain");
    let before = &chain[chain.len() - 2];
    let audio_pad = (before.pad_template("src"))
        .and_then(|t| mux.compatible_pad_template(&t))
        .and_then(|t| mux.request_pad(&t, None, None))
        .ok_or_else(|| Error::CantWrap(format!("{mux_name} takes no sound of this kind")))?;
    let audio_out = last.static_pad("src").expect("a queue's output");
    audio_out.link(&audio_pad).map_err(|e| failed(&e))?;
    // The sound waits until the picture is joined to the muxer: some
    // kinds of file (Ogg) must start every stream before any data.
    let held = Arc::new(Mutex::new(
        audio_out.add_probe(gst::PadProbeType::BLOCK_DOWNSTREAM, |_, _| {
            gst::PadProbeReturn::Ok
        }),
    ));
    let release = {
        let (pad, held) = (audio_out.clone(), held.clone());
        move || {
            if let Some(id) = held.lock().unwrap().take() {
                pad.remove_probe(id);
            }
        }
    };

    // The picture: its first stream, as it is, into the muxer. Anything
    // else in the file (its own sound, subtitles) is left behind.
    let release2 = release.clone();
    let linked = Arc::new(Mutex::new(false));
    let problem: Arc<Mutex<Option<String>>> = Arc::default();
    {
        let (p2, mux2, linked, problem) =
            (p.downgrade(), mux.clone(), linked.clone(), problem.clone());
        parse.connect_pad_added(move |_, pad| {
            let Some(p) = p2.upgrade() else { return };
            let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
            let is_video = caps
                .structure(0)
                .is_some_and(|s| s.name().starts_with("video/"));
            let mut done = linked.lock().unwrap();
            let into = if is_video && !*done {
                *done = true;
                let q = gst::ElementFactory::make("queue").build().ok();
                q.and_then(|q| {
                    p.add(&q).ok()?;
                    q.sync_state_with_parent().ok()?;
                    let to = mux2.compatible_pad(&q.static_pad("src")?, Some(&caps));
                    let Some(to) = to else {
                        *problem.lock().unwrap() = Some(format!(
                            "{mux_name} can't hold this film's picture ({})",
                            caps
                        ));
                        return None;
                    };
                    q.static_pad("src")?.link(&to).ok()?;
                    q.static_pad("sink")
                })
            } else {
                let f = gst::ElementFactory::make("fakesink")
                    .property("sync", false)
                    .property("async", false)
                    .build()
                    .ok();
                f.and_then(|f| {
                    p.add(&f).ok()?;
                    f.sync_state_with_parent().ok()?;
                    f.static_pad("sink")
                })
            };
            let video = into.is_some() && is_video;
            if let Some(to) = into {
                let _ = pad.link(&to);
            }
            if video {
                drop(done);
                release();
            }
        });
    }
    // A file with no picture lets the sound go, to end.
    parse.connect_no_more_pads(move |_| release2());
    let stop = Stop(p.clone());
    p.set_state(gst::State::Playing)
        .map_err(|_| Error::CantWrap(failure(&p).unwrap_or_else(|| "it didn't start".into())))?;
    let bus = p.bus().ok_or(Error::CantWrap("no bus".into()))?;
    let began = std::time::Instant::now();
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err(Error::CantWrap("cancelled".into()));
        }
        if !*linked.lock().unwrap() && began.elapsed() > PATIENCE.into() {
            break Err(Error::CantWrap("the film's picture never came".into()));
        }
        if let Some(why) = problem.lock().unwrap().take() {
            break Err(Error::CantWrap(why));
        }
        let Some(msg) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(200),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) else {
            continue;
        };
        match msg.view() {
            gst::MessageView::Eos(_) => break Ok(()),
            gst::MessageView::Error(e) => {
                let why = problem.lock().unwrap().take();
                break Err(Error::CantWrap(
                    why.unwrap_or_else(|| e.error().to_string()),
                ));
            }
            _ => {}
        }
    };
    drop(stop);
    if result.is_ok() && !*linked.lock().unwrap() {
        return Err(Error::NoPicture);
    }
    result
}
