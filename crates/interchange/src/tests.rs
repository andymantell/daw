// SPDX-License-Identifier: GPL-3.0-or-later
//! Round trips: our projects out and back in, and files written the way
//! other DAWs write them (the fixtures in `fixtures/`, written for these
//! tests from the format's published schema).

use std::io::Write;
use std::path::{Path, PathBuf};

use daw_project::{
    AutoPoint, Clip, Comment, Controller, Curve, Effect, EffectState, Fade, Lane, Marker,
    MeterChange, Note, NoteClip, PPQ, Project, Send, TempoChange, Track, TrackKind,
};

use crate::{ExportOptions, ImportOptions, Imported, Report, export, export_report, import};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-interchange-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A short stereo WAV file, `seconds` long at `rate`.
fn wav(path: &Path, rate: u32, seconds: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let frames = (seconds * rate as f64) as usize;
    for i in 0..frames {
        let s = ((i as f64 * 0.05).sin() * 8000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(-s).unwrap();
    }
    w.finalize().unwrap();
}

fn no_clap(_: &Effect) -> Option<String> {
    Some("org.example.clap-echo".into())
}

fn export_of(p: &Project, assets: &Path) -> (Vec<u8>, Report) {
    let dir = assets.to_path_buf();
    let path = move |a: &str| {
        let p = Path::new(a);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            dir.join(a)
        }
    };
    export(
        p,
        &ExportOptions {
            title: "Round trip",
            app_version: "0.0.0",
            asset_path: &path,
            clap_id: &no_clap,
        },
    )
    .unwrap()
}

fn import_of(bytes: &[u8], audio: &Path, rate: u32) -> Imported {
    import(
        bytes,
        &ImportOptions {
            rate,
            audio_dir: audio.to_path_buf(),
            base_dir: None,
            has_plugin: &|e: &Effect| e.name != "Missing Synth",
        },
    )
    .unwrap()
}

fn effect(id: &str, builtin: &str, params: &[(&str, f64)]) -> Effect {
    Effect {
        id: id.into(),
        name: builtin.to_uppercase(),
        builtin: builtin.into(),
        params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        ..Effect::default()
    }
}

/// A song using nearly everything the format can hold.
fn song(assets: &Path) -> Project {
    wav(&assets.join("take.wav"), 44_100, 3.0);
    let mut guitar = Track::new("gtr".into(), "Guitar".into());
    guitar.gain_db = -3.5;
    guitar.pan = -0.25;
    guitar.solo = true;
    guitar.output = Some("group".into());
    guitar.sends = vec![Send {
        gain_db: -12.0,
        pre_fader: true,
        ..Send::to("verb".into())
    }];
    guitar.clips = vec![
        Clip {
            id: "c1".into(),
            asset: "take.wav".into(),
            name: "take.wav".into(),
            start: 96_000,
            offset: 12_000,
            len: 48_000,
            fade_in: Fade {
                len: 2_400,
                curve: Curve::Linear,
            },
            fade_out: Fade {
                len: 4_800,
                curve: Curve::Linear,
            },
            ..Clip::default()
        },
        Clip {
            id: "c2".into(),
            asset: "take.wav".into(),
            name: "take.wav".into(),
            start: 400_000,
            offset: 0,
            len: 30_000,
            muted: true,
            seq: 1,
            ..Clip::default()
        },
    ];
    guitar.effects = vec![
        effect(
            "eq1",
            "eq",
            &[("low_cut", 80.0), ("mid1_gain", 3.0), ("mid1_q", 2.0)],
        ),
        effect(
            "comp1",
            "compressor",
            &[("ratio", 4.0), ("threshold", -20.0), ("knee", 2.0)],
        ),
        effect("verb1", "reverb", &[]),
        Effect {
            id: "vst".into(),
            name: "Fuzz".into(),
            class: (1..=16).collect(),
            bundle: "/plugins/Fuzz.vst3".into(),
            bypass: true,
            state: Some(EffectState {
                component: vec![1, 2, 3, 4],
                controller: Some(vec![9, 8]),
            }),
            ..Effect::default()
        },
        Effect {
            id: "clap".into(),
            name: "Echo".into(),
            class: crate::devices::clap_class("org.example.clap-echo"),
            bundle: "/plugins/Echo.clap".into(),
            state: Some(EffectState {
                component: vec![7; 40],
                controller: None,
            }),
            ..Effect::default()
        },
    ];
    let lane = |param: &str, pts: &[(i64, f64)]| Lane {
        param: param.into(),
        points: pts
            .iter()
            .map(|&(at, value)| AutoPoint { at, value })
            .collect(),
    };
    guitar.automation = vec![
        lane("gain", &[(0, -6.0), (48_000, 0.0), (300_001, -20.0)]),
        lane("pan", &[(10, -1.0), (90_000, 0.5)]),
        lane("send:verb", &[(0, -12.0), (100_000, -3.0)]),
        lane("fx:comp1:threshold", &[(0, -30.0), (77_777, -10.0)]),
        lane("fx:eq1:mid1_gain", &[(5, 0.0), (6_000, 6.0)]),
        lane("fx:verb1:mix", &[(0, 20.0), (50_000, 40.0)]),
        lane("plugin:vst:7", &[(0, 0.25), (1_000, 0.75)]),
    ];
    let mut keys = Track::new("keys".into(), "Keys".into());
    keys.kind = TrackKind::Instrument;
    keys.instrument = Some(effect("syn", "poly-synth", &[("cutoff", 1234.0)]));
    let mut clip = NoteClip::new("n1".into(), "Riff".into(), PPQ * 4, PPQ * 8);
    clip.offset = PPQ / 2;
    let mut a = Note::new(PPQ / 2, PPQ, 60, 100);
    a.channel = 2;
    a.expression_mut(Note::PITCH).put(0, 0.0);
    a.expression_mut(Note::PITCH).put(PPQ / 2, 1.5);
    a.expression_mut(Note::PRESSURE).put(10, 0.4);
    let b = Note::new(PPQ * 3 + 7, 333, 64, 37);
    clip.notes = vec![a, b];
    clip.controller_mut(Controller::Cc(1)).put(0, 0.0);
    clip.controller_mut(Controller::Cc(1)).put(PPQ, 127.0);
    clip.controller_mut(Controller::PitchBend)
        .put(PPQ * 2, -8192.0);
    clip.controller_mut(Controller::PitchBend)
        .put(PPQ * 3, 4096.0);
    keys.note_clips = vec![clip];
    keys.output = None;
    keys.parent = Some("fold".into());
    let mut fold = Track::new("fold".into(), "Band".into());
    fold.kind = TrackKind::Folder;
    fold.gain_db = 1.0;
    let mut group = Track::new("group".into(), "Guitars".into());
    group.kind = TrackKind::Bus;
    let mut verb = Track::new("verb".into(), "Reverb".into());
    verb.kind = TrackKind::Bus;
    verb.mute = true;
    Project {
        rate: 48_000,
        tempo_bpm: 100.0,
        beats_per_bar: 4,
        beat_unit: 4,
        tempo_changes: vec![
            TempoChange {
                id: "t1".into(),
                bar: 3,
                beat: 1,
                bpm: 130.0,
                glide: false,
            },
            TempoChange {
                id: "t2".into(),
                bar: 7,
                beat: 2,
                bpm: 90.5,
                glide: true,
            },
        ],
        meter_changes: vec![MeterChange {
            id: "m1".into(),
            bar: 5,
            beats: 3,
            unit: 4,
        }],
        markers: vec![
            Marker {
                id: "a".into(),
                bar: 1,
                name: "Intro".into(),
            },
            Marker {
                id: "b".into(),
                bar: 6,
                name: "Verse & chorus".into(),
            },
        ],
        tracks: vec![guitar, fold, keys, group, verb],
        comments: vec![Comment {
            id: "x".into(),
            author: String::new(),
            parent: None,
            track: None,
            start: 0,
            end: None,
            text: "Nice".into(),
            resolved: false,
            created: 0,
        }],
        ..Project::default()
    }
}

fn by_name<'p>(p: &'p Project, name: &str) -> &'p Track {
    p.tracks.iter().find(|t| t.name == name).unwrap()
}

fn id_of(p: &Project, name: &str) -> String {
    by_name(p, name).id.clone()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_project_goes_out_and_comes_back() {
    let dir = scratch("round");
    let src = song(&dir);
    let (bytes, report) = export_of(&src, &dir);
    let back = import_of(&bytes, &dir.join("in"), 48_000);
    assert_eq!(back.title.as_deref(), Some("Round trip"));
    assert!(back.application.starts_with("DAW"));
    let p = &back.project;

    // Time.
    assert_eq!(p.tempo_bpm, 100.0);
    let tempos: Vec<_> = (p.tempo_changes.iter())
        .map(|t| (t.bar, t.beat, t.bpm, t.glide))
        .collect();
    assert_eq!(tempos, [(3, 1, 130.0, false), (7, 2, 90.5, true)]);
    let meters: Vec<_> = (p.meter_changes.iter())
        .map(|m| (m.bar, m.beats, m.unit))
        .collect();
    assert_eq!(meters, [(5, 3, 4)]);
    let markers: Vec<_> = p.markers.iter().map(|m| (m.bar, m.name.as_str())).collect();
    assert_eq!(markers, [(1, "Intro"), (6, "Verse & chorus")]);

    // Tracks, kinds and routing.
    let names: Vec<_> = p.tracks.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        names,
        [
            ("Guitar", TrackKind::Audio),
            ("Band", TrackKind::Folder),
            ("Keys", TrackKind::Instrument),
            ("Guitars", TrackKind::Bus),
            ("Reverb", TrackKind::Bus),
        ]
    );
    let g = by_name(p, "Guitar");
    assert!(near(g.gain_db, -3.5));
    assert!(near(g.pan, -0.25));
    assert!(g.solo && !g.mute);
    assert_eq!(g.output, Some(id_of(p, "Guitars")));
    assert_eq!(g.sends.len(), 1);
    assert_eq!(g.sends[0].to, id_of(p, "Reverb"));
    assert!(near(g.sends[0].gain_db, -12.0) && g.sends[0].pre_fader);
    assert!(by_name(p, "Reverb").mute);
    let keys = by_name(p, "Keys");
    assert_eq!(keys.parent, Some(id_of(p, "Band")));
    assert_eq!(keys.output, None);
    assert!(near(by_name(p, "Band").gain_db, 1.0));

    // Audio clips, to the frame.
    let clips: Vec<_> = (g.clips.iter())
        .map(|c| {
            (
                c.start,
                c.offset,
                c.len,
                c.fade_in.len,
                c.fade_out.len,
                c.muted,
            )
        })
        .collect();
    assert_eq!(
        clips,
        [
            (96_000, 12_000, 48_000, 2_400, 4_800, false),
            (400_000, 0, 30_000, 0, 0, true)
        ]
    );
    assert!(Path::new(&g.clips[0].asset).is_file());
    assert_eq!(g.clips[0].asset, g.clips[1].asset, "one file for both");

    // Notes, with their expression and controller lines.
    let nc = &keys.note_clips[0];
    assert_eq!(
        (nc.name.as_str(), nc.start, nc.len),
        ("Riff", PPQ * 4, PPQ * 8)
    );
    assert_eq!(nc.offset, PPQ / 2);
    let notes: Vec<_> = (nc.notes.iter())
        .map(|n| (n.start, n.len, n.pitch, n.velocity, n.channel))
        .collect();
    assert_eq!(
        notes,
        [(PPQ / 2, PPQ, 60, 100, 2), (PPQ * 3 + 7, 333, 64, 37, 0)]
    );
    let pitch = nc.notes[0].expression(Note::PITCH).unwrap();
    assert_eq!(
        pitch.points,
        [
            AutoPoint { at: 0, value: 0.0 },
            AutoPoint {
                at: PPQ / 2,
                value: 1.5
            }
        ]
    );
    assert!(nc.notes[0].expression(Note::PRESSURE).is_some());
    let cc = nc.controllers.iter().find(|l| l.param == "cc:1").unwrap();
    let cc: Vec<_> = cc.points.iter().map(|p| (p.at, p.value)).collect();
    assert_eq!(cc.len(), 2);
    assert_eq!(cc[1].0, PPQ);
    assert!(near(cc[1].1, 127.0));
    let bend = nc.controllers.iter().find(|l| l.param == "bend").unwrap();
    assert!(near(bend.points[0].value, -8192.0));
    assert!(near(bend.points[1].value, 4096.0));

    // Effects: ours whole, plugins with their settings.
    let fx: Vec<_> = g.effects.iter().map(|e| e.builtin.as_str()).collect();
    assert_eq!(fx, ["eq", "compressor", "reverb", "", ""]);
    let src_g = by_name(&src, "Guitar");
    for (a, b) in src_g.effects.iter().zip(&g.effects).take(3) {
        let sorted = |e: &Effect| {
            let mut v = e.params.clone();
            v.sort_by(|x, y| x.0.cmp(&y.0));
            v
        };
        assert_eq!(sorted(a), sorted(b), "{}", a.builtin);
    }
    let vst = &g.effects[3];
    assert_eq!(vst.class, src_g.effects[3].class);
    assert_eq!(vst.state, src_g.effects[3].state);
    assert!(vst.bypass);
    let clap = &g.effects[4];
    assert_eq!(clap.class, src_g.effects[4].class);
    assert_eq!(clap.state, src_g.effects[4].state);
    assert!(daw_plugins::clap::is_clap(Path::new(&clap.bundle)));
    let synth = keys.instrument.as_ref().unwrap();
    assert_eq!(synth.builtin, "poly-synth");
    assert_eq!(synth.params, vec![("cutoff".to_string(), 1234.0)]);

    // Automation, to the frame, on the right settings.
    for l in &src_g.automation {
        let param = match l.param.split(':').collect::<Vec<_>>()[..] {
            ["send", _] => format!("send:{}", id_of(p, "Reverb")),
            ["fx", e, k] => {
                let i = src_g.effects.iter().position(|x| x.id == e).unwrap();
                format!("fx:{}:{k}", g.effects[i].id)
            }
            ["plugin", e, k] => {
                let i = src_g.effects.iter().position(|x| x.id == e).unwrap();
                format!("plugin:{}:{k}", g.effects[i].id)
            }
            _ => l.param.clone(),
        };
        let got = g
            .automation
            .iter()
            .find(|x| x.param == param)
            .unwrap_or_else(|| panic!("no lane {param}"));
        assert_eq!(got.points.len(), l.points.len(), "{param}");
        for (a, b) in l.points.iter().zip(&got.points) {
            assert_eq!(a.at, b.at, "{param}");
            assert!(
                (a.value - b.value).abs() < 1e-6,
                "{param}: {} {}",
                a.value,
                b.value
            );
        }
    }

    // What didn't go is said, going out.
    assert!(report.mentions("Comments"));
    let text = report.text();
    assert!(text.contains("knee"), "{text}");
    assert!(text.contains("REVERB"), "{text}");
    assert!(text.contains("Fades go with their lengths"), "{text}");
    // Coming back, nothing more is lost.
    assert!(back.report.is_empty(), "{}", back.report.text());
}

#[test]
fn the_report_comes_before_writing_and_says_the_same() {
    let dir = scratch("report");
    let p = song(&dir);
    let path = |a: &str| dir.join(a);
    let o = ExportOptions {
        title: "",
        app_version: "1",
        asset_path: &path,
        clap_id: &|_| None,
    };
    let early = export_report(&p, &o);
    let (_, late) = export(&p, &o).unwrap();
    assert_eq!(early, late);
    assert!(
        early
            .text()
            .contains("couldn't be found here to read its id")
    );
}

#[test]
fn audio_that_cant_be_read_is_reported_not_written() {
    let dir = scratch("missing");
    let mut p = song(&dir);
    p.tracks[0].clips[0].asset = "gone.wav".into();
    p.tracks[0].clips[1].asset = "gone.wav".into();
    let (bytes, report) = export_of(&p, &dir);
    assert!(report.text().contains("couldn't be read"));
    let back = import_of(&bytes, &dir.join("in"), 48_000);
    assert!(by_name(&back.project, "Guitar").clips.is_empty());
}

#[test]
fn a_project_at_another_rate_lands_on_the_same_times() {
    let dir = scratch("rate");
    let src = song(&dir);
    let (bytes, _) = export_of(&src, &dir);
    let back = import_of(&bytes, &dir.join("in"), 44_100);
    let g = by_name(&back.project, "Guitar");
    // 96 000 frames at 48 kHz is 2 s: 88 200 at 44.1 kHz.
    assert_eq!(g.clips[0].start, 88_200);
    assert_eq!(g.clips[0].offset, 11_025);
    assert_eq!(g.clips[0].len, 44_100);
    // Notes are in ticks either way.
    assert_eq!(by_name(&back.project, "Keys").note_clips[0].start, PPQ * 4);
}

/// A `.dawproject` file holding `xml` as its project and `files` beside.
fn package(xml: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    z.start_file("project.xml", o).unwrap();
    z.write_all(xml.as_bytes()).unwrap();
    for (name, data) in files {
        z.start_file(*name, o).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn loop_wav(dir: &Path) -> Vec<u8> {
    let p = dir.join("loop.wav");
    wav(&p, 48_000, 3.0);
    std::fs::read(p).unwrap()
}

#[test]
fn clips_inside_clips_come_in_flattened_and_cut() {
    let dir = scratch("nested");
    let bytes = package(
        &fixture("nested-clips.xml"),
        &[("audio/loop.wav", loop_wav(&dir))],
    );
    let back = import_of(&bytes, &dir.join("in"), 48_000);
    let p = &back.project;
    assert_eq!(p.tempo_bpm, 120.0);
    let drums = by_name(p, "Drums");
    // At 120 bpm a beat is 24 000 frames. The outer clip starts on beat
    // 4 and plays 4 beats from beat 1 of its content, where the inner
    // clip starts at content beat 0 and plays its audio from 0.5 s: so
    // the audio heard first is 0.5 s + 1 beat (0.5 s) in.
    assert_eq!(drums.clips.len(), 1);
    let c = &drums.clips[0];
    assert_eq!((c.start, c.offset, c.len), (96_000, 48_000, 72_000));
    assert_eq!(c.name, "Beat");
    assert_eq!(c.fade_in.len, 2_400);
    assert!(Path::new(&c.asset).is_file());
}

#[test]
fn a_looped_clip_comes_in_pass_by_pass() {
    let dir = scratch("looped");
    let bytes = package(&fixture("looped-notes.xml"), &[]);
    let back = import_of(&bytes, &dir.join("in"), 48_000);
    let bass = by_name(&back.project, "Bass");
    assert_eq!(bass.kind, TrackKind::Instrument);
    // A 2-beat loop played for 5 beats: three passes, the last cut short.
    let clips: Vec<_> = (bass.note_clips.iter())
        .map(|c| (c.start, c.len, c.notes.len()))
        .collect();
    assert_eq!(
        clips,
        [(0, PPQ * 2, 2), (PPQ * 2, PPQ * 2, 2), (PPQ * 4, PPQ, 1)]
    );
    let starts: Vec<_> = (bass.note_clips.iter())
        .flat_map(|c| c.notes.iter().map(move |n| c.start + n.start))
        .collect();
    assert_eq!(starts, [0, PPQ, PPQ * 2, PPQ * 3, PPQ * 4]);
    assert!(back.report.text().contains("one clip per pass"));
}

#[test]
fn a_file_from_elsewhere_maps_what_it_can_and_says_what_it_cant() {
    let dir = scratch("elsewhere");
    let preset = daw_plugins::presets::PresetFile {
        class: "0123456789ABCDEF0123456789ABCDEF".into(),
        state: daw_plugins::PluginState {
            component: vec![5, 6, 7],
            controller: None,
        },
    }
    .encode();
    let bytes = package(
        &fixture("elsewhere.xml"),
        &[
            ("audio/loop.wav", loop_wav(&dir)),
            ("plugins/amp.vstpreset", preset),
        ],
    );
    let back = import_of(&bytes, &dir.join("in"), 48_000);
    let p = &back.project;
    let text = back.report.text();

    // The master's level is the app's own; the tracks come in.
    let names: Vec<_> = p.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Lead", "Synth", "FX return"]);
    let lead = by_name(p, "Lead");
    // Linear 0.5 is about −6 dB; pan 0.75 of 0..1 is +0.5.
    assert!((lead.gain_db - -6.0206).abs() < 1e-3);
    assert!(near(lead.pan, 0.5));
    assert_eq!(lead.sends[0].to, id_of(p, "FX return"));
    assert_eq!(by_name(p, "FX return").kind, TrackKind::Bus);

    // Generic devices become ours.
    let comp = &lead.effects[0];
    assert_eq!(comp.builtin, "compressor");
    let v = |e: &Effect, k: &str| crate::devices::value(e, k);
    assert!(near(v(comp, "threshold"), -24.0));
    assert!(near(v(comp, "ratio"), 4.0));
    assert!(near(v(comp, "attack"), 5.0));
    assert!(near(v(comp, "release"), 200.0));
    assert!(near(v(comp, "makeup"), 3.0));
    let eq = &lead.effects[1];
    assert_eq!(eq.builtin, "eq");
    assert!(near(v(eq, "low_cut"), 90.0));
    assert!(near(v(eq, "mid1_freq"), 800.0));
    assert!(near(v(eq, "mid1_gain"), -4.0));
    // A disabled high cut is off.
    assert!(near(v(eq, "high_cut"), 20_000.0));
    // The plugin, with its settings, flagged as missing here.
    let amp = &lead.effects[2];
    assert_eq!(amp.name, "Missing Synth");
    assert_eq!(amp.state.as_ref().unwrap().component, [5, 6, 7]);
    assert!(text.contains("isn't on this computer"), "{text}");
    // A VST 2 plugin and a device of the other DAW's own are left out,
    // and said so.
    assert_eq!(lead.effects.len(), 3);
    assert!(text.contains("VST 2 plugin \"Old Chorus\""), "{text}");
    assert!(text.contains("\"Spectral Thing\" is a device of the other DAW's own"));
    assert!(
        text.contains("Clip launcher") || text.contains("Scenes"),
        "{text}"
    );

    // Held automation steps stay steps.
    let vol = lead.automation.iter().find(|l| l.param == "gain").unwrap();
    let at: Vec<_> = vol.points.iter().map(|p| p.at).collect();
    assert_eq!(at, [0, 47_999, 48_000]);
    assert!(near(vol.points[1].value, 0.0));
    assert!((vol.points[2].value - -6.0206).abs() < 1e-3);
    // Compressor threshold automation, through the generic setting.
    let thr = lead
        .automation
        .iter()
        .find(|l| l.param.ends_with(":threshold"))
        .unwrap();
    assert!(near(thr.points[0].value, -24.0) && near(thr.points[1].value, -12.0));

    // Warped audio plays at its own speed, said so.
    let a = &lead.clips[0];
    assert_eq!((a.start, a.offset), (0, 0));
    assert!(text.contains("plays at its own speed"), "{text}");

    // Synth: notes straight on the track, a CC line in a clip, and a
    // marker off the bar line moved to its bar.
    let synth = by_name(p, "Synth");
    assert_eq!(synth.kind, TrackKind::Instrument);
    assert_eq!(
        synth
            .note_clips
            .iter()
            .map(|c| c.notes.len())
            .sum::<usize>(),
        3
    );
    assert_eq!(back.project.markers[0].bar, 2);
    assert!(text.contains("Markers"), "{text}");
    // The tempo change on beat 8 is bar 3; the 3/4 change on beat 12
    // is a bar line too.
    let tempos: Vec<_> = (p.tempo_changes.iter())
        .map(|t| (t.bar, t.beat, t.bpm))
        .collect();
    assert_eq!(tempos, [(3, 1, 90.0)]);
    assert_eq!(p.meter_changes[0].bar, 4);
}

#[test]
fn damaged_files_are_refused_with_a_reason() {
    let dir = scratch("bad");
    let o = ImportOptions {
        rate: 48_000,
        audio_dir: dir.clone(),
        base_dir: None,
        has_plugin: &|_| true,
    };
    assert!(
        import(b"not a zip", &o)
            .unwrap_err()
            .contains("isn't a project file")
    );
    let empty = package("", &[]);
    assert!(import(&empty, &o).is_err());
    let other = package("<Song/>", &[]);
    assert!(
        import(&other, &o)
            .unwrap_err()
            .contains("doesn't hold a project")
    );
}
