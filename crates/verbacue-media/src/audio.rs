//! Audio extraction with a statically linked FFmpeg.
//!
//! The decoder reads any container and codec that FFmpeg knows (MKV, MP4, AVI, WebM with
//! AC3, E-AC3, DTS, AAC, Opus, ...). The resampler converts the selected track to the
//! format that Whisper needs (16 kHz, mono, f32), or to stereo for the dub mix.

use crate::error::Result;
use std::{
    borrow::Cow,
    fs::File,
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
};

use ffmpeg_next::{
    self as ffmpeg, ChannelLayout, codec,
    format::{Sample, sample::Type as SampleType},
    frame, media,
    software::resampling,
};

use verbacue_core::{
    Progress, check_cancel,
    error::{Context as _, Error, ErrorKind, bail},
};

pub const SAMPLE_RATE: u32 = 16_000;

/// One audio stream in a media file.
#[derive(Debug, Clone)]
pub struct AudioTrack {
    /// Position among the audio streams only. This is the value for `--audio-track`.
    pub index: usize,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: u16,
    pub sample_rate: u32,
}

/// Starts FFmpeg once per process. Every public function that uses FFmpeg calls it.
pub(crate) fn init() -> Result<()> {
    static INIT: std::sync::OnceLock<std::result::Result<(), String>> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        ffmpeg::init().map_err(|e| e.to_string())?;
        ffmpeg::log::set_level(ffmpeg::log::Level::Error);
        Ok(())
    })
    .clone()
    .map_err(|e| Error::new(ErrorKind::Other, format!("cannot start FFmpeg: {e}")).into())
}

/// Lists the audio streams of a media file.
///
/// # Errors
/// Returns an error if the file cannot be opened or a stream has no usable decoder.
pub fn list_tracks(path: &Path) -> Result<Vec<AudioTrack>> {
    init()?;
    let input =
        ffmpeg::format::input(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut tracks = Vec::new();
    for stream in input.streams() {
        if stream.parameters().medium() != media::Type::Audio {
            continue;
        }
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()?;
        let metadata = stream.metadata();
        tracks.push(AudioTrack {
            index: tracks.len(),
            codec: stream.parameters().id().name().to_string(),
            language: metadata.get("language").map(str::to_string),
            title: metadata.get("title").map(str::to_string),
            channels: decoder.channels(),
            sample_rate: decoder.rate(),
        });
    }
    Ok(tracks)
}

/// Audio in a raw file: interleaved f32 little-endian samples.
#[derive(Debug, Clone)]
pub struct RawAudio {
    pub path: PathBuf,
    pub rate: u32,
    pub channels: u16,
}

/// Decodes one audio track to 16 kHz mono f32 samples.
///
/// `track` is the position among audio streams (see [`AudioTrack::index`]).
/// `progress` receives the percent of the file decoded so far (0-100).
///
/// # Errors
/// Returns an error if the file has no such track, or decoding or resampling fails.
pub fn decode_pcm(path: &Path, track: usize, progress: &dyn Progress) -> Result<Vec<f32>> {
    let mut pcm = Vec::new();
    decode(
        path,
        track,
        ChannelLayout::MONO,
        SAMPLE_RATE,
        progress,
        &mut |s| {
            pcm.extend_from_slice(s);
            Ok(())
        },
    )?;
    if pcm.is_empty() {
        bail!(
            "audio track {track} of {} decoded to no samples",
            path.display()
        );
    }
    Ok(pcm)
}

/// Decodes one audio track to a raw stereo file at `rate` (see [`RawAudio`]). The
/// FFmpeg resampler mixes audio with more channels down to stereo. Returns the number
/// of stereo frames.
///
/// # Errors
/// Returns an error if the file has no such track, decoding fails, or `out` cannot be
/// written. Then `out` is deleted.
pub fn decode_to_file(
    path: &Path,
    track: usize,
    rate: u32,
    out: &Path,
    progress: &dyn Progress,
) -> Result<u64> {
    let result = (|| {
        let file = File::create(out).with_context(|| format!("cannot create {}", out.display()))?;
        let mut writer = BufWriter::new(file);
        let mut samples = 0u64;
        decode(
            path,
            track,
            ChannelLayout::STEREO,
            rate,
            progress,
            &mut |s| {
                for v in s {
                    writer.write_all(&v.to_le_bytes())?;
                }
                samples += s.len() as u64;
                Ok(())
            },
        )?;
        writer
            .flush()
            .with_context(|| format!("cannot write {}", out.display()))?;
        Ok(samples / 2)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

/// Decodes one audio track and gives the resampled, interleaved samples to `sink`.
fn decode(
    path: &Path,
    track: usize,
    layout: ChannelLayout,
    rate: u32,
    progress: &dyn Progress,
    sink: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<()> {
    init()?;
    let mut input =
        ffmpeg::format::input(path).with_context(|| format!("cannot open {}", path.display()))?;

    let stream = input
        .streams()
        .filter(|s| s.parameters().medium() == media::Type::Audio)
        .nth(track)
        .with_context(|| format!("{} has no audio track {track}", path.display()))?;
    let stream_index = stream.index();
    let time_base = f64::from(stream.time_base());
    let mut decoder = codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .audio()
        .context("cannot create the audio decoder")?;

    // The container duration is in AV_TIME_BASE units (microseconds).
    let total_secs = input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

    let target = Target { layout, rate };
    let mut resampler: Option<Resampler> = None;
    let mut decoded = frame::Audio::empty();

    for (stream, packet) in input.packets() {
        check_cancel(progress)?;
        if stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            resample_frame(&mut resampler, target, &decoded, sink)?;
        }
        if total_secs > 0.0
            && let Some(pts) = packet.pts()
        {
            let fraction = (pts as f64 * time_base / total_secs).clamp(0.0, 1.0);
            progress.update((fraction * 100.0) as u64, 100);
        }
    }

    decoder.send_eof()?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        resample_frame(&mut resampler, target, &decoded, sink)?;
    }
    if let Some(r) = resampler.as_mut() {
        r.flush(sink)?;
    }
    progress.update(100, 100);
    Ok(())
}

/// The output format of the resampler.
#[derive(Clone, Copy)]
struct Target {
    layout: ChannelLayout,
    rate: u32,
}

/// A resampler plus the input format it was built for.
struct Resampler {
    ctx: resampling::Context,
    target: Target,
    format: Sample,
    layout_bits: u64,
    channels: u16,
    rate: u32,
}

impl Resampler {
    fn new(frame: &frame::Audio, target: Target) -> Result<Self> {
        let layout = input_layout(frame);
        let ctx = resampling::Context::get(
            frame.format(),
            layout,
            frame.rate(),
            Sample::F32(SampleType::Planar),
            target.layout,
            target.rate,
        )
        .context("cannot create the audio resampler")?;
        Ok(Self {
            ctx,
            target,
            format: frame.format(),
            layout_bits: layout.bits(),
            channels: frame.channels(),
            rate: frame.rate(),
        })
    }

    fn matches(&self, frame: &frame::Audio) -> bool {
        self.format == frame.format()
            && self.layout_bits == input_layout(frame).bits()
            && self.channels == frame.channels()
            && self.rate == frame.rate()
    }

    fn run(
        &mut self,
        frame: &frame::Audio,
        sink: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        // Room for the resampled input plus samples that the resampler still holds.
        let capacity =
            frame.samples() * self.target.rate as usize / self.rate.max(1) as usize + 256;
        let mut converted = output_frame(capacity, self.target);
        self.ctx
            .run(frame, &mut converted)
            .context("resampling failed")?;
        send_samples(&converted, sink)
    }

    fn flush(&mut self, sink: &mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()> {
        loop {
            let mut converted = output_frame(4096, self.target);
            let more = self
                .ctx
                .flush(&mut converted)
                .context("resampler flush failed")?;
            send_samples(&converted, sink)?;
            if more.is_none() || converted.samples() == 0 {
                return Ok(());
            }
        }
    }
}

/// An allocated output frame in the resampler's output format.
fn output_frame(capacity: usize, target: Target) -> frame::Audio {
    let mut frame = frame::Audio::new(Sample::F32(SampleType::Planar), capacity, target.layout);
    frame.set_rate(target.rate);
    frame
}

/// Resamples one decoded frame. Some streams change format in the middle (for example
/// a DTS or AC3 track that switches channel count), so the resampler is rebuilt then.
fn resample_frame(
    resampler: &mut Option<Resampler>,
    target: Target,
    frame: &frame::Audio,
    sink: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<()> {
    if frame.samples() == 0 {
        return Ok(());
    }
    let frame = with_known_layout(frame);
    let frame = frame.as_ref();
    if resampler.as_ref().is_none_or(|r| !r.matches(frame)) {
        if let Some(old) = resampler.as_mut() {
            old.flush(sink)?;
        }
        *resampler = Some(Resampler::new(frame, target)?);
    }
    resampler.as_mut().map_or(Ok(()), |r| r.run(frame, sink))
}

/// Some decoders (for example raw PCM in AIFF or WAV) give frames with an "unspecified"
/// channel layout. The resampler checks that every input frame has the layout it was
/// built for, so such frames get the default layout for their channel count first.
pub fn with_known_layout(frame: &frame::Audio) -> Cow<'_, frame::Audio> {
    if frame.channel_layout().is_empty() {
        let mut fixed = frame.clone();
        fixed.set_channel_layout(ChannelLayout::default(i32::from(frame.channels())));
        Cow::Owned(fixed)
    } else {
        Cow::Borrowed(frame)
    }
}

fn input_layout(frame: &frame::Audio) -> ChannelLayout {
    frame.channel_layout()
}

/// Gives the samples of a planar frame to `sink`, interleaved.
fn send_samples(frame: &frame::Audio, sink: &mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()> {
    let n = frame.samples();
    if n == 0 {
        return Ok(());
    }
    let channels = usize::from(frame.channels()).max(1);
    if channels == 1 {
        return sink(&frame.plane::<f32>(0)[..n]);
    }
    let planes: Vec<&[f32]> = (0..channels).map(|c| &frame.plane::<f32>(c)[..n]).collect();
    let mut interleaved = Vec::with_capacity(n * channels);
    for i in 0..n {
        interleaved.extend(planes.iter().map(|p| p[i]));
    }
    sink(&interleaved)
}

/// Reads until `buf` is full or the reader ends. Returns the bytes read. Raw audio
/// files are read in whole frames this way.
///
/// # Errors
/// Returns the read error.
pub fn read_full(reader: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match reader.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbacue_core::Stopped;

    /// A 16-bit mono PCM WAV with a 440 Hz tone.
    fn write_sine_wav(path: &Path, seconds: u32) {
        let rate = 16_000u32;
        let samples: Vec<i16> = (0..rate * seconds)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 8000.0) as i16
            })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn decode_stops_when_cancelled() {
        let path = std::env::temp_dir().join("verbacue-decode-cancel.wav");
        write_sine_wav(&path, 3);
        assert!(decode_pcm(&path, 0, &()).is_ok());
        let err = decode_pcm(&path, 0, &Stopped).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
    }
}
