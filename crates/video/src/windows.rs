// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading video on Windows, through the system media framework's
//! source reader. Built and tried on CI only so far: the owner tries it
//! on a real PC (docs/review-queue/3.9-video-clips.md).
//!
//! The reader is asked for 32-bit RGB frames at the size we want; if it
//! can't scale, it is asked for them at the video's own size and we
//! shrink them ourselves.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFByteStream, IMFMediaType, IMFSourceReader, MF_MT_AAC_PAYLOAD_TYPE,
    MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_PD_DURATION, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_STREAMTICK,
    MF_TRANSCODE_CONTAINERTYPE, MF_VERSION, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video,
    MFSTARTUP_FULL, MFShutdown, MFStartup, MFTranscodeContainerType_MPEG4, MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToInt64,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Variant::VT_I8;
use windows_core::{GUID, HSTRING};

use crate::{Error, Frame, Info};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn unreadable(e: windows_core::Error) -> Error {
    Error::Unreadable(e.message())
}

/// The framework started on this thread, and stopped again when
/// dropped.
struct Started;

impl Started {
    fn new() -> Result<Started, Error> {
        // Already started another way on this thread is fine too.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
            .map_err(|e| Error::NoFramework(e.message()))?;
        Ok(Started)
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

/// Two 32-bit numbers packed in one attribute, high one first.
fn pair(t: &IMFAttributes, key: &GUID) -> Option<(u32, u32)> {
    let v = unsafe { t.GetUINT64(key) }.ok()?;
    Some(((v >> 32) as u32, v as u32))
}

fn reader(path: &Path, processing: Option<&GUID>) -> Result<IMFSourceReader, Error> {
    if !path.is_file() {
        return Err(Error::Unreadable(format!("{} isn't there", path.display())));
    }
    let mut attrs: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attrs, 1) }.map_err(unreadable)?;
    let attrs = attrs.ok_or(Error::NoFramework("no attributes".into()))?;
    if let Some(key) = processing {
        unsafe { attrs.SetUINT32(key, 1) }.map_err(unreadable)?;
    }
    let url = HSTRING::from(path);
    let r = unsafe { MFCreateSourceReaderFromURL(&url, &attrs) }.map_err(unreadable)?;
    // Only the picture: its sound is read elsewhere.
    unsafe {
        r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(unreadable)?;
        r.SetStreamSelection(VIDEO, true)
            .map_err(|_| Error::NoPicture)?;
    }
    Ok(r)
}

fn seconds(r: &IMFSourceReader) -> f64 {
    let source = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
    let Ok(mut v) = (unsafe { r.GetPresentationAttribute(source, &MF_PD_DURATION) }) else {
        return 0.0;
    };
    let hns = unsafe { PropVariantToInt64(&v) }.unwrap_or(0);
    let _ = unsafe { PropVariantClear(&mut v) };
    hns as f64 / 1e7
}

pub(crate) fn probe(path: &Path) -> Result<Info, Error> {
    let _mf = Started::new()?;
    let r = reader(path, None)?;
    let t = unsafe { r.GetNativeMediaType(VIDEO, 0) }.map_err(|_| Error::NoPicture)?;
    let (w, h) = pair(&t, &MF_MT_FRAME_SIZE).ok_or(Error::NoPicture)?;
    let (pn, pd) = pair(&t, &MF_MT_PIXEL_ASPECT_RATIO)
        .filter(|&(n, d)| n > 0 && d > 0)
        .unwrap_or((1, 1));
    let rate = pair(&t, &MF_MT_FRAME_RATE)
        .filter(|&(n, d)| n > 0 && d > 0)
        .unwrap_or((30, 1));
    Ok(Info {
        width: ((w as u64 * pn as u64) / pd as u64).max(1) as u32,
        height: h.max(1),
        rate: crate::tidy_rate(rate),
        seconds: seconds(&r),
    })
}

pub(crate) struct Reader {
    // Dropped before the framework stops.
    r: IMFSourceReader,
    /// The size frames come out at, and the bytes from one row to the
    /// next (below zero: the bottom row first).
    size: (u32, u32),
    stride: i32,
    /// Frames ending before this time, in seconds, are passed over: a
    /// seek lands on the key frame before where it was asked to go.
    from: f64,
    _mf: Started,
}

impl Reader {
    pub(crate) fn open(path: &Path, info: &Info, size: (u32, u32)) -> Result<Reader, Error> {
        let mf = Started::new()?;
        let want = |w: u32, h: u32| -> Result<IMFMediaType, Error> {
            let t = unsafe { MFCreateMediaType() }.map_err(unreadable)?;
            unsafe {
                t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .map_err(unreadable)?;
                t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                    .map_err(unreadable)?;
                t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)
                    .map_err(unreadable)?;
                t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
                    .map_err(unreadable)?;
            }
            Ok(t)
        };
        // Scaled by the framework if it can, or else at full size.
        let tries = [
            (
                Some(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING),
                size,
            ),
            (Some(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING), (0, 0)),
            (
                Some(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING),
                (0, 0),
            ),
        ];
        let mut last = Error::Unreadable("no way to convert its picture".into());
        for (processing, (w, h)) in tries {
            let r = reader(path, processing)?;
            let t = if w == 0 {
                // Its own size: only the colour format is asked for.
                let t = unsafe { MFCreateMediaType() }.map_err(unreadable)?;
                unsafe {
                    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                        .map_err(unreadable)?;
                    t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                        .map_err(unreadable)?;
                }
                t
            } else {
                want(w, h)?
            };
            if let Err(e) = unsafe { r.SetCurrentMediaType(VIDEO, None, &t) } {
                last = unreadable(e);
                continue;
            }
            let now = unsafe { r.GetCurrentMediaType(VIDEO) }.map_err(unreadable)?;
            let got = pair(&now, &MF_MT_FRAME_SIZE).unwrap_or((info.width, info.height));
            let stride = unsafe { now.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
                .map_or(got.0 as i32 * 4, |s| s as i32);
            return Ok(Reader {
                r,
                size: got,
                stride,
                from: 0.0,
                _mf: mf,
            });
        }
        Err(last)
    }

    pub(crate) fn seek(&mut self, seconds: f64) -> Result<(), Error> {
        let mut at = PROPVARIANT::default();
        // A 64-bit count of 100 ns units.
        unsafe {
            let v = &mut *at.Anonymous.Anonymous;
            v.vt = VT_I8;
            v.Anonymous.hVal = (seconds * 1e7).round() as i64;
            self.r
                .SetCurrentPosition(&GUID::zeroed(), &at)
                .map_err(unreadable)?;
        }
        self.from = seconds;
        Ok(())
    }

    pub(crate) fn next_frame(&mut self) -> Result<Option<Frame>, Error> {
        loop {
            let mut flags = 0u32;
            let mut time = 0i64;
            let mut sample = None;
            unsafe {
                self.r
                    .ReadSample(
                        VIDEO,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut time),
                        Some(&mut sample),
                    )
                    .map_err(unreadable)?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            // A gap in the stream: read on.
            let Some(sample) = sample else { continue };
            let start = time as f64 / 1e7;
            let len = unsafe { sample.GetSampleDuration() }.unwrap_or(0) as f64 / 1e7;
            // Before where the seek asked for: decoded to get there.
            if start + len.max(1e-4) <= self.from + 1e-6 {
                continue;
            }
            self.from = 0.0;
            let buf = unsafe { sample.ConvertToContiguousBuffer() }.map_err(unreadable)?;
            let (w, h) = (self.size.0 as usize, self.size.1 as usize);
            let mut rgba = vec![0u8; w * h * 4];
            unsafe {
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut n = 0u32;
                buf.Lock(&mut p, None, Some(&mut n)).map_err(unreadable)?;
                let row = self.stride.unsigned_abs() as usize;
                if p.is_null() || (n as usize) < row * (h.saturating_sub(1)) + w * 4 {
                    let _ = buf.Unlock();
                    return Err(Error::Unreadable("a frame came out short".into()));
                }
                let data = std::slice::from_raw_parts(p, n as usize);
                for y in 0..h {
                    // Rows run bottom up when the stride is below zero.
                    let sy = if self.stride < 0 { h - 1 - y } else { y };
                    let src = &data[sy * row..sy * row + w * 4];
                    let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                    // Blue, green, red and an unused byte.
                    for (d, s) in dst
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(src.as_chunks::<4>().0)
                    {
                        d.copy_from_slice(&[s[2], s[1], s[0], 255]);
                    }
                }
                let _ = buf.Unlock();
            }
            return Ok(Some(Frame {
                width: self.size.0,
                height: self.size.1,
                rgba,
                time: start,
            }));
        }
    }
}

/// Sound rates the system's AAC encoder takes.
const AAC_RATES: [u32; 2] = [44_100, 48_000];
/// How much sound goes in each sample written: a tenth of a second.
const CHUNK_SECONDS: f64 = 0.1;

fn audio_type(subtype: &GUID, channels: u32, rate: u32) -> Result<IMFMediaType, Error> {
    let failed = |e: windows_core::Error| Error::CantWrap(e.message());
    let t = unsafe { MFCreateMediaType() }.map_err(failed)?;
    unsafe {
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(failed)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype).map_err(failed)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
            .map_err(failed)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)
            .map_err(failed)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(failed)?;
        if *subtype == MFAudioFormat_AAC {
            // 192 kbit/s, the most this encoder offers but one.
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)
                .map_err(failed)?;
            t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).map_err(failed)?;
        } else {
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels * 2)
                .map_err(failed)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * channels * 2)
                .map_err(failed)?;
            t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
                .map_err(failed)?;
        }
    }
    Ok(t)
}

/// The next stretch of `sound` as 16-bit samples, or none at the end.
fn next_chunk(
    sound: &mut hound::WavReader<std::io::BufReader<std::fs::File>>,
    len: usize,
) -> Result<Vec<i16>, Error> {
    let spec = sound.spec();
    let bad = |e: hound::Error| Error::CantWrap(format!("the mix couldn't be read: {e}"));
    let mut out = Vec::with_capacity(len);
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in sound.samples::<f32>().take(len) {
                out.push((s.map_err(bad)?.clamp(-1.0, 1.0) * 32767.0).round() as i16);
            }
        }
        hound::SampleFormat::Int => {
            let shift = spec.bits_per_sample.saturating_sub(16);
            for s in sound.samples::<i32>().take(len) {
                let v = s.map_err(bad)?;
                out.push(if spec.bits_per_sample <= 16 {
                    v as i16
                } else {
                    // Rounded, not cut, to 16 bits.
                    ((v + (1 << (shift - 1))) >> shift).clamp(-32768, 32767) as i16
                });
            }
        }
    }
    Ok(out)
}

pub(crate) fn wrap(
    video: &Path,
    sound: &Path,
    out: &Path,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let _mf = Started::new()?;
    let failed = |e: windows_core::Error| Error::CantWrap(e.message());

    // The picture, as it is in the file: not decoded.
    let vr = reader(video, None)?;
    let native = unsafe { vr.GetNativeMediaType(VIDEO, 0) }.map_err(|_| Error::NoPicture)?;
    unsafe { vr.SetCurrentMediaType(VIDEO, None, &native) }.map_err(failed)?;

    // The mix.
    let mut mix = hound::WavReader::open(sound)
        .map_err(|e| Error::CantWrap(format!("the mix couldn't be read: {e}")))?;
    let spec = mix.spec();
    let (channels, rate) = (spec.channels as u32, spec.sample_rate);
    if !AAC_RATES.contains(&rate) {
        return Err(Error::CantWrap(format!(
            "the system's sound encoder takes 44.1 or 48 kHz, and this session runs at \
             {} kHz",
            rate as f64 / 1000.0
        )));
    }
    if !(1..=2).contains(&channels) {
        return Err(Error::CantWrap("the mix isn't mono or stereo".into()));
    }

    let mut attrs: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attrs, 2) }.map_err(failed)?;
    let attrs = attrs.ok_or(Error::NoFramework("no attributes".into()))?;
    unsafe {
        attrs
            .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
            .map_err(failed)?;
        attrs
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .map_err(failed)?;
    }
    let url = HSTRING::from(out);
    let w = unsafe { MFCreateSinkWriterFromURL(&url, None::<&IMFByteStream>, &attrs) }
        .map_err(failed)?;
    let picture = unsafe { w.AddStream(&native) }.map_err(|e| {
        Error::CantWrap(format!(
            "an MP4 file can't hold this film's picture as it is ({})",
            e.message()
        ))
    })?;
    unsafe { w.SetInputMediaType(picture, &native, None::<&IMFAttributes>) }.map_err(failed)?;
    let aac = audio_type(&MFAudioFormat_AAC, channels, rate)?;
    let pcm = audio_type(&MFAudioFormat_PCM, channels, rate)?;
    let audio = unsafe { w.AddStream(&aac) }.map_err(failed)?;
    unsafe { w.SetInputMediaType(audio, &pcm, None::<&IMFAttributes>) }.map_err(failed)?;
    unsafe { w.BeginWriting() }.map_err(failed)?;

    let chunk = ((rate as f64 * CHUNK_SECONDS) as usize).max(1) * channels as usize;
    // Where each stream has got to, in 100 ns units; `None` once ended.
    let mut picture_at = Some(0i64);
    let mut sound_at = Some(0i64);
    let mut written = 0u64;
    while picture_at.is_some() || sound_at.is_some() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::CantWrap("cancelled".into()));
        }
        // Whichever is behind goes next, so they go in side by side.
        let picture_next = match (picture_at, sound_at) {
            (Some(p), Some(s)) => p <= s,
            (Some(_), None) => true,
            _ => false,
        };
        if picture_next {
            let mut flags = 0u32;
            let mut time = 0i64;
            let mut sample = None;
            unsafe {
                vr.ReadSample(
                    VIDEO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .map_err(failed)?;
            }
            if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                unsafe { w.SendStreamTick(picture, time) }.map_err(failed)?;
            }
            if let Some(s) = sample {
                unsafe { w.WriteSample(picture, &s) }.map_err(failed)?;
                picture_at = Some(time);
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                picture_at = None;
            }
        } else {
            let samples = next_chunk(&mut mix, chunk)?;
            if samples.is_empty() {
                sound_at = None;
                continue;
            }
            let bytes = samples.len() * 2;
            let frames = (samples.len() / channels as usize) as u64;
            let buf = unsafe { MFCreateMemoryBuffer(bytes as u32) }.map_err(failed)?;
            unsafe {
                let mut p: *mut u8 = std::ptr::null_mut();
                buf.Lock(&mut p, None, None).map_err(failed)?;
                if p.is_null() {
                    let _ = buf.Unlock();
                    return Err(Error::CantWrap("no memory for the sound".into()));
                }
                let dst = std::slice::from_raw_parts_mut(p, bytes);
                for (d, s) in dst.as_chunks_mut::<2>().0.iter_mut().zip(&samples) {
                    *d = s.to_le_bytes();
                }
                let _ = buf.Unlock();
                buf.SetCurrentLength(bytes as u32).map_err(failed)?;
            }
            let start = (written as f64 * 1e7 / rate as f64).round() as i64;
            written += frames;
            let end = (written as f64 * 1e7 / rate as f64).round() as i64;
            let s = unsafe { MFCreateSample() }.map_err(failed)?;
            unsafe {
                s.AddBuffer(&buf).map_err(failed)?;
                s.SetSampleTime(start).map_err(failed)?;
                s.SetSampleDuration(end - start).map_err(failed)?;
                w.WriteSample(audio, &s).map_err(failed)?;
            }
            sound_at = Some(end);
        }
    }
    unsafe { w.Finalize() }.map_err(failed)?;
    Ok(())
}
