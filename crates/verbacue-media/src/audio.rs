//! Audio extraction with a statically linked FFmpeg.
//!
//! The decoder reads any container and codec that FFmpeg knows (MKV, MP4, AVI, WebM with
//! AC3, E-AC3, DTS, AAC, Opus, ...). The resampler converts the selected track to the
//! format that Whisper needs: 16 kHz, mono, f32.

use std::{borrow::Cow, path::Path};

use ffmpeg_next::{
    self as ffmpeg, ChannelLayout, codec,
    format::{Sample, sample::Type as SampleType},
    frame, media,
    software::resampling,
};

use verbacue_core::{
    Progress,
    error::{Context as _, Error, ErrorKind, Result, WithKind as _, bail},
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
    .map_err(|e| Error::new(ErrorKind::Other, format!("cannot start FFmpeg: {e}")))
}

/// Lists the audio streams of a media file.
///
/// # Errors
/// Returns an error if the file cannot be opened or a stream has no usable decoder.
pub fn list_tracks(path: &Path) -> Result<Vec<AudioTrack>> {
    init()?;
    let input = ffmpeg::format::input(path)
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mut tracks = Vec::new();
    for stream in input.streams() {
        if stream.parameters().medium() != media::Type::Audio {
            continue;
        }
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .with_kind(ErrorKind::Other)?
            .decoder()
            .audio()
            .with_kind(ErrorKind::Other)?;
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

/// Decodes one audio track to 16 kHz mono f32 samples.
///
/// `track` is the position among audio streams (see [`AudioTrack::index`]).
/// `progress` receives the percent of the file decoded so far (0-100).
///
/// # Errors
/// Returns an error if the file has no such track, or decoding or resampling fails.
pub fn decode_pcm(path: &Path, track: usize, progress: &dyn Progress) -> Result<Vec<f32>> {
    init()?;
    let mut input = ffmpeg::format::input(path)
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("cannot open {}", path.display()))?;

    let stream = input
        .streams()
        .filter(|s| s.parameters().medium() == media::Type::Audio)
        .nth(track)
        .with_context(|| format!("{} has no audio track {track}", path.display()))?;
    let stream_index = stream.index();
    let time_base = f64::from(stream.time_base());
    let mut decoder = codec::context::Context::from_parameters(stream.parameters())
        .with_kind(ErrorKind::Other)?
        .decoder()
        .audio()
        .with_kind(ErrorKind::Other)
        .context("cannot create the audio decoder")?;

    // The container duration is in AV_TIME_BASE units (microseconds).
    let total_secs = input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

    let mut pcm = Vec::new();
    let mut resampler: Option<Resampler> = None;
    let mut decoded = frame::Audio::empty();

    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet).with_kind(ErrorKind::Other)?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            resample_frame(&mut resampler, &decoded, &mut pcm)?;
        }
        if total_secs > 0.0
            && let Some(pts) = packet.pts()
        {
            let fraction = (pts as f64 * time_base / total_secs).clamp(0.0, 1.0);
            progress.update((fraction * 100.0) as u64, 100);
        }
    }

    decoder.send_eof().with_kind(ErrorKind::Other)?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        resample_frame(&mut resampler, &decoded, &mut pcm)?;
    }
    if let Some(r) = resampler.as_mut() {
        r.flush(&mut pcm)?;
    }
    progress.update(100, 100);

    if pcm.is_empty() {
        bail!(
            "audio track {track} of {} decoded to no samples",
            path.display()
        );
    }
    Ok(pcm)
}

/// A resampler plus the input format it was built for.
struct Resampler {
    ctx: resampling::Context,
    format: Sample,
    layout_bits: u64,
    channels: u16,
    rate: u32,
}

impl Resampler {
    fn new(frame: &frame::Audio) -> Result<Self> {
        let layout = input_layout(frame);
        let ctx = resampling::Context::get(
            frame.format(),
            layout,
            frame.rate(),
            Sample::F32(SampleType::Packed),
            ChannelLayout::MONO,
            SAMPLE_RATE,
        )
        .with_kind(ErrorKind::Other)
        .context("cannot create the audio resampler")?;
        Ok(Self {
            ctx,
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

    fn run(&mut self, frame: &frame::Audio, out: &mut Vec<f32>) -> Result<()> {
        // Room for the resampled input plus samples that the resampler still holds.
        let capacity = frame.samples() * SAMPLE_RATE as usize / self.rate.max(1) as usize + 256;
        let mut converted = output_frame(capacity);
        self.ctx
            .run(frame, &mut converted)
            .with_kind(ErrorKind::Other)
            .context("resampling failed")?;
        append_samples(&converted, out);
        Ok(())
    }

    fn flush(&mut self, out: &mut Vec<f32>) -> Result<()> {
        loop {
            let mut converted = output_frame(4096);
            let more = self
                .ctx
                .flush(&mut converted)
                .with_kind(ErrorKind::Other)
                .context("resampler flush failed")?;
            append_samples(&converted, out);
            if more.is_none() || converted.samples() == 0 {
                return Ok(());
            }
        }
    }
}

/// An allocated output frame in the resampler's output format.
fn output_frame(capacity: usize) -> frame::Audio {
    let mut frame = frame::Audio::new(
        Sample::F32(SampleType::Packed),
        capacity,
        ChannelLayout::MONO,
    );
    frame.set_rate(SAMPLE_RATE);
    frame
}

/// Resamples one decoded frame. Some streams change format in the middle (for example
/// a DTS or AC3 track that switches channel count), so the resampler is rebuilt then.
fn resample_frame(
    resampler: &mut Option<Resampler>,
    frame: &frame::Audio,
    out: &mut Vec<f32>,
) -> Result<()> {
    if frame.samples() == 0 {
        return Ok(());
    }
    let frame = with_known_layout(frame);
    let frame = frame.as_ref();
    if resampler.as_ref().is_none_or(|r| !r.matches(frame)) {
        if let Some(old) = resampler.as_mut() {
            old.flush(out)?;
        }
        *resampler = Some(Resampler::new(frame)?);
    }
    resampler.as_mut().map_or(Ok(()), |r| r.run(frame, out))
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

fn append_samples(frame: &frame::Audio, out: &mut Vec<f32>) {
    if frame.samples() > 0 {
        out.extend_from_slice(&frame.plane::<f32>(0)[..frame.samples()]);
    }
}
