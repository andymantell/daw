// SPDX-License-Identifier: GPL-3.0-or-later
//! What an assistant can hear and see (roadmap 2.11): the meters,
//! analysis of a track, a bus or the mix over a time range (spectrum,
//! loudness, peak and RMS, and the mix with one track left out), the
//! audio of a region rendered to a file, and a picture of a pane.
//!
//! Analysis and audio are rendered offline from fresh copies of the
//! effects, as an export is, on a thread of their own, so the app
//! carries on meanwhile; the thread answers the call itself. A picture
//! is taken of the next frame drawn.

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use daw_engine::analysis::{analyse, band_centres};
use daw_engine::engine::render_offline;
use daw_engine::export::LANE_BLOCK;
use daw_engine::graph::{Graph, Session};
use daw_engine::time::Frames;
use eframe::egui;
use serde_json::{Value, json};

use crate::App;
use crate::workspace::{PANES, Pane};

type Reply = Sender<Result<Value, String>>;

/// The longest stretch rendered for one call.
const MAX_SECONDS: f64 = 600.0;
/// Audio this short is sent in the answer as well as written.
const SEND_SECONDS: f64 = 10.0;
/// A picture not taken by then is given up on (a window not drawn).
const SHOT_TIMEOUT: Duration = Duration::from_secs(5);

/// A render running for a call: its thread answers, and the plugin
/// copies it plays must live until it ends.
pub(crate) struct Job {
    thread: JoinHandle<()>,
    _plugins: Vec<daw_plugins::Plugin>,
}

/// A picture asked for.
pub(crate) struct Shot {
    reply: Reply,
    /// The part of the window wanted, in points, or all of it.
    rect: Option<egui::Rect>,
    name: String,
    image: Arc<std::sync::Mutex<Option<Arc<egui::ColorImage>>>>,
    asked: Instant,
}

/// What an assistant's audio tools need between frames.
#[derive(Default)]
pub(crate) struct Listening {
    jobs: Vec<Job>,
    shots: Vec<Shot>,
    /// Where each pane was drawn this frame, and in which window.
    pub(crate) drawn: Vec<(Pane, egui::ViewportId, egui::Rect)>,
    last_drawn: Vec<(Pane, egui::ViewportId, egui::Rect)>,
}

/// The tools here that answer later.
pub(crate) const LATER: &[&str] = &["analyse", "render_audio", "get_picture"];

impl App {
    /// Starts a tool that answers later. False if `name` isn't one.
    pub(crate) fn listen_tool(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        args: &Value,
        reply: Reply,
    ) {
        let r = match name {
            "analyse" | "render_audio" => {
                self.start_render_job(name == "analyse", args, reply.clone())
            }
            "get_picture" => self.ask_picture(ctx, args, reply.clone()),
            _ => Err(format!("There is no tool {name}.")),
        };
        if let Err(e) = r {
            let _ = reply.send(Err(e));
        }
    }

    /// Once a frame: drops finished renders, sends pictures taken.
    pub(crate) fn listen_tick(&mut self, ctx: &egui::Context) {
        let l = &mut self.listening;
        l.last_drawn = std::mem::take(&mut l.drawn);
        l.jobs.retain(|j| !j.thread.is_finished());
        let mut waiting = Vec::new();
        for s in std::mem::take(&mut l.shots) {
            let image = s.image.lock().unwrap().take();
            match image {
                Some(img) => {
                    let _ =
                        s.reply
                            .send(picture_value(&img, s.rect, ctx.pixels_per_point(), &s.name));
                }
                None if s.asked.elapsed() > SHOT_TIMEOUT => {
                    let _ = s.reply.send(Err(
                        "The picture wasn't taken: the window may be hidden or minimised.".into(),
                    ));
                }
                None => waiting.push(s),
            }
        }
        l.shots = waiting;
        if !l.shots.is_empty() || !l.jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    pub(crate) fn meters_value(&self) -> Value {
        let st = self.ctl.status();
        let db = |x: f32| (20.0 * x.max(1e-10).log10()).max(-120.0);
        let tracks: Vec<Value> = (self.tracks.iter().enumerate())
            .map(|(i, t)| {
                let (rl, rr) = st.track_rms(i);
                json!({
                    "track": t.key,
                    "name": t.name,
                    "peak_db": [db(t.meter.0), db(t.meter.1)],
                    "rms_db": [db(rl), db(rr)],
                })
            })
            .collect();
        let (rl, rr) = st.master_rms();
        json!({
            "playing": st.playing(),
            "master": {
                "peak_db": [db(self.master_meter.0), db(self.master_meter.1)],
                "rms_db": [db(rl), db(rr)],
            },
            "tracks": tracks,
        })
    }

    fn start_render_job(
        &mut self,
        analysing: bool,
        args: &Value,
        reply: Reply,
    ) -> Result<(), String> {
        let (graph, plugins, from, frames) = self.region(args)?;
        let rate = self.rate;
        let what = describe(self, args);
        let thread = std::thread::Builder::new()
            .name("assistant-render".into())
            .spawn(move || {
                let [l, r] = render_offline(graph, from, frames, LANE_BLOCK);
                let from_s = from.0 as f64 / rate as f64;
                let answer = if analysing {
                    Ok(report_value(&l, &r, rate, from_s, &what))
                } else {
                    write_audio(&l, &r, rate, from_s, &what)
                };
                let _ = reply.send(answer);
            })
            .map_err(|e| e.to_string())?;
        self.listening.jobs.push(Job {
            thread,
            _plugins: plugins,
        });
        Ok(())
    }

    /// The graph a call asks to hear, and the frames: a track or bus
    /// on its own after its fader, or the mix, maybe without a track.
    fn region(
        &mut self,
        args: &Value,
    ) -> Result<(Graph, Vec<daw_plugins::Plugin>, Frames, usize), String> {
        let index = |key: Option<&str>| -> Result<Option<usize>, String> {
            match key {
                None => Ok(None),
                Some(k) => (self.track_index(k))
                    .map(Some)
                    .ok_or_else(|| format!("There is no track {k}.")),
            }
        };
        let only = index(args["track"].as_str())?;
        let without = index(args["without"].as_str())?;
        let mut s = self.session();
        // The mix as it stands: no practice mute, no live input.
        for (spec, t) in s.tracks.iter_mut().zip(&self.tracks) {
            spec.mix = t.mix;
            spec.input = None;
            spec.arm = false;
            spec.monitor = false;
        }
        if let Some(j) = without {
            s.tracks[j].mix.mute = true;
        }
        let all: Vec<usize> = (0..self.tracks.len()).collect();
        let (s, tracks) = match only {
            Some(i) if self.tracks[i].kind.is_bus() => (crate::mixer::through_bus(&s, i), all),
            Some(i) => {
                let mut one = Session::new(self.tempo_map());
                let mut spec = s.tracks[i].clone();
                spec.mix.mute = false;
                spec.mix.solo = false;
                crate::mixer::alone(&mut spec);
                one.tracks.push(spec);
                (one, vec![i])
            }
            None => (s, all),
        };
        let mut plugins = Vec::new();
        let graph = self.render_graph(&s, &tracks, &mut plugins)?;
        let rate = self.rate as f64;
        let map = self.tempo_map();
        let (from, to) = match (args["from_seconds"].as_f64(), args["to_seconds"].as_f64()) {
            (Some(a), Some(b)) => ((a * rate) as i64, (b * rate) as i64),
            (Some(a), None) => ((a * rate) as i64, graph.end().0),
            (None, Some(b)) => (0, (b * rate) as i64),
            (None, None) if self.looping => (
                map.frames_at(map.bar_start(self.loop_bars.0)).0,
                map.frames_at(map.bar_start(self.loop_bars.1)).0,
            ),
            (None, None) => (0, graph.end().0),
        };
        let from = from.max(0);
        if to <= from {
            return Err("There's nothing there: give a later end, or add some audio first.".into());
        }
        let frames = ((to - from) as f64).min(MAX_SECONDS * rate) as usize;
        Ok((graph, plugins, Frames(from), frames))
    }

    /// `s` made into a graph for rendering offline, with fresh copies
    /// of the effects and instruments of `tracks`, by index. Their
    /// plugins go in `plugins`, to be kept until the render ends. As an
    /// export makes its graphs.
    fn render_graph(
        &mut self,
        s: &Session,
        tracks: &[usize],
        plugins: &mut Vec<daw_plugins::Plugin>,
    ) -> Result<Graph, String> {
        let rate = self.rate;
        let mut s = s.clone();
        let mut delays = std::collections::HashMap::new();
        let mut fx = Vec::new();
        for slot in tracks.iter().flat_map(|&i| &self.tracks[i].fx) {
            let (p, proc) = self.effects.fresh(slot, rate)?;
            if let Some(p) = &p {
                delays.insert(slot.id, p.latency());
            }
            plugins.extend(p);
            fx.push((slot.id, proc));
        }
        let mut instruments: Vec<(u64, Box<dyn daw_engine::midi::Instrument>)> = Vec::new();
        let mut inst_delays = std::collections::HashMap::new();
        for i in tracks.iter().flat_map(|&i| self.tracks[i].instruments()) {
            if let Some((p, inst)) = i.fresh(&mut self.effects, rate)? {
                if let Some(p) = &p {
                    inst_delays.insert(i.id, p.latency());
                }
                plugins.extend(p);
                instruments.push((i.id, inst));
            }
        }
        // The copies add no helper process's block of delay: what they
        // report is what is made up for.
        for t in &mut s.tracks {
            for f in &mut t.inserts {
                if let Some(&d) = delays.get(&f.id) {
                    f.latency = d;
                }
            }
            let layers = t.layers.iter_mut().filter_map(|l| l.instrument.as_mut());
            for i in t.instrument.iter_mut().chain(layers) {
                if let Some(&d) = inst_delays.get(&i.id) {
                    i.latency = d;
                }
            }
        }
        s.compile_all(daw_audio_io::MAX_BLOCK, &mut fx, &mut instruments)
            .map_err(|e| e.to_string())
    }

    fn ask_picture(
        &mut self,
        ctx: &egui::Context,
        args: &Value,
        reply: Reply,
    ) -> Result<(), String> {
        let (viewport, rect, name) = match args["pane"].as_str() {
            None => (egui::ViewportId::ROOT, None, "the window".to_string()),
            Some(id) => {
                let pane = (PANES.iter())
                    .find(|p| p.id() == id)
                    .ok_or_else(|| format!("There is no pane {id}."))?;
                let (_, v, r) = (self.listening.last_drawn.iter())
                    .find(|(p, ..)| p == pane)
                    .ok_or_else(|| format!("The {} isn't showing: open it first.", pane.title()))?;
                (*v, Some(*r), pane.title().to_string())
            }
        };
        let image = Arc::new(std::sync::Mutex::new(None));
        let to = image.clone();
        let c = ctx.clone();
        ctx.send_viewport_cmd_to(
            viewport,
            egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                *to.lock().unwrap() = Some(img);
                c.request_repaint();
            })),
        );
        ctx.request_repaint();
        self.listening.shots.push(Shot {
            reply,
            rect: if viewport == egui::ViewportId::ROOT {
                rect
            } else {
                None
            },
            name,
            image,
            asked: Instant::now(),
        });
        Ok(())
    }
}

/// What a call asked to hear, in words.
fn describe(app: &App, args: &Value) -> String {
    let name = |k: &str| {
        app.track_index(k)
            .map_or(k.to_string(), |i| app.tracks[i].name.clone())
    };
    let mut s = match args["track"].as_str() {
        Some(k) => name(k),
        None => "The mix".into(),
    };
    if let Some(k) = args["without"].as_str() {
        s.push_str(&format!(" without {}", name(k)));
    }
    s
}

fn report_value(l: &[f32], r: &[f32], rate: u32, from: f64, what: &str) -> Value {
    let rep = analyse(l, r, rate);
    let finite = |x: f64| x.is_finite().then_some(x);
    let bands: Vec<Value> = (band_centres().iter().zip(&rep.bands_db))
        .map(|(c, d)| json!({"hz": c.round(), "db": (d * 10.0).round() / 10.0}))
        .collect();
    json!({
        "what": what,
        "from_seconds": from,
        "seconds": rep.seconds,
        "sample_peak_dbfs": rep.sample_peak_db,
        "true_peak_dbtp": rep.true_peak_db,
        "rms_dbfs": rep.rms_db,
        "integrated_lufs": finite(rep.integrated_lufs),
        "short_term_max_lufs": finite(rep.short_term_max_lufs),
        "loudness_range_lu": finite(rep.loudness_range_lu),
        "correlation": rep.correlation,
        "third_octave_bands": bands,
        "bands_note": "Average level per third-octave band, in dB where a full-scale sine reads about 0.",
    })
}

/// Writes the audio to the app's cache folder as a 16-bit WAV: where,
/// and the audio itself too when short.
fn write_audio(l: &[f32], r: &[f32], rate: u32, from: f64, what: &str) -> Result<Value, String> {
    let dir = crate::cache_dir().join("assistant");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.wav", daw_project::new_id()));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut bytes, spec).map_err(|e| e.to_string())?;
        for (a, b) in l.iter().zip(r) {
            for x in [a, b] {
                let v = (x.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
                w.write_sample(v).map_err(|e| e.to_string())?;
            }
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    let bytes = bytes.into_inner();
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let seconds = l.len() as f64 / rate as f64;
    let mut v = json!({
        "what": what,
        "file": path.display().to_string(),
        "from_seconds": from,
        "seconds": seconds,
        "rate": rate,
    });
    if seconds <= SEND_SECONDS {
        v["$content"] = json!([{"type": "audio", "data": base64(&bytes), "mimeType": "audio/wav"}]);
    }
    Ok(v)
}

/// The picture as PNG, cut to `rect` (points) when given.
fn picture_value(
    img: &egui::ColorImage,
    rect: Option<egui::Rect>,
    ppp: f32,
    name: &str,
) -> Result<Value, String> {
    let [w, h] = img.size;
    let (x0, y0, x1, y1) = match rect {
        Some(r) => {
            let px = |v: f32, max: usize| ((v * ppp).round().max(0.0) as usize).min(max);
            (
                px(r.left(), w),
                px(r.top(), h),
                px(r.right(), w),
                px(r.bottom(), h),
            )
        }
        None => (0, 0, w, h),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err("There was nothing to take a picture of.".into());
    }
    let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        for p in &img.pixels[y * w + x0..y * w + x1] {
            rgba.extend_from_slice(&p.to_array());
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut png_bytes, (x1 - x0) as u32, (y1 - y0) as u32);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    Ok(json!({
        "picture_of": name,
        "width": x1 - x0,
        "height": y1 - y0,
        "$content": [{"type": "image", "data": base64(&png_bytes), "mimeType": "image/png"}],
    }))
}

/// Standard base64, with padding.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::driver::Driver;

    #[test]
    fn base64_is_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// A synth track with a note held for the first two seconds.
    fn synth(d: &mut Driver) -> String {
        d.command("add-synth-track");
        let key = d.project()["tracks"][0]["id"].as_str().unwrap().to_string();
        let a = d.app();
        let t = &mut a.tracks[0];
        let mut c = daw_project::NoteClip::new("n".into(), "Held".into(), 0, 4 * 960);
        c.notes.push(daw_project::Note::new(0, 4 * 960, 45, 100));
        t.note_clips.push(c);
        a.rebuild();
        key
    }

    #[test]
    fn analysis_hears_a_track_and_the_mix_without_it() {
        let mut d = Driver::new();
        let key = synth(&mut d);
        let r = d
            .call(
                "analyse",
                json!({"track": key, "from_seconds": 0.0, "to_seconds": 1.5}),
            )
            .unwrap();
        assert!((r["seconds"].as_f64().unwrap() - 1.5).abs() < 1e-3, "{r}");
        assert!(r["rms_dbfs"].as_f64().unwrap() > -60.0, "{r}");
        assert_eq!(r["third_octave_bands"].as_array().unwrap().len(), 31);
        let none = d
            .call(
                "analyse",
                json!({"without": key, "from_seconds": 0.0, "to_seconds": 1.5}),
            )
            .unwrap();
        assert!(none["rms_dbfs"].as_f64().unwrap() < -100.0, "{none}");
        assert_eq!(none["integrated_lufs"], Value::Null);
        assert!(d.call("analyse", json!({"track": "nope"})).is_err());
    }

    #[test]
    fn rendered_audio_is_written_and_sent_when_short() {
        let mut d = Driver::new();
        synth(&mut d);
        let r = d
            .call(
                "render_audio",
                json!({"from_seconds": 0.0, "to_seconds": 1.0}),
            )
            .unwrap();
        let path = r["file"].as_str().unwrap();
        let wav = hound::WavReader::open(path).unwrap();
        assert_eq!(wav.spec().channels, 2);
        assert_eq!(wav.duration(), d.app().rate);
        assert_eq!(r["$content"][0]["type"], "audio");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn meters_list_every_track() {
        let mut d = Driver::new();
        synth(&mut d);
        let m = d.call("get_meters", json!({})).unwrap();
        assert_eq!(m["tracks"].as_array().unwrap().len(), 1);
        assert!(m["master"]["rms_db"][0].is_number());
    }

    #[test]
    fn a_picture_of_a_pane_not_showing_is_refused() {
        let mut d = Driver::new();
        let e = d.call("get_picture", json!({"pane": "video"})).unwrap_err();
        assert!(e.contains("isn't showing"), "{e}");
        assert!(d.call("get_picture", json!({"pane": "nothing"})).is_err());
    }

    #[test]
    fn a_picture_is_cut_to_the_pane() {
        let img = egui::ColorImage::filled([20, 10], egui::Color32::RED);
        let r = egui::Rect::from_min_max(egui::pos2(2.0, 1.0), egui::pos2(6.0, 4.0));
        let v = picture_value(&img, Some(r), 2.0, "Mixer").unwrap();
        assert_eq!(
            (v["width"].clone(), v["height"].clone()),
            (json!(8), json!(6))
        );
        assert_eq!(v["$content"][0]["mimeType"], "image/png");
    }
}
