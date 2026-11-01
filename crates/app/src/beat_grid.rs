// SPDX-License-Identifier: GPL-3.0-or-later
//! The beat grid window (song test 3 wish list 1): the tempos a
//! selected clip's notes fit, found in the background from its audio
//! (see `daw_engine::beats`), each with how sure it is and where its
//! first downbeat falls, and buttons to use one: as the clip's own tempo
//! (so it follows the song's), as the song's tempo, or to put the
//! downbeat on the nearest bar line. Assistants do the same with
//! `analyse` `beats`, then `stretch_clips` `tempo`, `set_tempo` and
//! `edit_clip` `anchor_seconds`.

use std::sync::{Arc, Mutex};

use daw_engine::beats::{Grid, beat_grid, onsets};
use daw_engine::time::Frames;
use eframe::egui::{self, RichText};

use crate::App;
use crate::stretch::Window;

type Found = Result<Vec<Grid>, String>;

/// A beat grid being found, or found, for one clip.
pub(crate) struct Finding {
    /// The clip's id.
    pub clip: u64,
    /// The clip's offset into its file, in its file's frames, and the
    /// file's rate: the grid's seconds count from there.
    offset: usize,
    rate: u32,
    result: Arc<Mutex<Option<Found>>>,
    /// The candidate chosen in the window.
    pub chosen: usize,
}

impl Finding {
    /// The candidates, once found.
    pub(crate) fn found(&self) -> Option<Found> {
        self.result.lock().unwrap().clone()
    }

    /// The file frame of a time `seconds` into the clip's audio.
    fn frame(&self, seconds: f64) -> f64 {
        self.offset as f64 + seconds * self.rate as f64
    }
}

impl App {
    /// Opens the beat grid window and starts finding the grid of the
    /// first selected audio clip.
    pub(crate) fn find_beat_grid(&mut self) {
        self.stretch.message = None;
        self.stretch.open = Some(Window::BeatGrid);
        let selected = &self.timeline.selected;
        let Some(c) = (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .find(|c| selected.contains(&c.id))
        else {
            self.stretch.beat_grid = None;
            return;
        };
        let map = self.tempo_map();
        let beats = map.signature_at(map.ticks_at(Frames(c.start))).beats;
        let result = Arc::new(Mutex::new(None));
        let out = result.clone();
        let (source, offset, len) = (c.source.clone(), c.offset, c.len);
        let spawned = std::thread::Builder::new()
            .name("beat-grid".into())
            .spawn(move || {
                let found = source.read_mono(offset + len).map(|all| {
                    let x = &all[offset.min(all.len())..];
                    beat_grid(&onsets(x, x, source.rate), beats)
                });
                *out.lock().unwrap() = Some(found);
            });
        if let Err(e) = spawned {
            *result.lock().unwrap() = Some(Err(e.to_string()));
        }
        self.stretch.beat_grid = Some(Finding {
            clip: c.id,
            offset: c.offset,
            rate: c.source.rate,
            result,
            chosen: 0,
        });
    }

    /// The clip a beat grid was found for and the candidate chosen.
    fn chosen_grid(&self) -> Option<(&crate::timeline::Clip, Grid, &Finding)> {
        let f = self.stretch.beat_grid.as_ref()?;
        let grids = f.found()?.ok()?;
        let g = *grids.get(f.chosen)?;
        let c = (self.tracks.iter())
            .flat_map(|t| &t.clips)
            .find(|c| c.id == f.clip)?;
        Some((c, g, f))
    }

    /// Where the chosen candidate's first downbeat is on the timeline
    /// now, in frames.
    pub(crate) fn beat_grid_downbeat(&self) -> Option<i64> {
        let (c, g, f) = self.chosen_grid()?;
        Some(c.out_of_frame(&self.tempo_map(), f.frame(g.first_downbeat)))
    }

    /// The chosen candidate as the clip's own tempo: the clip follows
    /// the song's tempo from it.
    pub(crate) fn beat_grid_clip_tempo(&mut self) {
        let Some((c, g, _)) = self.chosen_grid() else {
            return;
        };
        let id = c.id;
        self.timeline.selected = [id].into_iter().collect();
        self.set_clip_tempo((g.bpm * 100.0).round() / 100.0);
    }

    /// The chosen candidate as the song's starting tempo. Clips that
    /// follow tempo move with it, as when the tempo is typed.
    pub(crate) fn beat_grid_song_tempo(&mut self) {
        if !self.may_edit_song() {
            return;
        }
        let Some((_, g, _)) = self.chosen_grid() else {
            return;
        };
        self.tempo_bpm = (g.bpm * 100.0).round() / 100.0;
        self.time_changed();
    }

    /// Moves the clip (and those grouped with it) so the chosen
    /// candidate's first downbeat lands on the nearest bar line.
    pub(crate) fn beat_grid_line_up(&mut self) {
        let Some(at) = self.beat_grid_downbeat() else {
            return;
        };
        let id = self.stretch.beat_grid.as_ref().map(|f| f.clip);
        self.timeline.selected = id.into_iter().collect();
        self.line_up(at, true);
    }

    pub(crate) fn beat_grid_window(&mut self, ctx: &egui::Context) {
        let may_edit = self.may_edit_song();
        let map = self.tempo_map();
        let song_bpm = self.tempo_bpm;
        let down = self.beat_grid_downbeat().map(|at| {
            let t = map.ticks_at(Frames(at));
            let bb = map.bar_beat(t);
            let beat = map.signature_at(t).beat_ticks() as f64;
            (bb.bar, bb.beat as f64 + bb.tick as f64 / beat)
        });
        let clip_name = (self.stretch.beat_grid.as_ref()).and_then(|f| {
            (self.tracks.iter())
                .flat_map(|t| &t.clips)
                .find(|c| c.id == f.clip)
                .map(|c| c.label.clone())
        });
        let mut open = true;
        let mut act = None;
        let finding = &mut self.stretch.beat_grid;
        egui::Window::new("Beat grid")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "The tempos the clip's notes fit, best first, from 60 to 200 BPM. Made for \
                     a voice alone or another sparse part; half or double the tempo often fits \
                     too.",
                );
                let Some(f) = finding else {
                    ui.label(RichText::new("Select an audio clip first.").weak());
                    return;
                };
                if let Some(name) = &clip_name {
                    ui.label(RichText::new(name).strong());
                }
                let grids = match f.found() {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Listening for the beat…");
                        });
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(100));
                        return;
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().warn_fg_color, e);
                        return;
                    }
                    Some(Ok(g)) if g.is_empty() => {
                        ui.label("Too few notes to find a beat in.");
                        return;
                    }
                    Some(Ok(g)) => g,
                };
                egui::Grid::new("beat-grid-candidates")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Tempo").strong());
                        ui.label(RichText::new("How sure").strong());
                        ui.label(RichText::new("Drifts by").strong());
                        ui.label("");
                        ui.end_row();
                        for (i, g) in grids.iter().enumerate() {
                            // ai: view
                            if ui
                                .selectable_label(f.chosen == i, format!("{:.2} BPM", g.bpm))
                                .clicked()
                            {
                                f.chosen = i;
                            }
                            ui.label(format!("{:.0}%", 100.0 * g.confidence));
                            ui.label(format!("{:.0} ms", 1000.0 * g.drift));
                            let rel = g.relation.map(|(r, j)| {
                                let name = if r == 0.5 {
                                    "half"
                                } else if r == 2.0 {
                                    "double"
                                } else if r == 1.5 {
                                    "3/2"
                                } else {
                                    "2/3"
                                };
                                format!("{name} of {:.2}", grids[j].bpm)
                            });
                            ui.label(RichText::new(rel.unwrap_or_default()).weak());
                            ui.end_row();
                        }
                    });
                let g = grids[f.chosen.min(grids.len() - 1)];
                if let Some((bar, beat)) = down {
                    ui.label(format!(
                        "First downbeat now at bar {bar}, beat {beat:.2}. It is a guess: \
                         {:.0}% surer than the next likeliest beat.",
                        100.0 * g.downbeat_confidence
                    ));
                }
                if !may_edit {
                    return;
                }
                ui.horizontal(|ui| {
                    // ai: stretch_clips
                    if ui
                        .button("Use as clip's tempo")
                        .on_hover_text("The clip follows the song's tempo from this one")
                        .clicked()
                    {
                        act = Some(0);
                    }
                    // ai: set_tempo
                    if ui
                        .button(format!("Use as song tempo (now {song_bpm:.2})"))
                        .clicked()
                    {
                        act = Some(1);
                    }
                    // ai: edit_clip
                    if ui
                        .button("Put downbeat on nearest bar")
                        .on_hover_text("Moves the clip so its first downbeat is on a bar line")
                        .clicked()
                    {
                        act = Some(2);
                    }
                });
            });
        match act {
            Some(0) => self.beat_grid_clip_tempo(),
            Some(1) => self.beat_grid_song_tempo(),
            Some(2) => self.beat_grid_line_up(),
            _ => {}
        }
        if !open {
            self.stretch.open = None;
            self.stretch.beat_grid = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use crate::agent::driver::Driver;
    use crate::listen::tests::a_cappella;

    /// Waits for the window's beat grid to be found.
    fn found(d: &mut Driver) -> Vec<daw_engine::beats::Grid> {
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(r) = d.app().stretch.beat_grid.as_ref().and_then(|f| f.found()) {
                return r.unwrap();
            }
            assert!(Instant::now() < until, "the beat grid took too long");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn the_window_finds_a_voices_beat_and_puts_it_on_the_grid() {
        let mut d = Driver::new();
        let clip = a_cappella(&mut d, 116.4, 0.7, 12, 3.3);
        d.call("select_clips", json!({"clips": [clip]})).unwrap();
        d.command("beat-grid");
        let grids = found(&mut d);
        assert!((grids[0].bpm - 116.4).abs() < 0.3, "{grids:?}");

        // Its downbeat onto the nearest bar line.
        d.app().beat_grid_line_up();
        let at = d.app().beat_grid_downbeat().unwrap();
        let map = d.app().tempo_map();
        let bb = map.bar_beat(map.ticks_at(daw_engine::time::Frames(at)));
        assert!(
            (bb.beat, bb.tick) == (1, 0) || (bb.beat == 4 && bb.tick >= 959),
            "{bb:?}"
        );

        // As the clip's own tempo, then as the song's.
        d.app().beat_grid_clip_tempo();
        let c = d.project()["tracks"][0]["clips"][0].clone();
        let bpm = (grids[0].bpm * 100.0).round() / 100.0;
        assert_eq!(c["tempo"], json!(bpm), "{c}");
        d.app().beat_grid_song_tempo();
        assert_eq!(d.project()["tempo_bpm"], json!(bpm));
        d.command("undo");
        assert_eq!(d.project()["tempo_bpm"], json!(120.0));
    }
}
