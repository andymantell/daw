// SPDX-License-Identifier: GPL-3.0-or-later
//! Reads the words in a recording of a voice, with when each one starts
//! and ends (wish list, song test 3): an assistant, or a person, can then
//! pick a chop by its words rather than by pitch and timing alone.
//!
//! The work is done on this computer by a speech model (decision 0015),
//! which isn't shipped with the app: it is downloaded once, the first
//! time it is needed, and checked against the size and SHA-256 listed in
//! [`MODELS`]. The model's code and weights are MIT licensed; the
//! library that runs it is MIT (its Rust binding is Unlicense), all of
//! which a GPL-3.0-or-later program may use.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// The sample rate the model listens at.
pub const RATE: u32 = 16_000;

/// A speech model the app can download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// What it is called in the app and to assistants: "small".
    pub name: &'static str,
    /// Its file's name, in the models folder.
    pub file: &'static str,
    /// Where it is downloaded from.
    pub url: &'static str,
    /// The file's size in bytes.
    pub bytes: u64,
    /// The file's SHA-256, in hex.
    pub sha256: &'static str,
    /// What it is good for, for the download window.
    pub about: &'static str,
}

/// The files are pinned to one revision of the model page, so the size
/// and hash below stay the file's.
macro_rules! url {
    ($file:literal) => {
        concat!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/",
            "5359861c739e955e79d9a303bcbc70fb988958b1/",
            $file
        )
    };
}

/// The models offered, smallest first. Each reads many languages and
/// finds which one it is hearing. These are the compressed (quantised)
/// files from the model's official page.
pub const MODELS: &[Model] = &[
    Model {
        name: "base",
        file: "ggml-base-q5_1.bin",
        url: url!("ggml-base-q5_1.bin"),
        bytes: 59_707_625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
        about: "smallest and quickest; mishears more sung words",
    },
    Model {
        name: "small",
        file: "ggml-small-q5_1.bin",
        url: url!("ggml-small-q5_1.bin"),
        bytes: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        about: "good on singing, a few seconds a minute of audio",
    },
    Model {
        name: "medium",
        file: "ggml-medium-q5_0.bin",
        url: url!("ggml-medium-q5_0.bin"),
        bytes: 539_212_467,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        about: "the most accurate, and slower",
    },
];

/// The model used unless another is chosen.
pub const DEFAULT_MODEL: &str = "small";

/// The model called `name`.
pub fn model(name: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

/// One word, with when it is sung, in seconds from the start of the
/// audio given.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    /// As heard, without the punctuation around it.
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// How sure the model is of the word, 0 to 1: the least sure of
    /// its pieces.
    pub sure: f32,
}

/// One line as the model heard it: a phrase or sentence.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The line's text, with punctuation.
    pub text: String,
    /// Its words, in order.
    pub words: Vec<Word>,
}

/// What was heard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// The language heard, as a two-letter code ("en").
    pub language: String,
    pub lines: Vec<Line>,
}

impl Transcript {
    /// Every word, in order.
    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.lines.iter().flat_map(|l| &l.words)
    }
}

/// One piece of a word as the model gives it: its text (starting with a
/// space when it begins a word), its start and end in hundredths of a
/// second, and how sure the model is of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub text: String,
    pub t0: i64,
    pub t1: i64,
    pub p: f32,
}

/// Whether `c` is part of a word rather than punctuation around it.
fn wordy(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// Joins pieces into words: a piece starting with a space starts a word;
/// the rest go on the word before (a word's time runs from its first
/// letters to its last). Punctuation is trimmed off, and what
/// isn't a word at all (a lone comma, "♪", "[Music]") is dropped.
pub fn words_from_pieces(pieces: &[Piece], seconds: f64) -> Vec<Word> {
    let mut words: Vec<(String, i64, i64, f32)> = Vec::new();
    for p in pieces {
        let letters = |t: &str| t.chars().any(char::is_alphanumeric);
        let starts = p.text.starts_with(' ') || words.is_empty();
        match words.last_mut() {
            // A word's next piece, or punctuation after it.
            Some(w) if !starts => {
                if letters(&p.text) {
                    if !letters(&w.0) {
                        // Its first letters: " '" then "Cause".
                        (w.1, w.3) = (p.t0, p.p);
                    }
                    w.2 = w.2.max(p.t1);
                    w.3 = w.3.min(p.p);
                }
                w.0.push_str(&p.text);
            }
            _ => words.push((p.text.trim_start().to_string(), p.t0, p.t1, p.p)),
        }
    }
    words
        .into_iter()
        .filter(|w| !(w.0.starts_with('[') || w.0.starts_with('(')))
        .filter_map(|(text, t0, t1, p)| {
            let text = text
                .trim_matches(|c: char| !wordy(c))
                .trim_matches('\'')
                .to_string();
            if !text.chars().any(char::is_alphanumeric) {
                return None;
            }
            let start = (t0 as f64 / 100.0).clamp(0.0, seconds);
            let end = (t1 as f64 / 100.0).clamp(start, seconds);
            Some(Word {
                text,
                start,
                end,
                sure: p,
            })
        })
        .collect()
}

/// Reads the words in `samples`, mono at [`RATE`], with the model file
/// at `model`. `progress` is told how far it has got, 0 to 100. Takes a
/// while: run it off the UI thread.
pub fn transcribe(
    model: &Path,
    samples: &[f32],
    progress: impl FnMut(i32) + 'static,
) -> Result<Transcript, String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
    // Keeps the library's own log lines out of the app's output.
    whisper_rs::install_logging_hooks();
    if samples.is_empty() {
        return Ok(Transcript::default());
    }
    let path = model.to_str().ok_or("The model's path isn't text.")?;
    let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|e| format!("The speech model couldn't be opened: {e}"))?;
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("The speech model couldn't start: {e}"))?;
    let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    p.set_token_timestamps(true);
    p.set_language(Some("auto"));
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_special(false);
    p.set_print_timestamps(false);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    p.set_n_threads(threads as i32);
    p.set_progress_callback_safe(progress);
    state
        .full(p, samples)
        .map_err(|e| format!("The speech model failed: {e}"))?;
    let seconds = samples.len() as f64 / RATE as f64;
    let eot = ctx.token_eot();
    let mut lines = Vec::new();
    for seg in state.as_iter() {
        let mut pieces = Vec::new();
        for i in 0..seg.n_tokens() {
            let Some(tk) = seg.get_token(i) else { continue };
            let d = tk.token_data();
            // Timestamps and other special tokens come after the text ones.
            if d.id >= eot {
                continue;
            }
            let Ok(text) = tk.to_str_lossy() else {
                continue;
            };
            pieces.push(Piece {
                text: text.into_owned(),
                t0: d.t0,
                t1: d.t1,
                p: d.p,
            });
        }
        let words = words_from_pieces(&pieces, seconds);
        if words.is_empty() {
            continue;
        }
        lines.push(Line {
            text: seg.to_str_lossy().map_or_else(
                |_| {
                    words
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                |t| t.trim().to_string(),
            ),
            words,
        });
    }
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
        .unwrap_or("")
        .to_string();
    Ok(Transcript { language, lines })
}

/// Whether the file at `path` is `m`'s: the right size and SHA-256.
pub fn verify(path: &Path, m: &Model) -> Result<bool, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() != m.bytes {
        return Ok(false);
    }
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(hex == m.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(text: &str, t0: i64, t1: i64) -> Piece {
        Piece {
            text: text.into(),
            t0,
            t1,
            p: 0.9,
        }
    }

    #[test]
    fn pieces_join_into_words_without_punctuation() {
        let w = words_from_pieces(
            &[
                piece(" There", 1, 99),
                piece(" is", 99, 139),
                piece(" tum", 3884, 3884),
                piece("ble", 3904, 3937),
                piece(",", 3937, 3940),
                piece(" I", 1138, 1141),
                piece("'ll", 1141, 1160),
                piece(" '", 1434, 1448),
                piece("Cause", 1448, 1518),
                piece(".", 1518, 1530),
            ],
            100.0,
        );
        let texts: Vec<&str> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["There", "is", "tumble", "I'll", "Cause"]);
        assert_eq!((w[2].start, w[2].end), (38.84, 39.37));
        assert_eq!(w[4].start, 14.48, "the lone quote doesn't start it");
    }

    #[test]
    fn what_isnt_a_word_is_dropped_and_times_stay_in_the_audio() {
        let w = words_from_pieces(
            &[
                piece(" ♪", 0, 50),
                piece(" [", 50, 60),
                piece("Music", 60, 90),
                piece("]", 90, 95),
                piece(" la", 95, 400),
            ],
            2.0,
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].text, "la");
        assert_eq!((w[0].start, w[0].end), (0.95, 2.0));
    }

    #[test]
    fn the_default_model_is_listed_and_names_are_found_either_case() {
        assert!(model(DEFAULT_MODEL).is_some());
        assert_eq!(model("Small").map(|m| m.bytes), Some(190_085_487));
        assert!(model("huge").is_none());
        for m in MODELS {
            assert!(m.url.ends_with(m.file));
            assert_eq!(m.sha256.len(), 64);
        }
    }

    #[test]
    fn a_file_of_the_wrong_size_is_not_the_model() {
        let dir = std::env::temp_dir().join(format!("daw-speech-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.bin");
        std::fs::write(&f, b"not a model").unwrap();
        assert_eq!(verify(&f, &MODELS[0]), Ok(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With a model file and a recording named by `DAW_SPEECH_MODEL`
    /// and `DAW_SPEECH_RAW` (mono 16 kHz, 32-bit float raw), prints what
    /// it hears. Skipped otherwise: CI has no model.
    #[test]
    fn reads_a_real_recording_when_given_one() {
        let (Some(m), Some(a)) = (
            std::env::var_os("DAW_SPEECH_MODEL"),
            std::env::var_os("DAW_SPEECH_RAW"),
        ) else {
            return;
        };
        let raw = std::fs::read(a).unwrap();
        let x: Vec<f32> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let t = transcribe(Path::new(&m), &x, |_| {}).unwrap();
        for l in &t.lines {
            println!("{}", l.text);
            for w in &l.words {
                println!("  {:7.2}-{:7.2} {} ({:.2})", w.start, w.end, w.text, w.sure);
            }
        }
        assert!(t.words().count() > 0);
    }
}
