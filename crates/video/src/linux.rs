// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading video on Linux, through the system's media framework: a
//! pipeline from the file to decoded, scaled frames handed to us one at
//! a time. Which files open depends on the decoders installed.

use std::path::Path;

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
