// SPDX-License-Identifier: GPL-3.0-or-later
//! Frozen audio for missing plugins (roadmap 1.3).
//!
//! Each time a project is saved, every track with an effect switched on
//! is rendered through its effects, on a thread of its own with its own
//! copies of the plugins, the same way a stem is exported. The file goes
//! into the project's audio like any other, so bundles and shared folders
//! carry it, and the track in the document says which file it is and what
//! it was made from. Only a track that changed since its last render (its
//! clips, or its effects and their settings) is rendered again.
//!
//! A computer missing one of a track's plugins plays the frozen audio in
//! place of the track's clips and effects, as long as it still fits the
//! clips; the effects themselves, with their settings, stay in the
//! document as they were.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use daw_engine::export::{Written, write_wav};
use daw_engine::graph::{ClipSpec, Mix, Session};
use daw_engine::stream::StreamSource;
use daw_engine::time::Frames;
use daw_plugins::Plugin;
use daw_project::Frozen;
use eframe::egui;

use crate::{App, Track, cache_dir};

struct Job {
    /// The track's id.
    track: String,
    /// What the render will be, all but its asset.
    frozen: Frozen,
    path: PathBuf,
    thread: Option<JoinHandle<Result<Written, String>>>,
    cancel: Arc<AtomicBool>,
    /// The plugins' UI sides, kept until rendering ends.
    _plugins: Vec<Plugin>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Freezer {
    jobs: Vec<Job>,
    /// Where to write a bundle once every render is done.
    pub then_bundle: Option<PathBuf>,
}

impl Freezer {
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub fn making(&self, track: &str) -> bool {
        self.jobs.iter().any(|j| j.track == track)
    }

    fn stop(&mut self, track: &str) {
        // Dropping a job tells its thread to stop.
        self.jobs.retain(|j| j.track != track);
    }
}

impl App {
    /// Starts rendering the frozen audio of each track that needs it
    /// again. Called once the project is saved, when plugin settings
    /// have just been read into the document.
    pub(crate) fn freeze_tracks(&mut self) {
        if self.proj.folder.is_none() {
            return;
        }
        let doc = self.proj.doc.get();
        let mut dropped = Vec::new();
        for i in 0..self.tracks.len() {
            let key = self.tracks[i].key.clone();
            // Without all its plugins here the track can't be rendered;
            // whatever was shared for it stays.
            if self.proj.missing_fx.contains_key(&key) {
                continue;
            }
            let Some(pt) = doc.tracks.iter().find(|t| t.id == key) else {
                continue;
            };
            let mut want = pt.freeze_shape(self.rate);
            if !pt.wants_frozen() || want.clips.is_empty() {
                self.freezer.stop(&key);
                if let Some(old) = self.tracks[i].frozen.take() {
                    dropped.push(old.asset);
                }
                continue;
            }
            want.print = pt.freeze_print(self.rate);
            if self.tracks[i]
                .frozen
                .as_ref()
                .is_some_and(|f| f.print == want.print)
            {
                self.freezer.stop(&key);
                continue;
            }
            if (self.freezer.jobs.iter()).any(|j| j.track == key && j.frozen.print == want.print) {
                continue;
            }
            self.freezer.stop(&key);
            // Audio still being converted to the device's rate would be
            // left out: try again at the next save.
            if self.tracks[i]
                .clips
                .iter()
                .any(|c| c.source.rate != self.rate)
            {
                continue;
            }
            if let Err(e) = self.start_freeze(i, want) {
                self.message = Some(format!(
                    "Couldn't make frozen audio for {}: {e}",
                    self.tracks[i].name
                ));
            }
        }
        if !dropped.is_empty() {
            self.write_frozen();
            for a in dropped {
                self.forget_frozen(&a);
            }
        }
    }

    fn start_freeze(&mut self, i: usize, frozen: Frozen) -> Result<(), String> {
        let s = self.session();
        let mut spec = s.tracks[i].clone();
        // Before the fader: frozen audio plays through it like the clips.
        spec.mix = Mix::default();
        spec.input = None;
        spec.arm = false;
        spec.monitor = false;
        crate::mixer::alone(&mut spec);
        let mut one = Session::new(self.tempo_map());
        one.tracks.push(spec);
        let mut plugins = Vec::new();
        let mut fx = Vec::new();
        for slot in &self.tracks[i].fx {
            let (p, proc) = self.effects.fresh(slot, self.rate)?;
            plugins.extend(p);
            fx.push((slot.id, proc));
        }
        let graph = one
            .compile_with(daw_audio_io::MAX_BLOCK, &mut fx)
            .map_err(|e| e.to_string())?;
        let key = self.tracks[i].key.clone();
        let path = cache_dir().join(format!("frozen-{key}-{}.wav", frozen.print));
        std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
        let cancel = Arc::new(AtomicBool::new(false));
        let (c, p) = (cancel.clone(), path.clone());
        let (from, frames) = (frozen.start, frozen.len as usize);
        let thread = std::thread::spawn(move || {
            write_wav(graph, Frames(from), frames, &p, &AtomicU64::new(0), &c)
        });
        self.freezer.jobs.push(Job {
            track: key,
            frozen,
            path,
            thread: Some(thread),
            cancel,
            _plugins: plugins,
        });
        Ok(())
    }

    /// Takes in finished renders, once a frame. Saves the project when
    /// they change it, and writes a bundle waiting on them.
    pub(crate) fn poll_freezing(&mut self, ctx: &egui::Context) {
        if self.freezer.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut changed = false;
        let mut dropped = Vec::new();
        while let Some(k) = (self.freezer.jobs.iter())
            .position(|j| j.thread.as_ref().is_none_or(|t| t.is_finished()))
        {
            let mut job = self.freezer.jobs.remove(k);
            let result = (job.thread.take()).map_or(Err("the render stopped".into()), |t| {
                t.join()
                    .unwrap_or_else(|_| Err("the render stopped".into()))
            });
            let Some(t) = self.tracks.iter().position(|t| t.key == job.track) else {
                let _ = std::fs::remove_file(&job.path);
                continue;
            };
            let Some(folder) = self.proj.folder.clone() else {
                let _ = std::fs::remove_file(&job.path);
                continue;
            };
            match result.and_then(|_| folder.add_asset(&job.path, true)) {
                Ok(asset) => {
                    if self.tracks[t].freeze {
                        // Frozen here: it plays this from now on.
                        match self.source_for(&asset) {
                            Ok(s) => self.tracks[t].frozen_audio = Some(s),
                            Err(e) => {
                                self.message = Some(format!(
                                    "Couldn't open the frozen audio for {}: {e}",
                                    self.tracks[t].name
                                ));
                            }
                        }
                    }
                    let mut f = job.frozen.clone();
                    f.asset = asset;
                    if let Some(old) = self.tracks[t].frozen.replace(f)
                        && Some(&old.asset) != self.tracks[t].frozen.as_ref().map(|f| &f.asset)
                    {
                        dropped.push(old.asset);
                    }
                    changed = true;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&job.path);
                    if e != "cancelled" {
                        self.message = Some(format!(
                            "Couldn't make frozen audio for {}: {e}",
                            self.tracks[t].name
                        ));
                    }
                }
            }
        }
        if changed {
            self.sync_doc(ctx, true);
            self.write_frozen();
            for a in dropped {
                self.forget_frozen(&a);
            }
            if self.recording {
                self.proj.dirty = true;
            } else if let Err(e) = self.save(ctx) {
                self.message = Some(format!("Couldn't save the frozen audio: {e}"));
            }
        }
        if !self.freezer.busy()
            && let Some(dest) = self.freezer.then_bundle.take()
        {
            self.write_bundle(dest);
        }
    }

    /// Writes the tracks' frozen audio into the document. Not an edit:
    /// kept out of undo.
    fn write_frozen(&mut self) {
        let p = self.project();
        self.proj.doc.set_quietly(&p);
        self.proj.committed = p;
        self.proj.dirty = true;
    }

    /// Removes an old frozen file from the project's audio, unless
    /// something still plays it.
    fn forget_frozen(&mut self, asset: &str) {
        let used = self.project().tracks.iter().any(|t| {
            t.clips.iter().any(|c| c.asset == asset)
                || t.frozen.as_ref().is_some_and(|f| f.asset == asset)
        });
        if used {
            return;
        }
        self.proj.sources.remove(asset);
        if let Some(folder) = &self.proj.folder
            && folder.has_asset(asset)
        {
            let _ = std::fs::remove_file(folder.asset(asset));
        }
    }

    /// The frozen audio `t` plays here instead of its clips and effects,
    /// if it does: when it is frozen, or one of its plugins is missing,
    /// and the audio still fits its clips.
    pub(crate) fn frozen_playing(&self, t: &Track) -> Option<ClipSpec> {
        if !t.freeze && !self.proj.missing_fx.contains_key(&t.key) {
            return None;
        }
        let f = t.frozen.as_ref()?;
        let src: &Arc<StreamSource> = t.frozen_audio.as_ref()?;
        if src.rate != self.rate || !f.fits(&self.track_value(t), self.rate) {
            return None;
        }
        let k = self.rate as f64 / f.rate.max(1) as f64;
        let start = (f.start as f64 * k).round().max(0.0) as i64;
        let len = ((f.len as f64 * k).round().max(0.0) as usize).min(src.frames);
        Some(ClipSpec {
            len: Some(len),
            ..ClipSpec::new(Frames(start), src.clone())
        })
    }

    /// A line about a track's frozen audio, for its effects window, and
    /// the effects missing here, by name.
    pub(crate) fn frozen_note(&self, t: &Track) -> (Option<String>, Vec<String>) {
        let missing: Vec<String> = (self.proj.missing_fx.get(&t.key).into_iter().flatten())
            .map(|(_, e)| e.name.clone())
            .collect();
        if !missing.is_empty() {
            let names = missing.join(", ");
            let line = if self.frozen_playing(t).is_some() {
                format!(
                    "Playing its frozen audio, as {names} isn't installed here. Your copy keeps \
                     the effects and their settings as they are."
                )
            } else if t.frozen.is_some() {
                format!(
                    "{names} isn't installed here, and the frozen audio no longer fits this \
                     track's clips, so it plays without it until someone with every plugin saves."
                )
            } else {
                format!(
                    "{names} isn't installed here and no frozen audio was shared, so it plays \
                     without it."
                )
            };
            return (Some(line), missing);
        }
        if t.freeze {
            let line = if self.frozen_playing(t).is_some() {
                "Frozen: it plays its frozen audio, with these effects switched off. Unfreeze \
                 it from its menu on the timeline to change them."
            } else if self.freezer.making(&t.key) {
                "Freezing…"
            } else if self.proj.folder.is_none() {
                "Frozen once the project is saved. Until then it plays through these effects."
            } else {
                "Frozen, but its clips or effects have changed since, so it plays through these \
                 effects until it is frozen again when the project is saved."
            };
            return (Some(line.into()), missing);
        }
        if !t.fx.iter().any(|s| s.is_plugin() && !s.bypass) {
            return (None, missing);
        }
        let line = if self.freezer.making(&t.key) {
            "Making frozen audio for computers without these plugins…"
        } else if self.proj.folder.is_none() {
            "Frozen audio, for computers without these plugins, is made once the project is saved."
        } else if t.frozen.is_some() {
            "Frozen audio, for computers without these plugins, is made each time the project is \
             saved."
        } else {
            "Frozen audio, for computers without these plugins, is made when the project is saved."
        };
        (Some(line.into()), missing)
    }
}

#[cfg(test)]
#[path = "../../plugins/tests/common/bundle.rs"]
pub(crate) mod test_plugin;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use daw_engine::engine::render_with;
    use daw_engine::time::Frames;
    use daw_project::{Effect, EffectState, ProjectDoc, ProjectFolder};
    use egui_kittest::Harness;

    use super::test_plugin;
    use crate::App;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-frozen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(|_cc| App::new(Vec::new()));
        h.run();
        h
    }

    fn until(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) {
        for _ in 0..1000 {
            h.step();
            if what(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let dest = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dest);
            } else {
                std::fs::copy(e.path(), dest).unwrap();
            }
        }
    }

    /// A second of a tone, mono, at `rate`.
    fn tone(dir: &Path, rate: u32) -> PathBuf {
        let path = dir.join("guitar.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..rate {
            let x = (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin();
            w.write_sample((x * 16_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    /// What the app plays, from the start, rendered offline with its own
    /// copies of any plugins.
    fn render(app: &mut App, frames: usize) -> Vec<f32> {
        let s = app.session();
        let mut keep = Vec::new();
        let mut fx = Vec::new();
        for slot in app.tracks.iter().flat_map(|t| &t.fx) {
            let (p, proc) = app.effects.fresh(slot, app.rate).unwrap();
            keep.push(p);
            fx.push((slot.id, proc));
        }
        let g = s.compile_with(daw_audio_io::MAX_BLOCK, &mut fx).unwrap();
        let mut out = Vec::new();
        render_with(g, Frames(0), frames, 256, |l, r| {
            for (a, b) in l.iter().zip(r) {
                out.extend([*a, *b]);
            }
            true
        });
        out
    }

    fn saved(dir: &Path) -> daw_project::Project {
        let (_, bytes) = ProjectFolder::open(dir).unwrap();
        ProjectDoc::from_bytes(&bytes, 1).unwrap().get()
    }

    #[test]
    fn a_track_missing_its_plugin_plays_its_frozen_audio() {
        let dir = scratch("missing");
        // The plugin in a folder of its own, so it can go missing.
        let plugin = dir.join("Plugins").join("DawTestGain.vst3");
        copy_dir(&test_plugin::bundle(), &plugin);
        let class = daw_plugins::scan::scan_here(&plugin)
            .unwrap()
            .into_iter()
            .find(|c| c.is_audio_plugin() && !c.is_instrument())
            .unwrap();

        let mut h = harness();
        let rate = h.state().rate;
        h.state_mut().add_file(&tone(&dir, rate));
        until(&mut h, |a| !a.tracks.is_empty());
        let effect = Effect {
            id: daw_project::new_id(),
            name: class.name.clone(),
            class: class.cid.iter().map(|&b| b as u8).collect(),
            bundle: plugin.to_string_lossy().into_owned(),
            bypass: false,
            // Half gain.
            state: Some(EffectState {
                component: 0.5f64.to_le_bytes().to_vec(),
                controller: None,
            }),
            ..Default::default()
        };
        let app = h.state_mut();
        let slot = app.effects.restore(&effect, rate).unwrap();
        let gain = slot.plugin().unwrap().params()[0].id;
        app.tracks[0].fx.push(slot);
        // A lane on the plugin: its half gain for a second, then an
        // eighth. The frozen audio follows it.
        let mut lane =
            daw_project::Lane::new(format!("{}{}:{gain}", daw_project::Lane::PLUGIN, effect.id));
        lane.put(0, 0.5);
        lane.put(rate as i64 / 2 - 1, 0.5);
        lane.put(rate as i64 / 2, 0.125);
        app.tracks[0].automation.push(lane);
        app.tracks[0].mix.gain_db = -6.0;
        app.rebuild();
        h.run();

        // Saving makes the frozen audio, then saves again with it.
        let song = dir.join("Song");
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &song).unwrap();
        assert!(h.state().freezer.making(&h.state().tracks[0].key));
        until(&mut h, |a| !a.freezer.busy());
        h.run();
        let frozen = h.state().tracks[0]
            .frozen
            .clone()
            .expect("frozen audio made");
        let folder = ProjectFolder { dir: song.clone() };
        assert!(folder.has_asset(&frozen.asset));
        assert_eq!(frozen.start, 0);
        assert_eq!(
            frozen.len,
            3 * rate as i64,
            "the clip and two seconds after"
        );
        assert!(!h.state().proj.dirty, "saved with it");
        assert_eq!(saved(&song).tracks[0].frozen.as_ref(), Some(&frozen));
        let effects = saved(&song).tracks[0].effects.clone();
        assert_eq!(
            effects[0].state.as_ref().unwrap().component,
            effect.state.as_ref().unwrap().component
        );

        // A bundle carries it.
        let bundle = dir.join("Song.dawbundle");
        h.state_mut().write_bundle(bundle.clone());
        until(&mut h, |a| a.proj.bundle_job.is_none());
        let elsewhere = ProjectFolder::create(&dir.join("Elsewhere")).unwrap();
        daw_collab::bundle::unpack(&bundle, &elsewhere).unwrap();
        assert!(elsewhere.has_asset(&frozen.asset));

        // Nothing changed: nothing is made again.
        h.state_mut().save(&ctx).unwrap();
        assert!(!h.state().freezer.busy());

        let frames = 3 * rate as usize;
        let with_plugin = render(h.state_mut(), frames);
        let loud = with_plugin.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(loud > 0.1, "the tone is heard ({loud})");
        drop(h);

        // Another computer, without the plugin.
        std::fs::remove_dir_all(&plugin).unwrap();
        let mut h = harness();
        h.state_mut().open_project(song.clone());
        until(&mut h, |a| !a.proj.loading());
        h.run();
        let app = h.state();
        assert!(app.tracks[0].fx.is_empty());
        assert!(app.proj.missing_fx.contains_key(&app.tracks[0].key));
        assert!(app.frozen_playing(&app.tracks[0]).is_some());
        let without = render(h.state_mut(), frames);
        assert_eq!(without.len(), with_plugin.len());
        // The plugin takes its lane's value once a block, so the lane's
        // step can land a block apart between the two renders: compare
        // away from it (stereo, so two samples a frame).
        let step = rate as usize;
        let away = |i: usize| i + 8192 < step || i > step + 8192;
        let worst = (with_plugin.iter().zip(&without).enumerate())
            .filter(|(i, _)| away(*i))
            .fold(0.0f32, |m, (_, (a, b))| m.max((a - b).abs()));
        assert!(worst < 1e-5, "sounds the same, within 24 bits ({worst})");
        let peak = |a: &[f32]| a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let (early, late) = (
            peak(&without[..step - 8192]),
            peak(&without[step + 8192..2 * step]),
        );
        assert!(
            late < early * 0.3,
            "the lane is in the frozen audio: {early} then {late}"
        );

        // Its own save keeps the effect, its settings and the frozen
        // audio as they were, and makes nothing.
        h.state_mut().tracks[0].mix.gain_db = -3.0;
        h.run();
        h.state_mut().save(&ctx).unwrap();
        assert!(!h.state().freezer.busy());
        let p = saved(&song);
        assert_eq!(p.tracks[0].effects, effects);
        assert_eq!(p.tracks[0].frozen.as_ref(), Some(&frozen));

        // Moved clips no longer fit: it plays without the plugin.
        h.state_mut().tracks[0].clips[0].start += rate as i64;
        h.run();
        let app = h.state();
        assert!(app.frozen_playing(&app.tracks[0]).is_none());
        let (line, missing) = app.frozen_note(&app.tracks[0]);
        assert_eq!(missing, vec![effect.name.clone()]);
        assert!(line.unwrap().contains("no longer fits"));
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_effects_keep_their_place_in_the_chain() {
        let dir = scratch("order");
        let plugin = dir.join("Plugins").join("DawTestGain.vst3");
        copy_dir(&test_plugin::bundle(), &plugin);
        let class = daw_plugins::scan::scan_here(&plugin)
            .unwrap()
            .into_iter()
            .find(|c| c.is_audio_plugin() && !c.is_instrument())
            .unwrap();
        let effect = |id: &str, class: Vec<u8>| Effect {
            id: id.into(),
            name: id.into(),
            class,
            bundle: plugin.to_string_lossy().into_owned(),
            bypass: false,
            state: Some(EffectState {
                component: 1f64.to_le_bytes().to_vec(),
                controller: None,
            }),
            ..Default::default()
        };
        let gain: Vec<u8> = class.cid.iter().map(|&b| b as u8).collect();
        let mut t = daw_project::Track::new("t1".into(), "Guitar".into());
        t.effects = vec![
            effect("first", gain.clone()),
            // Not installed anywhere.
            effect("missing", vec![9; 16]),
            effect("last", gain),
        ];
        let mut h = harness();
        let mut p = h.state().project();
        p.tracks.push(t);
        h.state_mut().apply(p);
        h.run();
        let app = h.state();
        assert_eq!(app.tracks[0].fx.len(), 2);
        let ids: Vec<String> = (app.project().tracks[0].effects.iter())
            .map(|e| e.id.clone())
            .collect();
        assert_eq!(ids, ["first", "missing", "last"]);
        // Its plugins aren't all here, so it isn't frozen here.
        let ctx = h.ctx.clone();
        h.state_mut().save_in(&ctx, &dir.join("Song")).unwrap();
        assert!(!h.state().freezer.busy());
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
