// SPDX-License-Identifier: GPL-3.0-or-later
//! Track and effect chain presets (plan A3): Track → Save track as
//! preset… keeps a track's whole set-up (instrument, layers, effects,
//! amp, sends and mixer settings), Save effect chain as preset… just its
//! effects, and Presets… lists them to add to any project, as a new
//! track or onto one there. The presets that come with the app are kept
//! apart from the user's, which can be tagged, found by tag or name,
//! exported as one file and imported again on another computer, so a
//! teacher can send a student a tone. See `daw_project::Preset` for what
//! a preset keeps.
//!
//! The user's presets are files in their profile, e.g.
//! `C:\Users\<name>\DAW\Presets\Crunch guitar.dawpreset`. Each is a
//! bundle holding the sound files it uses, so it plays the same wherever
//! it goes. Those files go into the project when a preset is added (or,
//! before the project is first saved, into `Presets\Files`, from where
//! saving brings them in).
//!
//! Every action here is an `App` method that takes what it acts on, so
//! the command table, menus and keys (and later the agent interface)
//! all call the same code.

use std::path::{Path, PathBuf};

use daw_collab::bundle;
use daw_effects::Kind;
use daw_project::{
    PRESET_EXTENSION, Preset, PresetKind, Project, ProjectDoc, ProjectFolder, Track, TrackKind,
    new_id, new_peer, parse_tags,
};
use eframe::egui::{self, RichText};

use crate::templates::file_name_ok;
use crate::{APP_NAME, App, user_dir};

/// Where the user's own presets are kept.
pub fn presets_dir() -> PathBuf {
    user_dir().join(APP_NAME).join("Presets")
}

/// Where a preset's sound files wait for a project never saved.
const FILES: &str = "Files";

/// Whether `path` is named like a preset.
pub fn is_preset(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(PRESET_EXTENSION))
}

/// A preset in the list: one of the user's, with its file, or one that
/// comes with the app.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) preset: Preset,
    pub(crate) file: Option<PathBuf>,
}

pub(crate) struct Presets {
    /// The Presets window.
    pub(crate) library: Option<Library>,
    /// The Save as preset window.
    pub(crate) saving: Option<Saving>,
    /// Where the user's presets are; none in tests until they set one,
    /// as they must not touch the user's own.
    pub(crate) dir: Option<PathBuf>,
}

impl Default for Presets {
    fn default() -> Presets {
        Presets {
            library: None,
            saving: None,
            dir: (!cfg!(test)).then(presets_dir),
        }
    }
}

pub(crate) struct Library {
    list: Vec<Entry>,
    /// Files that couldn't be read, and why.
    problems: Vec<String>,
    picked: Option<usize>,
    /// Only those whose name, tags or description have this in them.
    find: String,
    /// Only those with this tag.
    tag: Option<String>,
    /// The track Put on puts the picked preset onto, by key.
    onto: Option<String>,
    /// One of the user's whose Delete… was pressed, waiting for yes.
    deleting: Option<usize>,
}

/// The Save as preset window, as filled in so far.
pub(crate) struct Saving {
    pub(crate) track: usize,
    pub(crate) kind: PresetKind,
    pub(crate) name: String,
    pub(crate) tags: String,
    pub(crate) about: String,
}

/// A built-in effect with some of its settings changed, by key.
fn fx(kind: Kind, set: &[(&str, f64)]) -> daw_project::Effect {
    debug_assert!(
        set.iter()
            .all(|(k, _)| kind.params().iter().any(|p| p.key == *k)),
        "{} has no setting among {set:?}",
        kind.id()
    );
    let e = daw_effects::Effect::new(kind);
    let values: Vec<(String, f64)> = set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    e.load(&values);
    let mut params = e.values();
    params.sort_by(|a, b| a.0.cmp(&b.0));
    daw_project::Effect {
        id: new_id(),
        name: kind.name().into(),
        builtin: kind.id().into(),
        params,
        ..daw_project::Effect::default()
    }
}

/// A switching container with `clean` in its first chain and `driven`
/// in its second, followed by `after`, as a chain's effects.
fn switching(
    clean: Vec<daw_project::Effect>,
    driven: Vec<daw_project::Effect>,
    after: Vec<daw_project::Effect>,
) -> Vec<daw_project::Effect> {
    let mut switch = fx(Kind::Switch, &[]);
    switch.name = "Clean or driven".into();
    let mut list = vec![switch.clone()];
    for (branch, chain) in [clean, driven].into_iter().enumerate() {
        list.extend(chain.into_iter().map(|e| daw_project::Effect {
            container: Some(switch.id.clone()),
            branch: branch as u32,
            ..e
        }));
    }
    list.extend(after);
    list
}

/// An instrument track playing one of the app's instruments with some
/// of its settings changed, by key, as a track preset.
fn instrument_preset(
    kind: daw_instruments::Kind,
    name: &str,
    tags: &str,
    about: &str,
    set: &[(&str, f64)],
) -> Preset {
    let b = daw_instruments::Builtin::new(kind);
    let p = b.params();
    for (k, v) in set {
        debug_assert!(p.index(k).is_some(), "{} has no setting {k}", kind.id());
        p.set_key(k, *v);
    }
    let mut params = p.values();
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let mut track = Track::new(String::new(), name.into());
    track.kind = TrackKind::Instrument;
    track.instrument = Some(daw_project::Effect {
        id: new_id(),
        name: kind.name().into(),
        builtin: kind.id().into(),
        params,
        ..daw_project::Effect::default()
    });
    Preset {
        name: name.into(),
        kind: PresetKind::Track,
        tags: parse_tags(tags),
        about: about.into(),
        track,
    }
}

/// The presets that come with the app. Placeholders until the owner
/// chooses which tones ship (see the review queue).
pub(crate) fn included() -> Vec<Preset> {
    let chain = |name: &str, tags: &str, about: &str, effects| {
        let mut track = Track::new(String::new(), name.into());
        track.effects = effects;
        Preset {
            name: name.into(),
            kind: PresetKind::Chain,
            tags: parse_tags(tags),
            about: about.into(),
            track,
        }
    };
    // The amp's voices: 0 clean, 1 warm, 2 crunch, 3 high gain.
    let mut list = vec![
        chain(
            "Clean guitar",
            "guitar, clean",
            "A clean amp, a little compression and a small room.",
            vec![
                fx(Kind::Amp, &[("voice", 0.0), ("gain", 3.0), ("treble", 6.0)]),
                fx(Kind::Compressor, &[("threshold", -20.0), ("ratio", 3.0)]),
                fx(Kind::Reverb, &[("decay", 1.2), ("mix", 18.0)]),
            ],
        ),
        chain(
            "Crunch guitar",
            "guitar, crunch",
            "Crunch for chords and riffs, with a short echo.",
            vec![
                fx(Kind::Amp, &[("voice", 2.0), ("gain", 6.0)]),
                fx(
                    Kind::Delay,
                    &[("time", 375.0), ("feedback", 25.0), ("mix", 12.0)],
                ),
                fx(Kind::Reverb, &[("mix", 15.0)]),
            ],
        ),
        chain(
            "High gain rhythm",
            "guitar, high gain, rhythm",
            "A gate to keep it quiet between chords, then a high-gain amp.",
            vec![
                fx(Kind::Gate, &[("threshold", -50.0)]),
                fx(Kind::Amp, &[("voice", 3.0), ("gain", 7.0), ("middle", 4.0)]),
                fx(Kind::Eq, &[("low_cut", 80.0)]),
            ],
        ),
        chain(
            "Clean and driven guitar",
            "guitar, clean, crunch, switch",
            "A clean amp channel and a driven one, switched by a key, a footswitch or \
             automation (Switch to the next chain), into one reverb.",
            switching(
                vec![
                    fx(Kind::Compressor, &[("threshold", -20.0), ("ratio", 3.0)]),
                    fx(Kind::Amp, &[("voice", 0.0), ("gain", 3.0), ("treble", 6.0)]),
                ],
                vec![
                    fx(Kind::Gate, &[("threshold", -55.0)]),
                    fx(Kind::Amp, &[("voice", 2.0), ("gain", 7.0)]),
                ],
                vec![fx(Kind::Reverb, &[("decay", 1.2), ("mix", 15.0)])],
            ),
        ),
        chain(
            "Bass DI",
            "bass",
            "Even compression and a little low end for a bass played straight in.",
            vec![
                fx(
                    Kind::Compressor,
                    &[("threshold", -24.0), ("ratio", 4.0), ("attack", 20.0)],
                ),
                fx(Kind::Eq, &[("low_cut", 30.0), ("low_gain", 2.0)]),
            ],
        ),
        chain(
            "Vocal",
            "vocal",
            "Low cut, compression, softer s sounds and a little reverb.",
            vec![
                fx(Kind::Eq, &[("low_cut", 100.0)]),
                fx(Kind::Compressor, &[("threshold", -20.0), ("ratio", 3.0)]),
                fx(Kind::DeEsser, &[]),
                fx(Kind::Reverb, &[("mix", 15.0)]),
            ],
        ),
    ];
    // Patch Twenty, the semi-modular monosynth (classic synths, S7):
    // sounds made from its settings alone.
    use daw_instruments::Kind::PatchTwenty;
    list.extend([
        instrument_preset(
            PatchTwenty,
            "Patch Twenty screaming lead",
            "synth, lead, mono, aggressive",
            "Pulse against a ring modulator, both filters peaking, envelope 2 opening the \
             low-pass, the keyboard tracking both filters and the mod generator moving \
             the pulse width.",
            &[
                ("osc1", 2.0),
                ("osc2", 3.0),
                ("osc2_level", 0.7),
                ("hp_cutoff", 150.0),
                ("hp_peak", 0.7),
                ("lp_cutoff", 500.0),
                ("lp_peak", 0.95),
                ("lp_eg2", 3.0),
                ("drive", 0.8),
                ("eg2_decay", 0.5),
                ("eg2_sustain", 0.4),
                ("glide", 0.05),
                ("retrigger", 0.0),
                ("patch1_from", 7.0),
                ("patch1_to", 7.0),
                ("patch1_amount", 0.7),
                ("patch2_from", 1.0),
                ("patch2_to", 4.0),
                ("patch2_amount", 0.6),
                ("vintage", 0.5),
            ],
        ),
        instrument_preset(
            PatchTwenty,
            "Patch Twenty bass",
            "synth, bass, mono",
            "A saw an octave down with a square under it, the low-pass snapping shut on \
             each note, a little drive and a short glide between notes.",
            &[
                ("osc1", 1.0),
                ("osc1_octave", 1.0),
                ("osc2", 1.0),
                ("osc2_octave", 0.0),
                ("osc2_level", 0.5),
                ("lp_cutoff", 250.0),
                ("lp_peak", 0.5),
                ("lp_eg2", 2.5),
                ("eg2_decay", 0.25),
                ("eg2_sustain", 0.1),
                ("drive", 0.5),
                ("glide", 0.06),
                ("retrigger", 0.0),
                ("vintage", 0.4),
            ],
        ),
        instrument_preset(
            PatchTwenty,
            "Patch Twenty drone",
            "synth, drone, pad, mono",
            "No key needed: the amplifier is held open, the mod generator sweeps the \
             low-pass, its sample-and-hold steps the high-pass, the oscillators a fifth \
             apart. Play one note to set the pitch.",
            &[
                ("osc1", 1.0),
                ("osc1_octave", 1.0),
                ("osc2", 0.0),
                ("osc2_pitch", 7.0),
                ("osc2_level", 0.6),
                ("hp_cutoff", 120.0),
                ("hp_peak", 0.6),
                ("lp_cutoff", 400.0),
                ("lp_peak", 0.8),
                ("lp_eg2", 0.0),
                ("lp_mg", 1.5),
                ("mg_rate", 0.25),
                ("mg_shape", 0.3),
                ("drive", 0.6),
                ("drone", 1.0),
                ("patch1_from", 5.0),
                ("patch1_to", 5.0),
                ("patch1_amount", 0.8),
                ("patch2_from", 1.0),
                ("patch2_to", 9.0),
                ("patch2_amount", 0.5),
                ("vintage", 0.6),
            ],
        ),
        instrument_preset(
            PatchTwenty,
            "Guitar through Patch Twenty",
            "guitar, synth, filter, external",
            "The guitar through the synth's two filters: pick the guitar's input on the \
             track's strip and turn on Monitor. Each pluck triggers the envelope, which \
             sweeps the low-pass; no oscillators.",
            &[
                ("osc1_level", 0.0),
                ("osc2_level", 0.0),
                ("ext_in", 1.0),
                ("ext_gain", 6.0),
                ("ext_trigger", 1.0),
                ("ext_threshold", -24.0),
                ("hp_cutoff", 90.0),
                ("hp_peak", 0.3),
                ("lp_cutoff", 300.0),
                ("lp_peak", 0.75),
                ("lp_eg2", 3.0),
                ("eg2_attack", 0.005),
                ("eg2_decay", 0.35),
                ("eg2_sustain", 0.2),
                ("eg2_release", 0.3),
                ("drive", 0.6),
                ("level", -4.0),
            ],
        ),
    ]);
    let mut bus = Track::new(String::new(), "Reverb".into());
    bus.kind = TrackKind::Bus;
    bus.effects = vec![fx(Kind::Reverb, &[("decay", 2.2), ("mix", 100.0)])];
    list.push(Preset {
        name: "Reverb bus".into(),
        kind: PresetKind::Track,
        tags: parse_tags("bus, reverb"),
        about: "A bus that is all reverb, for tracks to send to.".into(),
        track: bus,
    });
    list
}

/// The preset in the file at `path`, without taking in its sound files.
pub(crate) fn read_preset(path: &Path) -> Result<Preset, String> {
    let b = bundle::peek(path)?;
    preset_of(path, b)
}

fn preset_of(path: &Path, b: bundle::Bundle) -> Result<Preset, String> {
    let stem = path
        .file_stem()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let meta = b
        .meta
        .ok_or_else(|| format!("{} is a project bundle, not a preset", path.display()))?;
    let p = ProjectDoc::from_bytes(&b.doc, new_peer())?.get();
    Preset::from_parts(p, &String::from_utf8_lossy(&meta), &stem)
        .map_err(|e| format!("{} is not a preset: {e}", path.display()))
}

/// The user's presets in `dir`, by name, and the files that couldn't be
/// read.
fn read_presets(dir: &Path) -> (Vec<Entry>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (found, problems);
    };
    for e in entries.flatten() {
        let path = e.path();
        if !is_preset(&path) {
            continue;
        }
        match read_preset(&path) {
            Ok(preset) => found.push(Entry {
                preset,
                file: Some(path),
            }),
            Err(e) => problems.push(e),
        }
    }
    found.sort_by_key(|e| e.preset.name.to_lowercase());
    (found, problems)
}

/// Included presets first, then the user's in `dir`, and the files that
/// couldn't be read.
pub(crate) fn all_presets(dir: Option<&Path>) -> (Vec<Entry>, Vec<String>) {
    let mut list: Vec<Entry> = (included().into_iter())
        .map(|preset| Entry { preset, file: None })
        .collect();
    let (mine, problems) = dir.map(read_presets).unwrap_or_default();
    list.extend(mine);
    (list, problems)
}

/// Writes `preset` to `dest` with the sound files it uses, found by
/// `find`. Returns the files that couldn't be found (and so aren't in
/// it). The file appears whole or not at all.
pub(crate) fn write_preset(
    dest: &Path,
    preset: &Preset,
    find: impl Fn(&str) -> Option<PathBuf>,
) -> Result<Vec<String>, String> {
    let mut preset = preset.clone();
    // The files are named by their contents in a folder of their own
    // first, as a project's are, for the bundle to take them from.
    let scratch =
        std::env::temp_dir().join(format!("daw-preset-{}-{}", std::process::id(), new_id()));
    let files = ProjectFolder {
        dir: scratch.clone(),
    };
    let mut names = std::collections::HashMap::new();
    let mut left_out = Vec::new();
    let written = (|| {
        for f in preset.files() {
            match find(&f).filter(|p| p.is_file()) {
                Some(path) => {
                    names.insert(f, files.add_asset(&path, false)?);
                }
                None => left_out.push(f),
            }
        }
        preset.rename_files(|f| names.get(f).cloned());
        let assets: Vec<String> = names.values().cloned().collect();
        let mut doc = ProjectDoc::new(new_peer());
        doc.set_quietly(&preset.to_project());
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        bundle::write_with(
            dest,
            &preset.name,
            &doc.to_bytes(),
            &files,
            &assets,
            Some(preset.meta().as_bytes()),
        )
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    written.map(|()| left_out)
}

/// What a preset couldn't find, as a sentence to add to a message.
fn dropped_note(d: &daw_project::Dropped) -> String {
    match d.missing.as_slice() {
        [] => String::new(),
        [one] => format!(" This project has no track called {one}, so that route was left out."),
        more => format!(
            " This project has no tracks called {}, so those routes were left out.",
            more.join(", ")
        ),
    }
}

impl App {
    /// The track presets are saved from or put onto: the one whose
    /// effects or instrument window is open, else the one with the
    /// selected clip or note clip, else the first.
    pub(crate) fn preset_track(&self) -> Option<usize> {
        let by_clip =
            |id: u64| (self.tracks.iter()).position(|t| t.clips.iter().any(|c| c.id == id));
        let by_note =
            |id: &str| (self.tracks.iter()).position(|t| t.note_clips.iter().any(|c| c.id == id));
        (self.effects.open_track)
            .or(self.instrument_open)
            .filter(|&i| i < self.tracks.len())
            .or_else(|| self.timeline.selected.iter().find_map(|&id| by_clip(id)))
            .or_else(|| self.timeline.note_sel.as_deref().and_then(by_note))
            .or_else(|| (!self.tracks.is_empty()).then_some(0))
    }

    /// Opens the Presets window.
    pub(crate) fn open_presets(&mut self) {
        let (list, problems) = all_presets(self.preset_ui.dir.as_deref());
        let onto = self.preset_track().map(|i| self.tracks[i].key.clone());
        self.preset_ui.library = Some(Library {
            list,
            problems,
            picked: None,
            find: String::new(),
            tag: None,
            onto,
            deleting: None,
        });
    }

    /// Opens the Save as preset window for track `track`.
    pub(crate) fn ask_save_preset(&mut self, track: Option<usize>, kind: PresetKind) {
        let Some(track) = track.or_else(|| self.preset_track()) else {
            self.message = Some("There is no track to save as a preset.".into());
            return;
        };
        let name = match kind {
            PresetKind::Track => self.tracks[track].name.clone(),
            PresetKind::Chain => format!("{} effects", self.tracks[track].name),
        };
        self.preset_ui.saving = Some(Saving {
            track,
            kind,
            name,
            tags: String::new(),
            about: String::new(),
        });
    }

    /// Keeps track `track` (or its effect chain) as the user's preset
    /// `name`, replacing one of that name. Returns its file, and the
    /// sound files it uses that couldn't be found.
    pub(crate) fn save_preset(
        &mut self,
        ctx: &egui::Context,
        track: usize,
        kind: PresetKind,
        name: &str,
        tags: &[String],
        about: &str,
    ) -> Result<(PathBuf, Vec<String>), String> {
        if !file_name_ok(name) {
            return Err("Choose a name without / \\ : * ? \" < > |".into());
        }
        let Some(dir) = self.preset_ui.dir.clone() else {
            return Err("There is nowhere to keep presets.".into());
        };
        self.sync_doc(ctx, true);
        self.write_plugin_states();
        let p = self.shown_project();
        let key = self
            .tracks
            .get(track)
            .ok_or("There is no such track.")?
            .key
            .clone();
        let ti = (p.tracks.iter())
            .position(|t| t.id == key)
            .ok_or("There is no such track.")?;
        let mut preset = Preset::of_track(&p, ti, kind, name).ok_or("There is no such track.")?;
        preset.tags = tags.to_vec();
        preset.about = about.trim().to_string();
        let folder = self.proj.folder.clone();
        let path = dir.join(format!("{name}.{PRESET_EXTENSION}"));
        let left_out = write_preset(&path, &preset, |f| match &folder {
            Some(fo) if fo.has_asset(f) => Some(fo.asset(f)),
            _ => Some(PathBuf::from(f)),
        })?;
        Ok((path, left_out))
    }

    /// The preset in `e`, ready to add: a file's sound files taken into
    /// the project (or kept aside until it is saved).
    fn ready(&mut self, e: &Entry) -> Result<Preset, String> {
        let Some(path) = &e.file else {
            return Ok(e.preset.clone());
        };
        let (into, in_project) = match &self.proj.folder {
            Some(f) => (f.clone(), true),
            None => {
                let dir = (self.preset_ui.dir.clone())
                    .ok_or("There is nowhere to keep the preset's files.")?;
                let into = ProjectFolder {
                    dir: dir.join(FILES),
                };
                (into, false)
            }
        };
        let mut preset = preset_of(path, bundle::unpack(path, &into)?)?;
        if !in_project {
            preset.rename_files(|f| {
                daw_project::is_asset_name(f).then(|| into.asset(f).to_string_lossy().into_owned())
            });
        }
        Ok(preset)
    }

    /// Makes a change to the project, written as one undoable edit when
    /// the document is next brought up to date (labelled, for an
    /// assistant's).
    fn edit_project<R>(
        &mut self,
        ctx: &egui::Context,
        f: impl FnOnce(&mut Project) -> Result<R, String>,
    ) -> Result<R, String> {
        self.sync_doc(ctx, true);
        let mut p = self.project();
        let r = f(&mut p)?;
        let before = self.proj.committed.clone();
        self.apply(p);
        // Not yet written: the next sync writes it.
        self.proj.committed = before;
        Ok(r)
    }

    /// The preset called `name`: the user's if they have one by that
    /// name, else one that comes with the app.
    pub(crate) fn find_preset(&self, name: &str) -> Result<Entry, String> {
        let (list, _) = all_presets(self.preset_ui.dir.as_deref());
        let mut found: Vec<Entry> = list.into_iter().filter(|e| e.preset.name == name).collect();
        found.sort_by_key(|e| e.file.is_none());
        found
            .into_iter()
            .next()
            .ok_or_else(|| format!("There is no preset called {name}."))
    }

    /// Adds the preset in `e` to the project as a new track. Returns
    /// the track's index, and a note on routes it couldn't keep.
    pub(crate) fn add_preset_track(
        &mut self,
        ctx: &egui::Context,
        e: &Entry,
    ) -> Result<(usize, String), String> {
        let preset = self.ready(e)?;
        self.edit_project(ctx, |p| {
            let (i, d) = preset.add_to(p);
            Ok((i, dropped_note(&d)))
        })
    }

    /// Puts the preset in `e` onto track `track` (see
    /// `daw_project::Preset::apply_to`). Returns a note on routes it
    /// couldn't keep.
    pub(crate) fn apply_preset(
        &mut self,
        ctx: &egui::Context,
        track: usize,
        e: &Entry,
    ) -> Result<String, String> {
        let key = self
            .tracks
            .get(track)
            .ok_or("There is no such track.")?
            .key
            .clone();
        let preset = self.ready(e)?;
        self.edit_project(ctx, |p| {
            let ti = (p.tracks.iter())
                .position(|t| t.id == key)
                .ok_or("There is no such track.")?;
            preset.apply_to(p, ti).map(|d| dropped_note(&d))
        })
    }

    /// Takes the preset file at `path` into the user's presets, under a
    /// name of its own if one of theirs has its name. Returns that name.
    pub(crate) fn import_preset(&mut self, path: &Path) -> Result<String, String> {
        let preset = read_preset(path)?;
        let dir = (self.preset_ui.dir.clone()).ok_or("There is nowhere to keep presets.")?;
        let base = if file_name_ok(&preset.name) {
            preset.name.clone()
        } else {
            "Imported preset".to_string()
        };
        let mut name = base.clone();
        let mut n = 2;
        while dir.join(format!("{name}.{PRESET_EXTENSION}")).exists() {
            name = format!("{base} {n}");
            n += 1;
        }
        let mut preset = preset;
        preset.name = name.clone();
        // Written again (not copied) so it carries the name it has here.
        let folder = ProjectFolder {
            dir: dir.join(FILES),
        };
        let b = bundle::unpack(path, &folder)?;
        let mut read = preset_of(path, b)?;
        read.name = name.clone();
        write_preset(
            &dir.join(format!("{name}.{PRESET_EXTENSION}")),
            &read,
            |f| Some(folder.asset(f)),
        )?;
        Ok(name)
    }

    /// Writes the preset in `e` to `dest`, to send to someone.
    pub(crate) fn export_preset(&mut self, e: &Entry, dest: &Path) -> Result<(), String> {
        match &e.file {
            Some(src) if src != dest => std::fs::copy(src, dest)
                .map(|_| ())
                .map_err(|err| format!("Couldn't export the preset: {err}")),
            Some(_) => Ok(()),
            None => write_preset(dest, &e.preset, |_| None).map(|_| ()),
        }
    }

    /// Deletes the user's preset in `e`.
    pub(crate) fn delete_preset(&mut self, e: &Entry) -> Result<(), String> {
        let path = e
            .file
            .as_ref()
            .ok_or("Presets that come with the app can't be deleted.")?;
        std::fs::remove_file(path).map_err(|err| format!("Couldn't delete the preset: {err}"))
    }

    /// Asks for a preset file and imports it.
    pub(crate) fn pick_preset_to_import(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Import preset")
            .set_directory(user_dir())
            .add_filter("Preset", &[PRESET_EXTENSION])
            .pick_file();
        if let Some(path) = picked {
            self.import_dropped_preset(&path);
        }
    }

    /// Imports a preset given by a file dialog or dropped on the window,
    /// saying how it went.
    pub(crate) fn import_dropped_preset(&mut self, path: &Path) {
        self.message = Some(match self.import_preset(path) {
            Ok(name) => {
                if self.preset_ui.library.is_some() {
                    self.open_presets();
                }
                format!("Imported the preset “{name}”. It is in Presets….")
            }
            Err(e) => format!("Couldn't import the preset: {e}"),
        });
    }

    pub(crate) fn preset_windows(&mut self, ctx: &egui::Context) {
        if let Some(ask) = self.effects.chain_ask.take() {
            match ask {
                crate::fx::ChainAsk::Save => {
                    self.ask_save_preset(self.effects.open_track, PresetKind::Chain)
                }
                crate::fx::ChainAsk::Load => self.open_presets(),
            }
        }
        self.library_window(ctx);
        self.save_preset_window(ctx);
    }

    fn library_window(&mut self, ctx: &egui::Context) {
        let Some(mut lib) = self.preset_ui.library.take() else {
            return;
        };
        let mut keep = true;
        let mut add = None;
        let mut put = None;
        let mut export = None;
        let mut delete = None;
        let mut save = None;
        let mut import = false;
        let tracks: Vec<(String, String)> = (self.tracks.iter())
            .map(|t| (t.key.clone(), t.name.clone()))
            .collect();
        if lib
            .onto
            .as_ref()
            .is_none_or(|k| !tracks.iter().any(|t| &t.0 == k))
        {
            lib.onto = tracks.first().map(|t| t.0.clone());
        }
        let mut tags: Vec<String> = (lib.list.iter())
            .flat_map(|e| e.preset.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        egui::Window::new("Presets")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let l = ui.label("Find");
                    ui.add(
                        // ai: view
                        egui::TextEdit::singleline(&mut lib.find)
                            .hint_text("name, tag or word")
                            .desired_width(180.0),
                    )
                    .labelled_by(l.id);
                    egui::ComboBox::from_id_salt("preset-tag")
                        .selected_text(lib.tag.as_deref().unwrap_or("Any tag"))
                        .show_ui(ui, |ui| {
                            // ai: view
                            ui.selectable_value(&mut lib.tag, None, "Any tag");
                            for t in &tags {
                                // ai: view
                                ui.selectable_value(&mut lib.tag, Some(t.clone()), t);
                            }
                        })
                        .response
                        .accessible_name("Tag");
                });
                let find = lib.find.trim().to_lowercase();
                let shown = |e: &Entry| {
                    let p = &e.preset;
                    lib.tag.as_ref().is_none_or(|t| p.tags.contains(t))
                        && (find.is_empty()
                            || p.name.to_lowercase().contains(&find)
                            || p.about.to_lowercase().contains(&find)
                            || p.tags.iter().any(|t| t.contains(&find)))
                };
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        let mut heading = None;
                        let mut any = false;
                        for (i, e) in lib.list.iter().enumerate() {
                            if !shown(e) {
                                continue;
                            }
                            any = true;
                            let group = if e.file.is_some() {
                                "Yours"
                            } else {
                                "Included"
                            };
                            if heading != Some(group) {
                                ui.label(RichText::new(group).strong());
                                heading = Some(group);
                            }
                            // ai: add_preset.name
                            let r = ui.selectable_label(lib.picked == Some(i), &e.preset.name);
                            if r.clicked() {
                                lib.picked = Some(i);
                                lib.deleting = None;
                            }
                            if r.double_clicked() {
                                add = Some(i);
                            }
                            let mut line = e.preset.kind.name().to_string();
                            if !e.preset.tags.is_empty() {
                                line += &format!(" · {}", e.preset.tags.join(", "));
                            }
                            if !e.preset.about.is_empty() {
                                line += &format!(" · {}", e.preset.about);
                            }
                            ui.label(RichText::new(line).weak());
                        }
                        if !any {
                            ui.label(RichText::new("No presets match.").weak());
                        }
                    });
                for p in &lib.problems {
                    ui.label(RichText::new(format!("Couldn't read {p}")).weak());
                }
                ui.separator();
                let picked = lib.picked.filter(|&i| lib.list.get(i).is_some_and(shown));
                if let Some(i) = lib.deleting {
                    ui.label(format!(
                        "Delete the preset “{}”? Tracks made with it stay as they are.",
                        lib.list[i].preset.name
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            delete = Some(i);
                        }
                        // ai: view
                        if ui.button("Cancel").clicked() {
                            lib.deleting = None;
                        }
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    if ui
                        // ai: add_preset.name
                        .add_enabled(picked.is_some(), egui::Button::new("Add as a new track"))
                        .clicked()
                    {
                        add = picked;
                    }
                    let onto_name = (tracks.iter())
                        .find(|t| Some(&t.0) == lib.onto.as_ref())
                        .map_or("", |t| t.1.as_str());
                    if ui
                        .add_enabled(
                            picked.is_some() && lib.onto.is_some(),
                            // ai: add_preset.name
                            egui::Button::new("Put on"),
                        )
                        .on_hover_text(
                            "An effect chain replaces the track's effects; a track preset \
                             replaces its instrument, effects, sends and settings, keeping \
                             its clips",
                        )
                        .clicked()
                    {
                        put = picked;
                    }
                    egui::ComboBox::from_id_salt("preset-onto")
                        .selected_text(onto_name)
                        .show_ui(ui, |ui| {
                            for (k, n) in &tracks {
                                // ai: view
                                ui.selectable_value(&mut lib.onto, Some(k.clone()), n);
                            }
                        })
                        .response
                        .accessible_name("Track to put it on");
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(picked.is_some(), egui::Button::new("Export…"))
                        .on_hover_text("Write it to a file, with its sound files, to send")
                        .clicked()
                    {
                        export = picked;
                    }
                    let mine = picked.is_some_and(|i| lib.list[i].file.is_some());
                    if ui.add_enabled(mine, egui::Button::new("Delete…")).clicked() {
                        lib.deleting = picked;
                    }
                    // ai: file_command.id
                    if ui.button("Import…").clicked() {
                        import = true;
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        // ai: save_preset.kind
                        .add_enabled(lib.onto.is_some(), egui::Button::new("Save track…"))
                        .on_hover_text("Keep the chosen track's whole set-up as a preset")
                        .clicked()
                    {
                        save = Some(PresetKind::Track);
                    }
                    if ui
                        // ai: save_preset.kind
                        .add_enabled(lib.onto.is_some(), egui::Button::new("Save its effects…"))
                        .on_hover_text("Keep only the chosen track's effect chain as a preset")
                        .clicked()
                    {
                        save = Some(PresetKind::Chain);
                    }
                    // ai: view
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                });
            });
        let onto = (lib.onto.as_ref()).and_then(|k| self.tracks.iter().position(|t| &t.key == k));
        let result = if let Some(i) = add {
            let e = lib.list[i].clone();
            Some(
                self.add_preset_track(ctx, &e)
                    .map(|(_, note)| format!("Added “{}” as a new track.{note}", e.preset.name)),
            )
        } else if let (Some(i), Some(t)) = (put, onto) {
            let e = lib.list[i].clone();
            Some(
                self.apply_preset(ctx, t, &e).map(|note| {
                    format!("Put “{}” on {}.{note}", e.preset.name, self.tracks[t].name)
                }),
            )
        } else if let Some(i) = delete {
            let r = self.delete_preset(&lib.list[i]);
            let (list, problems) = all_presets(self.preset_ui.dir.as_deref());
            lib.list = list;
            lib.problems = problems;
            lib.picked = None;
            lib.deleting = None;
            r.err().map(Err)
        } else {
            None
        };
        match result {
            Some(Ok(m)) => {
                self.message = Some(m);
                keep = false;
            }
            Some(Err(m)) => self.message = Some(m),
            None => {}
        }
        if let Some(i) = export {
            let e = lib.list[i].clone();
            if let Some(dest) = rfd::FileDialog::new()
                .set_title("Export preset")
                .set_directory(user_dir())
                .set_file_name(format!("{}.{PRESET_EXTENSION}", e.preset.name))
                .add_filter("Preset", &[PRESET_EXTENSION])
                .save_file()
            {
                self.message = Some(match self.export_preset(&e, &dest) {
                    Ok(()) => format!("Exported “{}” to {}.", e.preset.name, dest.display()),
                    Err(err) => err,
                });
            }
        }
        if keep {
            self.preset_ui.library = Some(lib);
        }
        if import {
            self.pick_preset_to_import();
        }
        if let Some(kind) = save {
            self.ask_save_preset(onto, kind);
        }
    }

    fn save_preset_window(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.preset_ui.saving.take() else {
            return;
        };
        if s.track >= self.tracks.len() {
            return;
        }
        let mut keep = true;
        let mut go = false;
        let name = s.name.trim().to_string();
        let exists = file_name_ok(&name)
            && (self.preset_ui.dir.as_ref())
                .is_some_and(|d| d.join(format!("{name}.{PRESET_EXTENSION}")).exists());
        let tracks: Vec<String> = self.tracks.iter().map(|t| t.name.clone()).collect();
        egui::Window::new("Save as preset")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("From");
                    egui::ComboBox::from_id_salt("preset-from")
                        .selected_text(&tracks[s.track])
                        .show_ui(ui, |ui| {
                            for (i, n) in tracks.iter().enumerate() {
                                // ai: save_preset.track
                                ui.selectable_value(&mut s.track, i, n);
                            }
                        })
                        .response
                        .accessible_name("Track to save");
                });
                // ai: save_preset.kind
                ui.radio_value(&mut s.kind, PresetKind::Track, "The whole track")
                    .on_hover_text("Its instrument, effects, amp, sends and mixer settings");
                // ai: save_preset.kind
                ui.radio_value(&mut s.kind, PresetKind::Chain, "Only its effect chain");
                ui.label(
                    RichText::new(
                        "Clips, notes and automation aren't kept. Sound files the effects \
                         use go in the preset.",
                    )
                    .weak(),
                );
                egui::Grid::new("preset-fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let l = ui.label("Name");
                        let r = ui
                            .add(
                                // ai: save_preset.name
                                egui::TextEdit::singleline(&mut s.name)
                                    .hint_text("e.g. Crunch guitar"),
                            )
                            .labelled_by(l.id);
                        go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.end_row();
                        let l = ui.label("Tags");
                        ui.add(
                            // ai: save_preset.tags
                            egui::TextEdit::singleline(&mut s.tags)
                                .hint_text("e.g. guitar, lesson 3"),
                        )
                        .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label("About");
                        ui.add(
                            // ai: save_preset.about
                            egui::TextEdit::multiline(&mut s.about)
                                .desired_rows(2)
                                .hint_text("What it's for (optional)"),
                        )
                        .labelled_by(l.id);
                        ui.end_row();
                    });
                if exists {
                    ui.label(
                        RichText::new(
                            "You already have a preset with this name: saving replaces it.",
                        )
                        .weak(),
                    );
                }
                ui.horizontal(|ui| {
                    // ai: save_preset
                    go |= ui.button(if exists { "Replace" } else { "Save" }).clicked();
                    // ai: view
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        if go {
            let tags = parse_tags(&s.tags);
            match self.save_preset(ctx, s.track, s.kind, &name, &tags, &s.about) {
                Ok((_, left_out)) => {
                    keep = false;
                    let mut m = format!("Saved the preset “{name}”.");
                    if !left_out.is_empty() {
                        m += &format!(
                            " These files couldn't be found, so aren't in it: {}.",
                            left_out.join(", ")
                        );
                    }
                    self.message = Some(m);
                    if self.preset_ui.library.is_some() {
                        self.open_presets();
                    }
                }
                Err(e) => self.message = Some(e),
            }
        }
        if keep {
            self.preset_ui.saving = Some(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn app(dir: &Path) -> Harness<'static, App> {
        let dir = dir.to_path_buf();
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .allow_missing_glyphs()
            .build_eframe(move |_cc| {
                let mut app = App::new(Vec::new());
                app.preset_ui.dir = Some(dir.join("Presets"));
                app
            });
        h.run();
        h
    }

    fn ctx(h: &Harness<'_, App>) -> egui::Context {
        h.ctx.clone()
    }

    /// A short sound file, to stand for a cabinet's response.
    fn wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..480 {
            w.write_sample(if i == 0 { i16::MAX } else { 0 }).unwrap();
        }
        w.finalize().unwrap();
    }

    /// A guitar track with an amp and a cabinet reading `cab`, sending to
    /// a reverb bus, after a kick track.
    fn guitar(h: &mut Harness<'_, App>, cab: &Path) {
        let ctx = ctx(h);
        let app = h.state_mut();
        let mut p = app.project();
        let mut verb = Track::new("verb".into(), "Reverb".into());
        verb.kind = TrackKind::Bus;
        let mut g = Track::new("g".into(), "Guitar".into());
        g.gain_db = -4.0;
        g.effects = vec![
            fx(Kind::Amp, &[("voice", 2.0), ("gain", 8.0)]),
            daw_project::Effect {
                response: cab.to_string_lossy().into_owned(),
                ..fx(Kind::Cabinet, &[("mix", 80.0)])
            },
        ];
        g.sends.push(daw_project::Send::to("verb".into()));
        p.tracks = vec![verb, g];
        app.proj.doc.set(&p);
        app.apply(p);
        app.sync_doc(&ctx, true);
    }

    #[test]
    fn the_included_presets_name_real_settings_and_load() {
        let list = included();
        assert!(list.len() >= 5);
        for p in &list {
            for e in &p.track.effects {
                let kind = Kind::from_id(&e.builtin).unwrap();
                for (k, _) in &e.params {
                    assert!(kind.params().iter().any(|q| q.key == k), "{}: {k}", p.name);
                }
            }
        }
        let crunch = list.iter().find(|p| p.name == "Crunch guitar").unwrap();
        let amp = &crunch.track.effects[0];
        assert!(amp.params.contains(&("gain".into(), 6.0)));
    }

    #[test]
    fn a_track_preset_is_saved_found_by_tag_and_added_to_another_project() {
        let dir = scratch("track");
        let cab = dir.join("my cab.wav");
        wav(&cab);
        let mut h = app(&dir);
        guitar(&mut h, &cab);
        let ctx = ctx(&h);

        // Saved through its window.
        h.state_mut().ask_save_preset(Some(1), PresetKind::Track);
        h.run();
        assert_eq!(h.state().preset_ui.saving.as_ref().unwrap().name, "Guitar");
        h.state_mut()
            .preset_ui
            .saving
            .as_mut()
            .unwrap()
            .name
            .clear();
        h.run();
        h.get_by_label("Name").focus();
        h.run();
        h.get_by_label("Name").type_text("Big crunch");
        h.run();
        h.get_by_label("Tags").focus();
        h.run();
        h.get_by_label("Tags").type_text("Guitar, crunch");
        h.run();
        h.get_by_label("Save").click();
        h.run();
        let file = dir.join("Presets").join("Big crunch.dawpreset");
        assert!(file.exists(), "{:?}", h.state().message);
        let read = read_preset(&file).unwrap();
        assert_eq!(read.tags, ["guitar", "crunch"]);
        assert_eq!(read.kind, PresetKind::Track);
        assert_eq!(read.track.sends[0].to, "Reverb");
        assert!(
            daw_project::is_asset_name(&read.track.effects[1].response),
            "the cabinet's file is in the preset: {}",
            read.track.effects[1].response
        );

        // A new project, with a reverb bus but nothing else.
        let app = h.state_mut();
        app.new_project();
        let mut p = app.project();
        let mut verb = Track::new("v2".into(), "Reverb".into());
        verb.kind = TrackKind::Bus;
        p.tracks = vec![verb];
        app.proj.doc.set(&p);
        app.apply(p);
        h.run();

        h.state_mut().open_presets();
        h.run();
        // Found by its tag.
        h.get_by_label("Find").focus();
        h.run();
        h.get_by_label("Find").type_text("crunch");
        h.run();
        assert!(
            h.query_by_label("Clean guitar").is_none(),
            "only those that match"
        );
        h.get_by_label("Big crunch").click();
        h.run();
        assert!(
            h.state()
                .preset_ui
                .library
                .as_ref()
                .unwrap()
                .picked
                .is_some()
        );
        h.get_by_label("Add as a new track").click();
        h.run();
        let app = h.state();
        assert!(app.preset_ui.library.is_none(), "{:?}", app.message);
        assert_eq!(app.tracks.len(), 2);
        let t = &app.tracks[1];
        assert_eq!(t.name, "Guitar");
        assert_eq!(t.mix.gain_db, -4.0);
        assert_eq!(t.fx.len(), 2);
        assert_eq!(t.sends[0].to, "v2", "it sends to the bus of that name");
        assert!(t.clips.is_empty());
        // The cabinet's file came along, kept aside until a save.
        let r = t.fx[1].response_ref();
        assert!(Path::new(&r).is_file(), "{r}");
        assert!(r.starts_with(dir.join("Presets").join(FILES).to_str().unwrap()));
        // One step to undo.
        h.state_mut().undo(&ctx);
        assert_eq!(h.state().tracks.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chain_preset_goes_onto_a_track_and_keeps_its_clips() {
        let dir = scratch("chain");
        let mut h = app(&dir);
        let ctx = ctx(&h);
        {
            let app = h.state_mut();
            let mut p = app.project();
            let mut t = Track::new("a".into(), "Vox".into());
            t.effects = vec![fx(Kind::Delay, &[])];
            p.tracks = vec![t];
            app.proj.doc.set(&p);
            app.apply(p);
        }
        let vocal = Entry {
            preset: included().into_iter().find(|p| p.name == "Vocal").unwrap(),
            file: None,
        };
        let note = h.state_mut().apply_preset(&ctx, 0, &vocal).unwrap();
        assert_eq!(note, "");
        let names: Vec<_> = h.state().tracks[0]
            .fx
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(names, ["EQ", "Compressor", "De-esser", "Reverb"]);
        assert_eq!(h.state().tracks[0].name, "Vox");

        // Saved as the user's, it comes back the same.
        let (file, left_out) = h
            .state_mut()
            .save_preset(&ctx, 0, PresetKind::Chain, "My vocal", &[], "")
            .unwrap();
        assert!(left_out.is_empty());
        let read = read_preset(&file).unwrap();
        assert_eq!(read.kind, PresetKind::Chain);
        assert_eq!(read.track.effects.len(), 4);
        assert_eq!(
            read.track.effects[1].params,
            vocal.preset.track.effects[1].params
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_exported_preset_imports_on_another_computer_with_its_files() {
        let dir = scratch("export");
        let cab = dir.join("cab.wav");
        wav(&cab);
        let mut h = app(&dir);
        guitar(&mut h, &cab);
        let ctx = ctx(&h);
        let (file, _) = h
            .state_mut()
            .save_preset(
                &ctx,
                1,
                PresetKind::Chain,
                "Teacher's tone",
                &["lesson".into()],
                "",
            )
            .unwrap();
        let sent = dir.join("sent").join("Teacher's tone.dawpreset");
        std::fs::create_dir_all(sent.parent().unwrap()).unwrap();
        let e = Entry {
            preset: read_preset(&file).unwrap(),
            file: Some(file.clone()),
        };
        h.state_mut().export_preset(&e, &sent).unwrap();
        let sound = std::fs::read(&cab).unwrap();
        std::fs::remove_file(&cab).unwrap();

        // The student's computer: an empty presets folder.
        let student = scratch("student");
        let mut s = app(&student);
        let name = s.state_mut().import_preset(&sent).unwrap();
        assert_eq!(name, "Teacher's tone");
        // Importing again keeps both.
        assert_eq!(
            s.state_mut().import_preset(&sent).unwrap(),
            "Teacher's tone 2"
        );
        let (list, problems) = all_presets(s.state().preset_ui.dir.as_deref());
        assert!(problems.is_empty(), "{problems:?}");
        let mine = list
            .iter()
            .find(|e| e.preset.name == "Teacher's tone")
            .unwrap()
            .clone();
        assert_eq!(mine.preset.tags, ["lesson"]);
        let sctx = s.ctx.clone();
        let (i, _) = s.state_mut().add_preset_track(&sctx, &mine).unwrap();
        let t = &s.state().tracks[i];
        assert_eq!(t.name, "Teacher's tone");
        let r = t.fx[1].response_ref();
        assert!(
            Path::new(&r).is_file(),
            "the cabinet's file came with it: {r}"
        );
        assert_eq!(std::fs::read(&r).unwrap(), sound, "the teacher's sound");
        // Nor is a file that isn't one.
        let junk = dir.join("junk.dawpreset");
        std::fs::write(&junk, b"not a preset").unwrap();
        assert!(read_preset(&junk).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&student);
    }

    #[test]
    fn a_bus_preset_is_refused_on_an_audio_track() {
        let dir = scratch("bus");
        let mut h = app(&dir);
        let ctx = ctx(&h);
        {
            let app = h.state_mut();
            let mut p = app.project();
            p.tracks = vec![Track::new("a".into(), "Guitar".into())];
            app.proj.doc.set(&p);
            app.apply(p);
        }
        let bus = Entry {
            preset: included()
                .into_iter()
                .find(|p| p.name == "Reverb bus")
                .unwrap(),
            file: None,
        };
        assert!(h.state_mut().apply_preset(&ctx, 0, &bus).is_err());
        let (i, _) = h.state_mut().add_preset_track(&ctx, &bus).unwrap();
        assert_eq!(h.state().tracks[i].kind, TrackKind::Bus);
        assert_eq!(h.state().tracks[i].name, "Reverb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End to end, as an assistant would: list, add, save and undo.
    #[test]
    fn an_assistant_lists_adds_and_saves_presets() {
        use serde_json::json;
        let dir = scratch("agent");
        let mut d = crate::agent::driver::Driver::new();
        d.app().preset_ui.dir = Some(dir.join("Presets"));
        let list = d.call("list_presets", json!({})).unwrap();
        let crunch = (list.as_array().unwrap().iter())
            .find(|p| p["name"] == "Crunch guitar")
            .unwrap();
        assert_eq!(crunch["kind"], "chain");
        assert_eq!(crunch["included"], true);

        let added = d
            .call("add_preset", json!({"name": "Crunch guitar"}))
            .unwrap();
        let key = added["track"].as_str().unwrap().to_string();
        let p = d.project();
        assert_eq!(p["tracks"][0]["name"], "Crunch guitar");
        assert_eq!(p["tracks"][0]["effects"].as_array().unwrap().len(), 3);

        // Onto that track, another chain replaces its effects.
        d.call("add_preset", json!({"name": "Vocal", "track": key}))
            .unwrap();
        assert_eq!(
            d.project()["tracks"][0]["effects"]
                .as_array()
                .unwrap()
                .len(),
            4
        );

        d.call(
            "save_preset",
            json!({"track": key, "kind": "chain", "name": "Mine", "tags": ["Vocal", "lesson"]}),
        )
        .unwrap();
        let list = d.call("list_presets", json!({})).unwrap();
        let mine = (list.as_array().unwrap().iter())
            .find(|p| p["name"] == "Mine")
            .unwrap();
        assert_eq!(mine["included"], false);
        assert_eq!(mine["tags"], json!(["vocal", "lesson"]));

        // Each request was one undo step.
        let ctx = d.h.ctx.clone();
        d.app().undo(&ctx);
        assert_eq!(
            d.project()["tracks"][0]["effects"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        d.app().undo(&ctx);
        assert_eq!(d.project()["tracks"].as_array().unwrap().len(), 0);
        assert!(d.call("add_preset", json!({"name": "No such"})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
