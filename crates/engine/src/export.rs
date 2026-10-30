// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing a mix to a file (roadmap 1.8, 3.6): the whole graph for a
//! mixdown, or a graph of one track or bus for a stem, as WAV, FLAC, MP3
//! or Opus.
//!
//! WAV and FLAC are 24-bit at the session's rate. MP3 is constant bit
//! rate, with the encoder's own header frame giving players its delay
//! and padding so the file starts and ends where the mix does. Opus is
//! always 48 kHz, so other rates are converted first; it goes in an Ogg
//! file as the Opus-in-Ogg specification (RFC 7845) lays out. The
//! encoders' licences are in docs/decisions/0014-export-encoders.md.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::engine::render_with;
use crate::graph::Graph;
use crate::time::Frames;

/// A kind of file to export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 24-bit WAV.
    Wav,
    /// 24-bit FLAC: lossless, about half the size of WAV.
    Flac,
    /// MP3 at this many kbit/s, one of [`MP3_KBPS`].
    Mp3(u32),
    /// Opus at this many kbit/s, one of [`OPUS_KBPS`].
    Opus(u32),
}

/// The MP3 bit rates offered, in kbit/s.
pub const MP3_KBPS: [u32; 4] = [128, 192, 256, 320];
/// The Opus bit rates offered, in kbit/s.
pub const OPUS_KBPS: [u32; 5] = [96, 128, 160, 192, 256];

impl Format {
    /// The file name ending.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Flac => "flac",
            Format::Mp3(_) => "mp3",
            Format::Opus(_) => "opus",
        }
    }
}

/// Renders `frames` frames of `graph` from `from` into a stereo 24-bit
/// WAV file at `path`. See [`write`].
pub fn write_wav(
    graph: Graph,
    from: Frames,
    frames: usize,
    path: &Path,
    done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Written, String> {
    write(graph, from, frames, path, Format::Wav, done, cancel)
}

/// Renders `frames` frames of `graph` from `from` into a stereo file of
/// `format` at `path`. `done` counts frames rendered so far; setting
/// `cancel` stops early and removes the file. The file appears only when
/// complete: it is written beside `path` first and renamed.
pub fn write(
    graph: Graph,
    from: Frames,
    frames: usize,
    path: &Path,
    format: Format,
    done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Written, String> {
    let rate = graph.tempo().sample_rate();
    let tmp = path.with_extension(format!("{}.part", format.extension()));
    let file =
        File::create(&tmp).map_err(|e| format!("could not create {}: {e}", tmp.display()))?;
    let out = BufWriter::new(file);
    let made: Result<Box<dyn Encode>, String> = match format {
        Format::Wav => Wav::new(out, rate).map(|e| Box::new(e) as _),
        Format::Flac => Flac::new(out, rate).map(|e| Box::new(e) as _),
        Format::Mp3(kbps) => Mp3::new(out, rate, kbps).map(|e| Box::new(e) as _),
        Format::Opus(kbps) => Opus::new(out, rate, kbps).map(|e| Box::new(e) as _),
    };
    let mut enc = match made {
        Ok(e) => e,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("could not write {}: {e}", path.display()));
        }
    };
    let mut error = None;
    let mut peak = 0.0f32;
    let block = graph.max_block();
    render_with(graph, from, frames, block, |l, r| {
        for x in l.iter().chain(r) {
            peak = peak.max(x.abs());
        }
        if let Err(e) = enc.push(l, r) {
            error = Some(e);
            return false;
        }
        done.fetch_add(l.len() as u64, Ordering::Relaxed);
        !cancel.load(Ordering::Relaxed)
    });
    let finished = enc.finish();
    if let Some(e) = error.or(finished.err()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not write {}: {e}", path.display()));
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&tmp);
        return Err("cancelled".into());
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(Written { peak })
}

/// What [`write`] wrote.
#[derive(Clone, Copy, Debug)]
pub struct Written {
    /// Highest sample level before clipping to full scale.
    pub peak: f32,
}

impl Written {
    pub fn clipped(&self) -> bool {
        self.peak > 1.0
    }
}

type Out = BufWriter<File>;

/// One kind of file being written, a block of stereo audio at a time.
trait Encode {
    fn push(&mut self, l: &[f32], r: &[f32]) -> Result<(), String>;
    /// Writes what is left and completes the file.
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// Full scale for 24-bit samples.
const FULL_24: f32 = 8_388_607.0;

fn to_24(x: f32) -> i32 {
    (x.clamp(-1.0, 1.0) * FULL_24).round() as i32
}

struct Wav(hound::WavWriter<Out>);

impl Wav {
    fn new(out: Out, rate: u32) -> Result<Wav, String> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let w = hound::WavWriter::new(out, spec).map_err(|e| e.to_string())?;
        Ok(Wav(w))
    }
}

impl Encode for Wav {
    fn push(&mut self, l: &[f32], r: &[f32]) -> Result<(), String> {
        for (a, b) in l.iter().zip(r) {
            for x in [a, b] {
                self.0.write_sample(to_24(*x)).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        self.0.finalize().map_err(|e| e.to_string())
    }
}

/// Frames in each FLAC block: the reference encoder's usual size.
const FLAC_BLOCK: usize = 4096;

/// FLAC, written a frame at a time. Its header block holds the length
/// and a checksum of the audio, so it is written again at the end.
struct Flac {
    out: Out,
    config: flacenc::error::Verified<flacenc::config::Encoder>,
    info: flacenc::component::StreamInfo,
    buf: flacenc::source::FrameBuf,
    context: flacenc::source::Context,
    /// Interleaved samples waiting for a full block.
    pending: Vec<i32>,
    frame: usize,
}

impl Flac {
    fn new(mut out: Out, rate: u32) -> Result<Flac, String> {
        use flacenc::error::Verify;
        let e = |e: &dyn std::fmt::Display| e.to_string();
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|(_, err)| e(&err))?;
        let info = flacenc::component::StreamInfo::new(rate as usize, 2, 24).map_err(|x| e(&x))?;
        out.write_all(b"fLaC").map_err(|x| e(&x))?;
        write_flac_info(&mut out, &info)?;
        Ok(Flac {
            out,
            config,
            info,
            buf: flacenc::source::FrameBuf::with_size(2, FLAC_BLOCK).map_err(|x| e(&x))?,
            context: flacenc::source::Context::new(24, 2),
            pending: Vec::with_capacity(2 * FLAC_BLOCK),
            frame: 0,
        })
    }

    /// Encodes and writes the pending samples as one frame.
    fn frame_out(&mut self) -> Result<(), String> {
        use flacenc::component::BitRepr;
        use flacenc::source::Fill;
        let e = |e: &dyn std::fmt::Display| e.to_string();
        self.buf
            .fill_interleaved(&self.pending)
            .map_err(|x| e(&x))?;
        self.context
            .fill_interleaved(&self.pending)
            .map_err(|x| e(&x))?;
        let frame =
            flacenc::encode_fixed_size_frame(&self.config, &self.buf, self.frame, &self.info)
                .map_err(|x| e(&x))?;
        self.info.update_frame_info(&frame);
        let mut sink = flacenc::bitsink::MemSink::<u8>::with_capacity(frame.count_bits());
        frame.write(&mut sink).map_err(|x| e(&x))?;
        self.out.write_all(sink.as_slice()).map_err(|x| e(&x))?;
        self.pending.clear();
        self.frame += 1;
        Ok(())
    }
}

/// The FLAC header block (STREAMINFO, the only one): its 4-byte heading,
/// marked as the last block, then the 34 bytes of `info`.
fn write_flac_info(out: &mut Out, info: &flacenc::component::StreamInfo) -> Result<(), String> {
    use flacenc::component::BitRepr;
    let mut sink = flacenc::bitsink::MemSink::<u8>::with_capacity(info.count_bits());
    info.write(&mut sink).map_err(|e| e.to_string())?;
    let body = sink.as_slice();
    let len = body.len() as u32;
    out.write_all(&[0x80, (len >> 16) as u8, (len >> 8) as u8, len as u8])
        .and_then(|_| out.write_all(body))
        .map_err(|e| e.to_string())
}

impl Encode for Flac {
    fn push(&mut self, l: &[f32], r: &[f32]) -> Result<(), String> {
        for (a, b) in l.iter().zip(r) {
            self.pending.push(to_24(*a));
            self.pending.push(to_24(*b));
            if self.pending.len() == 2 * FLAC_BLOCK {
                self.frame_out()?;
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        if !self.pending.is_empty() {
            self.frame_out()?;
        }
        // The last block may be short; the header gives the usual size.
        let block = self.info.max_block_size();
        let _ = self.info.set_block_sizes(block, block);
        self.info.set_md5_digest(&self.context.md5_digest());
        let mut out = self.out;
        out.seek(SeekFrom::Start(4)).map_err(|e| e.to_string())?;
        write_flac_info(&mut out, &self.info)?;
        out.flush().map_err(|e| e.to_string())
    }
}

/// MP3 through the LAME library, at a constant bit rate.
struct Mp3 {
    out: Out,
    lame: mp3lame_encoder::Encoder,
    buf: Vec<u8>,
}

impl Mp3 {
    fn new(out: Out, rate: u32, kbps: u32) -> Result<Mp3, String> {
        use mp3lame_encoder::{Bitrate, Builder, Mode, Quality};
        let brate = match kbps {
            128 => Bitrate::Kbps128,
            192 => Bitrate::Kbps192,
            256 => Bitrate::Kbps256,
            320 => Bitrate::Kbps320,
            _ => return Err(format!("{kbps} kbit/s is not an MP3 rate offered")),
        };
        let e = |e: mp3lame_encoder::BuildError| format!("MP3 encoder: {e:?}");
        let mut b = Builder::new().ok_or("the MP3 encoder could not start")?;
        b.set_num_channels(2).map_err(e)?;
        // The input rate; the encoder picks the nearest MP3 rate, and
        // converts, for rates MP3 has no room for (such as 96 kHz).
        b.set_sample_rate(rate).map_err(e)?;
        b.set_brate(brate).map_err(e)?;
        b.set_mode(Mode::JointStereo).map_err(e)?;
        b.set_quality(Quality::Best).map_err(e)?;
        // Space at the start for the header frame written in `finish`.
        b.set_to_write_vbr_tag(true).map_err(e)?;
        Ok(Mp3 {
            out,
            lame: b.build().map_err(e)?,
            buf: Vec::new(),
        })
    }
}

impl Encode for Mp3 {
    fn push(&mut self, l: &[f32], r: &[f32]) -> Result<(), String> {
        let l: Vec<f32> = l.iter().map(|x| x.clamp(-1.0, 1.0)).collect();
        let r: Vec<f32> = r.iter().map(|x| x.clamp(-1.0, 1.0)).collect();
        self.buf.clear();
        self.buf
            .reserve(mp3lame_encoder::max_required_buffer_size(l.len()));
        let input = mp3lame_encoder::DualPcm {
            left: &l[..],
            right: &r[..],
        };
        self.lame
            .encode_to_vec(input, &mut self.buf)
            .map_err(|e| format!("MP3 encoder: {e:?}"))?;
        self.out.write_all(&self.buf).map_err(|e| e.to_string())
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        self.buf.clear();
        self.buf.reserve(7200);
        self.lame
            .flush_to_vec::<mp3lame_encoder::FlushGap>(&mut self.buf)
            .map_err(|e| format!("MP3 encoder: {e:?}"))?;
        self.out.write_all(&self.buf).map_err(|e| e.to_string())?;
        // The header frame, over the space kept for it: the length, and
        // the encoder's delay and padding for players to take off.
        self.buf.clear();
        self.buf.reserve(self.lame.lame_tag_size());
        if self.lame.lame_tag_encode_to_vec(&mut self.buf).is_some() {
            self.out
                .seek(SeekFrom::Start(0))
                .map_err(|e| e.to_string())?;
            self.out.write_all(&self.buf).map_err(|e| e.to_string())?;
        }
        self.out.flush().map_err(|e| e.to_string())
    }
}

/// Opus works at 48 kHz.
const OPUS_RATE: u32 = 48_000;
/// Frames in each Opus packet: 20 ms, the encoder's usual size.
const OPUS_PACKET: usize = 960;
/// The Ogg stream's serial number. Any number will do for a file with
/// one stream; a fixed one keeps exports of the same mix identical.
const OGG_SERIAL: u32 = 0x4441_5721;

/// Opus in an Ogg file. At 48 kHz it is encoded as it comes; at any
/// other rate the whole render is kept and converted at the end, so a
/// long song holds its audio in memory for a moment.
struct Opus {
    writer: ogg::PacketWriter<'static, Out>,
    enc: opus::Encoder,
    rate: u32,
    /// Interleaved samples waiting for a full packet, or at another
    /// rate, the whole render.
    pending: Vec<f32>,
    /// Frames of the encoder's own delay at the start, for players to
    /// skip.
    pre_skip: u64,
    /// Frames at 48 kHz given to the encoder so far.
    fed: u64,
    packet: Vec<u8>,
    /// The last packet, held back so the final one can be marked as the
    /// end of the stream.
    held: Option<(Vec<u8>, u64)>,
}

impl Opus {
    fn new(out: Out, rate: u32, kbps: u32) -> Result<Opus, String> {
        if !OPUS_KBPS.contains(&kbps) {
            return Err(format!("{kbps} kbit/s is not an Opus rate offered"));
        }
        let e = |e: opus::Error| format!("Opus encoder: {e}");
        let mut enc =
            opus::Encoder::new(OPUS_RATE, opus::Channels::Stereo, opus::Application::Audio)
                .map_err(e)?;
        enc.set_bitrate(opus::Bitrate::Bits(kbps as i32 * 1000))
            .map_err(e)?;
        let pre_skip = enc.get_lookahead().map_err(e)? as u64;
        let mut writer = ogg::PacketWriter::new(out);
        // Identification header (RFC 7845 section 5.1).
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(2);
        head.extend((pre_skip as u16).to_le_bytes());
        head.extend(rate.to_le_bytes());
        head.extend(0i16.to_le_bytes());
        head.push(0);
        // Comment header (section 5.2): who wrote it, and no tags.
        let vendor = format!("DAW {}", env!("CARGO_PKG_VERSION"));
        let mut tags = b"OpusTags".to_vec();
        tags.extend((vendor.len() as u32).to_le_bytes());
        tags.extend(vendor.as_bytes());
        tags.extend(0u32.to_le_bytes());
        for h in [head, tags] {
            writer
                .write_packet(h, OGG_SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)
                .map_err(|e| e.to_string())?;
        }
        Ok(Opus {
            writer,
            enc,
            rate,
            pending: Vec::new(),
            pre_skip,
            fed: 0,
            packet: vec![0; 4000],
            held: None,
        })
    }

    /// Encodes the pending samples, a packet at a time, leaving any part
    /// packet. `end` pads that out with silence and also encodes enough
    /// more to get the encoder's delay out, then marks the stream's end
    /// at `total` frames (at 48 kHz).
    fn encode(&mut self, end: Option<u64>) -> Result<(), String> {
        let mut used = 0;
        loop {
            let left = self.pending.len() - used;
            let frame: &[f32] = if left >= 2 * OPUS_PACKET {
                &self.pending[used..used + 2 * OPUS_PACKET]
            } else if end.is_some_and(|total| self.fed < total + self.pre_skip) {
                self.pending.drain(..used);
                used = 0;
                self.pending.resize(2 * OPUS_PACKET, 0.0);
                &self.pending[..]
            } else {
                break;
            };
            let n = self
                .enc
                .encode_float(frame, &mut self.packet)
                .map_err(|e| format!("Opus encoder: {e}"))?;
            used += 2 * OPUS_PACKET;
            self.fed += OPUS_PACKET as u64;
            if let Some((p, granule)) = self.held.take() {
                self.writer
                    .write_packet(
                        p,
                        OGG_SERIAL,
                        ogg::PacketWriteEndInfo::NormalPacket,
                        granule,
                    )
                    .map_err(|e| e.to_string())?;
            }
            self.held = Some((self.packet[..n].to_vec(), self.fed));
        }
        self.pending.drain(..used.min(self.pending.len()));
        if let Some(total) = end
            && let Some((p, _)) = self.held.take()
        {
            // The last packet's position says where the audio ends, so
            // the padding is not played (RFC 7845 section 4).
            self.writer
                .write_packet(
                    p,
                    OGG_SERIAL,
                    ogg::PacketWriteEndInfo::EndStream,
                    total + self.pre_skip,
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Encode for Opus {
    fn push(&mut self, l: &[f32], r: &[f32]) -> Result<(), String> {
        for (a, b) in l.iter().zip(r) {
            self.pending.push(a.clamp(-1.0, 1.0));
            self.pending.push(b.clamp(-1.0, 1.0));
        }
        if self.rate == OPUS_RATE {
            self.encode(None)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        if self.rate != OPUS_RATE {
            self.pending = crate::import::resample(&self.pending, 2, self.rate, OPUS_RATE)?;
        }
        let total = self.fed + (self.pending.len() / 2) as u64;
        self.encode(Some(total))?;
        let mut out = self.writer.into_inner();
        out.flush().map_err(|e| e.to_string())
    }
}
