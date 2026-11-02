// SPDX-License-Identifier: GPL-3.0-or-later
//! Reads sounds for the Lucky Seven from the public bulk-dump format of
//! the original: a system-exclusive file holding thirty-two packed
//! sounds (4096 bytes of data) or one sound unpacked (155 bytes), as
//! the original sent them over MIDI and as sound collections for it
//! are kept. Several dumps in one file are read one after another, and
//! a file of bare packed sounds with no system-exclusive wrapping reads
//! too. Nothing comes with the app: this is for the owner's own
//! sounds.
//!
//! Layout, from the format's public documentation: each packed sound is
//! 128 bytes, operators six down to one at 17 bytes each (four rates,
//! four levels, the break point, the two depths, the two curves in one
//! byte, the detune and rate scaling in one, the velocity and
//! amplitude-modulation sensitivities in one, the output level, the
//! coarse frequency and mode in one, the fine frequency), then the
//! pitch envelope's eight values, the algorithm, the feedback with the
//! sync switch, the LFO's speed, delay, two depths, and its sync, shape
//! and pitch sensitivity in one byte, the transpose, and ten characters
//! of name.

use std::path::Path;

use super::{at, op, op_key};

/// One sound read from a bank: its name and the settings that make it,
/// by key, ready for the instrument's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct BankVoice {
    pub name: String,
    pub settings: Vec<(String, f64)>,
}

/// A bank read from a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Bank {
    /// The file it came from, as given.
    pub file: String,
    pub voices: Vec<BankVoice>,
}

const PACKED: usize = 128;
const UNPACKED: usize = 155;

/// The sounds in a file.
pub fn open(path: &Path) -> Result<Bank, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let voices = read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Bank {
        file: path.display().to_string(),
        voices,
    })
}

/// The sounds in the bytes of a file.
pub fn read(bytes: &[u8]) -> Result<Vec<BankVoice>, String> {
    let mut voices = Vec::new();
    let mut found_header = false;
    let mut i = 0;
    while i + 6 <= bytes.len() {
        // A dump starts F0 43 0n ff with ff the format: 09 for a bank
        // of thirty-two, 00 for one sound; then two bytes of length.
        if bytes[i] == 0xF0 && bytes[i + 1] == 0x43 && bytes[i + 2] & 0xF0 == 0 {
            found_header = true;
            let format = bytes[i + 3];
            let len = ((bytes[i + 4] as usize) << 7) | bytes[i + 5] as usize;
            let data = &bytes[(i + 6).min(bytes.len())..];
            match format {
                0x09 => {
                    if len != PACKED * 32 || data.len() < len {
                        return Err("a bank dump that isn't 32 sounds long".into());
                    }
                    for v in data[..len].as_chunks::<PACKED>().0 {
                        voices.push(unpack(v));
                    }
                }
                0x00 => {
                    if len != UNPACKED || data.len() < len {
                        return Err("a sound dump that isn't 155 bytes long".into());
                    }
                    voices.push(unpacked(&data[..len]));
                }
                f => return Err(format!("a dump of format {f:#04x}, not a sound or a bank")),
            }
            // Past the data, its checksum and the end byte.
            i += 6 + len;
            while i < bytes.len() && bytes[i] != 0xF7 {
                i += 1;
            }
            i += 1;
        } else if found_header {
            i += 1;
        } else {
            break;
        }
    }
    if !found_header {
        // Bare packed sounds, as some collections keep them.
        if bytes.is_empty() || !bytes.len().is_multiple_of(PACKED) {
            return Err("not a sound bank: no dump header, and not whole sounds".into());
        }
        for v in bytes.as_chunks::<PACKED>().0 {
            voices.push(unpack(v));
        }
    }
    if voices.is_empty() {
        return Err("no sounds in it".into());
    }
    Ok(voices)
}

/// A name's ten characters, trimmed; a sound with no name is "Sound".
fn name(bytes: &[u8]) -> String {
    let s: String = bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                ' '
            }
        })
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Sound".into() } else { s }
}

/// An operator's settings from its values in the format's own fields.
#[allow(clippy::too_many_arguments)]
fn op_settings(
    o: usize,
    rates: [u8; 4],
    levels: [u8; 4],
    break_point: u8,
    left_depth: u8,
    right_depth: u8,
    left_curve: u8,
    right_curve: u8,
    rate_scale: u8,
    amp_mod: u8,
    velocity: u8,
    level: u8,
    mode: u8,
    coarse: u8,
    fine: u8,
    detune: u8,
    out: &mut Vec<(String, f64)>,
) {
    let k = |k: usize| op_key(o, k);
    let push = |out: &mut Vec<(String, f64)>, k: usize, v: f64| out.push((op_key(o, k), v));
    push(out, op::MODE, (mode & 1) as f64);
    push(out, op::COARSE, coarse.min(31) as f64);
    push(out, op::FINE, fine.min(99) as f64);
    push(out, op::DETUNE, (detune.min(14) as f64) - 7.0);
    push(out, op::LEVEL, level.min(99) as f64);
    for (i, r) in rates.iter().enumerate() {
        out.push((k(op::RATE1 + i), (*r).min(99) as f64));
    }
    for (i, l) in levels.iter().enumerate() {
        out.push((k(op::LEVEL1 + i), (*l).min(99) as f64));
    }
    // Break points count from A-1, which is note 21.
    push(out, op::BREAK, (break_point.min(99) as f64) + 21.0);
    push(out, op::LEFT_CURVE, (left_curve & 3) as f64);
    push(out, op::LEFT_DEPTH, left_depth.min(99) as f64);
    push(out, op::RIGHT_CURVE, (right_curve & 3) as f64);
    push(out, op::RIGHT_DEPTH, right_depth.min(99) as f64);
    push(out, op::RATE_SCALE, (rate_scale & 7) as f64);
    push(out, op::VELOCITY, (velocity & 7) as f64);
    push(out, op::AMP_MOD, (amp_mod & 3) as f64);
}

/// The rest of a sound's settings from the format's own fields.
#[allow(clippy::too_many_arguments)]
fn common_settings(
    pitch_rates: [u8; 4],
    pitch_levels: [u8; 4],
    algorithm: u8,
    feedback: u8,
    osc_sync: u8,
    lfo_speed: u8,
    lfo_delay: u8,
    lfo_pitch: u8,
    lfo_amp: u8,
    lfo_sync: u8,
    lfo_wave: u8,
    pitch_mod_sens: u8,
    transpose: u8,
    out: &mut Vec<(String, f64)>,
) {
    let keys = super::params();
    let key = |i: usize| keys[i].key.clone();
    for (i, r) in pitch_rates.iter().enumerate() {
        out.push((key(at::PITCH_RATE1 + i), (*r).min(99) as f64));
    }
    for (i, l) in pitch_levels.iter().enumerate() {
        out.push((key(at::PITCH_LEVEL1 + i), (*l).min(99) as f64));
    }
    out.push((key(at::ALGORITHM), (algorithm & 31) as f64 + 1.0));
    out.push((key(at::FEEDBACK), (feedback & 7) as f64));
    out.push((key(at::OSC_SYNC), (osc_sync & 1) as f64));
    out.push((key(at::LFO_SPEED), lfo_speed.min(99) as f64));
    out.push((key(at::LFO_DELAY), lfo_delay.min(99) as f64));
    out.push((key(at::LFO_PITCH), lfo_pitch.min(99) as f64));
    out.push((key(at::LFO_AMP), lfo_amp.min(99) as f64));
    out.push((key(at::LFO_SYNC), (lfo_sync & 1) as f64));
    out.push((key(at::LFO_WAVE), lfo_wave.min(5) as f64));
    out.push((key(at::PITCH_MOD_SENS), (pitch_mod_sens & 7) as f64));
    // Transpose counts from two octaves down.
    out.push((key(at::TRANSPOSE), (transpose.min(48) as f64) - 24.0));
}

/// A packed sound of 128 bytes.
fn unpack(v: &[u8]) -> BankVoice {
    let mut out = Vec::with_capacity(at::COUNT);
    for o in 0..6 {
        // Operators are stored six first.
        let b = &v[(5 - o) * 17..(5 - o) * 17 + 17];
        op_settings(
            o,
            [b[0], b[1], b[2], b[3]],
            [b[4], b[5], b[6], b[7]],
            b[8],
            b[9],
            b[10],
            b[11] & 3,
            (b[11] >> 2) & 3,
            b[12] & 7,
            b[13] & 3,
            (b[13] >> 2) & 7,
            b[14],
            b[15] & 1,
            (b[15] >> 1) & 31,
            b[16],
            (b[12] >> 3) & 15,
            &mut out,
        );
    }
    common_settings(
        [v[102], v[103], v[104], v[105]],
        [v[106], v[107], v[108], v[109]],
        v[110],
        v[111] & 7,
        (v[111] >> 3) & 1,
        v[112],
        v[113],
        v[114],
        v[115],
        v[116] & 1,
        (v[116] >> 1) & 7,
        (v[116] >> 4) & 7,
        v[117],
        &mut out,
    );
    BankVoice {
        name: name(&v[118..128]),
        settings: out,
    }
}

/// A single sound of 155 bytes, one value a byte.
fn unpacked(v: &[u8]) -> BankVoice {
    let mut out = Vec::with_capacity(at::COUNT);
    for o in 0..6 {
        let b = &v[(5 - o) * 21..(5 - o) * 21 + 21];
        op_settings(
            o,
            [b[0], b[1], b[2], b[3]],
            [b[4], b[5], b[6], b[7]],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15],
            b[16],
            b[17],
            b[18],
            b[19],
            b[20],
            &mut out,
        );
    }
    common_settings(
        [v[126], v[127], v[128], v[129]],
        [v[130], v[131], v[132], v[133]],
        v[134],
        v[135],
        v[136],
        v[137],
        v[138],
        v[139],
        v[140],
        v[141],
        v[142],
        v[143],
        v[144],
        &mut out,
    );
    BankVoice {
        name: name(&v[145..155]),
        settings: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packed sound with a name, and some values to find again: op 1
    /// at level 99 with ratio 2 and detune +3, op 2 at level 70 fixed
    /// at 100 Hz, algorithm 5, feedback 7 with the sync off.
    pub(crate) fn packed(name: &str) -> Vec<u8> {
        let mut v = vec![0u8; 128];
        for o in 0..6 {
            let b = &mut v[(5 - o) * 17..(5 - o) * 17 + 17];
            b[..4].copy_from_slice(&[99, 80, 70, 60]);
            b[4..8].copy_from_slice(&[99, 90, 80, 0]);
            b[8] = 39; // C3
            b[12] = 7 << 3; // detune 0
            b[15] = 1 << 1; // coarse 1, ratio
        }
        let op1 = &mut v[85..102];
        op1[14] = 99;
        op1[15] = 2 << 1;
        op1[12] = (10 << 3) | 2; // detune +3, rate scaling 2
        op1[13] = (7 << 2) | 1; // velocity 7, amp mod 1
        op1[11] = (3 << 2) | 1; // right +linear, left -exponential
        let op2 = &mut v[68..85];
        op2[14] = 70;
        op2[15] = (2 << 1) | 1; // coarse 2 fixed: 100 Hz
        v[102..106].copy_from_slice(&[99, 99, 99, 99]);
        v[106..110].copy_from_slice(&[50, 50, 50, 50]);
        v[110] = 4; // algorithm 5
        v[111] = 7; // feedback 7, sync off
        v[112] = 35;
        v[113] = 10;
        v[114] = 20;
        v[115] = 30;
        v[116] = (3 << 4) | (4 << 1) | 1; // pms 3, sine, sync on
        v[117] = 36; // transpose +12
        let n = name.as_bytes();
        v[118..118 + n.len().min(10)].copy_from_slice(&n[..n.len().min(10)]);
        for b in &mut v[118 + n.len().min(10)..128] {
            *b = b' ';
        }
        v
    }

    /// A bank dump of `voices` with its header, checksum and end.
    pub(crate) fn dump(voices: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0xF0, 0x43, 0x00, 0x09, 0x20, 0x00];
        let data: Vec<u8> = voices.iter().flatten().copied().collect();
        let sum: u32 = data.iter().map(|&b| b as u32).sum();
        out.extend(&data);
        out.push(((128 - (sum & 0x7F)) & 0x7F) as u8);
        out.push(0xF7);
        out
    }

    fn get(v: &BankVoice, key: &str) -> f64 {
        v.settings
            .iter()
            .find(|s| s.0 == key)
            .unwrap_or_else(|| panic!("{key}"))
            .1
    }

    #[test]
    fn a_bank_dump_reads_its_thirty_two_sounds_with_their_values() {
        let voices: Vec<Vec<u8>> = (0..32).map(|i| packed(&format!("SOUND {i}"))).collect();
        let bank = read(&dump(&voices)).unwrap();
        assert_eq!(bank.len(), 32);
        assert_eq!(bank[0].name, "SOUND 0");
        assert_eq!(bank[31].name, "SOUND 31");
        let v = &bank[7];
        assert_eq!(
            v.settings.len(),
            at::COUNT - 6,
            "all but the playing settings"
        );
        assert_eq!(get(v, "op1_level"), 99.0);
        assert_eq!(get(v, "op1_coarse"), 2.0);
        assert_eq!(get(v, "op1_mode"), 0.0);
        assert_eq!(get(v, "op1_detune"), 3.0);
        assert_eq!(get(v, "op1_rate_scale"), 2.0);
        assert_eq!(get(v, "op1_velocity"), 7.0);
        assert_eq!(get(v, "op1_amp_mod"), 1.0);
        assert_eq!(get(v, "op1_left_curve"), 1.0);
        assert_eq!(get(v, "op1_right_curve"), 3.0);
        assert_eq!(get(v, "op1_rate1"), 99.0);
        assert_eq!(get(v, "op1_rate4"), 60.0);
        assert_eq!(get(v, "op1_level2"), 90.0);
        assert_eq!(get(v, "op1_break"), 60.0);
        assert_eq!(get(v, "op2_level"), 70.0);
        assert_eq!(get(v, "op2_mode"), 1.0);
        assert_eq!(get(v, "op2_coarse"), 2.0);
        assert_eq!(get(v, "op6_level"), 0.0);
        assert_eq!(get(v, "op6_detune"), 0.0);
        assert_eq!(get(v, "algorithm"), 5.0);
        assert_eq!(get(v, "feedback"), 7.0);
        assert_eq!(get(v, "osc_sync"), 0.0);
        assert_eq!(get(v, "lfo_speed"), 35.0);
        assert_eq!(get(v, "lfo_delay"), 10.0);
        assert_eq!(get(v, "lfo_pitch"), 20.0);
        assert_eq!(get(v, "lfo_amp"), 30.0);
        assert_eq!(get(v, "lfo_sync"), 1.0);
        assert_eq!(get(v, "lfo_wave"), 4.0);
        assert_eq!(get(v, "pitch_mod_sens"), 3.0);
        assert_eq!(get(v, "pitch_level1"), 50.0);
        assert_eq!(get(v, "transpose"), 12.0);
        // Every key is one the instrument has.
        let keys = super::super::params();
        for (k, _) in &v.settings {
            assert!(keys.iter().any(|p| &p.key == k), "{k}");
        }
        // Two dumps in one file read on; bare packed sounds read too.
        let mut two = dump(&voices);
        two.extend(dump(&voices));
        assert_eq!(read(&two).unwrap().len(), 64);
        let bare: Vec<u8> = voices[..3].iter().flatten().copied().collect();
        assert_eq!(read(&bare).unwrap().len(), 3);
    }

    #[test]
    fn a_single_sound_dump_reads_and_rubbish_is_refused() {
        let mut v = vec![0u8; 155];
        for o in 0..6 {
            let b = &mut v[(5 - o) * 21..(5 - o) * 21 + 21];
            b[..8].copy_from_slice(&[99, 99, 99, 99, 99, 99, 99, 0]);
            b[8] = 39;
            b[18] = 1;
            b[20] = 7;
        }
        v[16] = 80; // op 6's level
        v[105 + 16] = 99; // op 1's level
        v[105 + 20] = 12; // op 1 detune +5
        v[134] = 31; // algorithm 32
        v[135] = 3;
        v[144] = 24;
        v[145..155].copy_from_slice(b"ONE SOUND ");
        let mut out = vec![0xF0, 0x43, 0x00, 0x00, 0x01, 0x1B];
        out.extend(&v);
        out.push(0);
        out.push(0xF7);
        let bank = read(&out).unwrap();
        assert_eq!(bank.len(), 1);
        assert_eq!(bank[0].name, "ONE SOUND");
        assert_eq!(get(&bank[0], "op6_level"), 80.0);
        assert_eq!(get(&bank[0], "op1_level"), 99.0);
        assert_eq!(get(&bank[0], "op1_detune"), 5.0);
        assert_eq!(get(&bank[0], "algorithm"), 32.0);
        assert_eq!(get(&bank[0], "feedback"), 3.0);
        assert_eq!(get(&bank[0], "transpose"), 0.0);
        assert!(read(b"hello").is_err());
        assert!(read(&[]).is_err());
        assert!(read(&[0xF0, 0x43, 0x00, 0x09, 0x20, 0x00, 1, 2, 3]).is_err());
        assert!(read(&[0xF0, 0x43, 0x00, 0x05, 0x00, 0x02, 1, 2, 0xF7]).is_err());
    }
}
