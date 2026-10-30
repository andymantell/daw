// SPDX-License-Identifier: GPL-3.0-or-later
//! Exporting to WAV (roadmap 1.8), FLAC, MP3 and Opus (roadmap 3.6).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use daw_engine::export::{Format, write, write_wav};
use daw_engine::graph::{AudioData, ClipSpec, Session, TrackSpec};
use daw_engine::time::{Frames, Tempo, TempoMap, Ticks, TimeSignature};

const RATE: u32 = 44_100;

fn session(samples: Vec<f32>) -> Session {
    let mut s = session_at(RATE, samples);
    s.tracks[0].mix.pan = -1.0;
    s
}

fn session_at(rate: u32, samples: Vec<f32>) -> Session {
    let mut s = Session::new(TempoMap::new(rate, Tempo::bpm(120), TimeSignature::COMMON));
    s.tracks.push(TrackSpec::new(vec![ClipSpec::new(
        Ticks(0),
        Arc::new(AudioData {
            channels: vec![samples],
            rate,
        }),
    )]));
    s
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("daw-export-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn writes_the_mix_as_24_bit_wav() {
    let samples: Vec<f32> = (0..10_000)
        .map(|i| ((i % 200) as f32 - 100.0) / 200.0)
        .collect();
    let s = session(samples.clone());
    let path = dir("mix").join("mix.wav");
    let done = AtomicU64::new(0);
    let w = write_wav(
        s.compile(512).unwrap(),
        Frames(0),
        10_000,
        &path,
        &done,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!w.clipped());
    assert_eq!(done.load(Ordering::Relaxed), 10_000);

    let mut r = hound::WavReader::open(&path).unwrap();
    let spec = r.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (2, RATE, 24)
    );
    let got: Vec<i32> = r.samples::<i32>().map(Result::unwrap).collect();
    assert_eq!(got.len(), 20_000);
    for (i, x) in samples.iter().enumerate() {
        let want = (x * 8_388_607.0).round() as i32;
        assert!((got[2 * i] - want).abs() <= 1, "frame {i}");
        assert_eq!(got[2 * i + 1], 0, "hard left leaves right silent");
    }
    assert!(!path.with_extension("wav.part").exists());
}

#[test]
fn reports_clipping_and_cancels_cleanly() {
    let s = session(vec![1.5; 1000]);
    let d = dir("clip");
    let w = write_wav(
        s.compile(512).unwrap(),
        Frames(0),
        1000,
        &d.join("loud.wav"),
        &AtomicU64::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(w.clipped());
    assert!((w.peak - 1.5).abs() < 1e-6);

    let s = session(vec![0.1; 100_000]);
    let path = d.join("cancelled.wav");
    let err = write_wav(
        s.compile(512).unwrap(),
        Frames(0),
        100_000,
        &path,
        &AtomicU64::new(0),
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert_eq!(err, "cancelled");
    assert!(!path.exists());
    assert!(!path.with_extension("wav.part").exists());
}

/// A second of a 440 Hz tone at half scale, panned centre.
fn tone(rate: u32) -> Session {
    let samples: Vec<f32> = (0..rate)
        .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin())
        .collect();
    session_at(rate, samples)
}

fn export(s: &Session, frames: usize, path: &std::path::Path, format: Format) {
    let done = AtomicU64::new(0);
    write(
        s.compile(512).unwrap(),
        Frames(0),
        frames,
        path,
        format,
        &done,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(done.load(Ordering::Relaxed), frames as u64);
    assert!(path.exists());
    assert!(
        !path
            .with_extension(format!("{}.part", format.extension()))
            .exists()
    );
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn flac_holds_exactly_what_wav_does() {
    let s = tone(RATE);
    let d = dir("flac");
    // Not a whole number of blocks, so the last frame is short.
    let frames = 30_001;
    export(&s, frames, &d.join("mix.wav"), Format::Wav);
    export(&s, frames, &d.join("mix.flac"), Format::Flac);
    let wav = daw_engine::import::read(&d.join("mix.wav")).unwrap();
    let flac = daw_engine::import::read(&d.join("mix.flac")).unwrap();
    assert_eq!((flac.channels, flac.rate), (2, RATE));
    assert_eq!(flac.samples.len(), 2 * frames);
    assert_eq!(flac.samples, wav.samples, "lossless");
    let size = |p: &str| std::fs::metadata(d.join(p)).unwrap().len();
    assert!(size("mix.flac") < size("mix.wav"));
}

#[test]
fn mp3_sounds_like_the_mix() {
    let s = tone(RATE);
    let d = dir("mp3");
    let frames = RATE as usize;
    export(&s, frames, &d.join("mix.wav"), Format::Wav);
    export(&s, frames, &d.join("mix.mp3"), Format::Mp3(320));
    let wav = daw_engine::import::read(&d.join("mix.wav")).unwrap();
    let mp3 = daw_engine::import::read(&d.join("mix.mp3")).unwrap();
    assert_eq!((mp3.channels, mp3.rate), (2, RATE));
    // Within an MP3 frame or two of the length, depending on whether
    // the decoder takes the encoder's delay off.
    let got = mp3.samples.len() / 2;
    assert!(
        got >= frames - 1152 && got <= frames + 3 * 1152,
        "{got} frames"
    );
    // A steady stretch from the middle is as loud as the mix.
    let mid = &mp3.samples[2 * 10_000..2 * 30_000];
    let want = rms(&wav.samples[2 * 10_000..2 * 30_000]);
    let db = 20.0 * (rms(mid) / want).log10();
    assert!(db.abs() < 0.2, "{db} dB");
    // Sized for 320 kbit/s: about 40 kB for the second.
    let size = std::fs::metadata(d.join("mix.mp3")).unwrap().len();
    assert!((36_000..46_000).contains(&size), "{size} bytes");
}

/// Decodes an Ogg Opus file: its pre-skip, and the audio after it, cut
/// to the end the last page gives.
fn read_opus(path: &std::path::Path) -> (u32, Vec<f32>) {
    let mut r = ogg::PacketReader::new(std::fs::File::open(path).unwrap());
    let head = r.read_packet_expected().unwrap();
    assert_eq!(&head.data[..8], b"OpusHead");
    assert_eq!(head.data[9], 2, "stereo");
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
    let input_rate = u32::from_le_bytes(head.data[12..16].try_into().unwrap());
    let tags = r.read_packet_expected().unwrap();
    assert_eq!(&tags.data[..8], b"OpusTags");
    let mut dec = opus::Decoder::new(48_000, opus::Channels::Stereo).unwrap();
    let mut out = Vec::new();
    let mut end = 0;
    let mut buf = vec![0.0; 2 * 5760];
    while let Some(p) = r.read_packet().unwrap() {
        let n = dec.decode_float(&p.data, &mut buf, false).unwrap();
        out.extend_from_slice(&buf[..2 * n]);
        if p.last_in_stream() {
            end = p.absgp_page() as usize;
        }
    }
    assert!(end > 0, "the stream is marked as ended");
    out.truncate(2 * end);
    (input_rate, out.split_off(2 * pre_skip))
}

#[test]
fn opus_sounds_like_the_mix_at_48k_and_at_other_rates() {
    for rate in [48_000, RATE] {
        let s = tone(rate);
        let d = dir(&format!("opus-{rate}"));
        let frames = rate as usize;
        export(&s, frames, &d.join("mix.wav"), Format::Wav);
        export(&s, frames, &d.join("mix.opus"), Format::Opus(160));
        let wav = daw_engine::import::read(&d.join("mix.wav")).unwrap();
        let (input_rate, opus) = read_opus(&d.join("mix.opus"));
        assert_eq!(input_rate, rate);
        // Exactly the mix's length, at 48 kHz.
        assert_eq!(
            opus.len() / 2,
            frames * 48_000 / rate as usize,
            "at {rate} Hz"
        );
        let at = |x: usize| 2 * x * 48_000 / rate as usize;
        let mid = &opus[at(10_000)..at(30_000)];
        let want = rms(&wav.samples[2 * 10_000..2 * 30_000]);
        let db = 20.0 * (rms(mid) / want).log10();
        assert!(db.abs() < 0.3, "{db} dB at {rate} Hz");
        // Lined up with the mix: the tone starts at the start.
        let first = opus.iter().position(|x| x.abs() > 0.05).unwrap() / 2;
        assert!(first < 48, "starts {first} frames late at {rate} Hz");
    }
}

#[test]
fn an_unoffered_bit_rate_is_refused_and_leaves_no_file() {
    let s = tone(RATE);
    let path = dir("bad-rate").join("mix.mp3");
    let err = write(
        s.compile(512).unwrap(),
        Frames(0),
        1000,
        &path,
        Format::Mp3(100),
        &AtomicU64::new(0),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(err.contains("100 kbit/s"), "{err}");
    assert!(!path.exists());
    assert!(!path.with_extension("mp3.part").exists());
}
