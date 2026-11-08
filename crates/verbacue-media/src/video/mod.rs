//! Video output: a copy of the input with subtitles added.
//!
//! [`embed`] writes MKV. It copies all audio and video streams without re-encoding and
//! adds the subtitles as SRT tracks. MKV can hold almost any codec, so this works for
//! any input that FFmpeg can read.
//!
//! [`burn`] writes MP4. It decodes the video, draws the subtitle text into each frame,
//! and encodes it again with the hardware H.264 encoder (VideoToolbox) on macOS, or with
//! FFmpeg's own MPEG-4 encoder elsewhere. Audio is copied if MP4 can hold its codec, and
//! is encoded to AAC otherwise.

mod burn;
mod embed;
mod fonts;
mod overlay;

use std::path::Path;

use ffmpeg_next::{
    self as ffmpeg, Dictionary, Rational, codec, encoder,
    format::{self},
    frame, media,
};

use verbacue_core::{
    Progress,
    error::{Context as _, ErrorKind, Result, WithKind as _},
    subtitle::Cue,
};

use burn::{open_video_decoder, write_burned};
use embed::write_embedded;

pub struct SubtitleTrack<'a> {
    /// ISO 639-1 code, for example "hu".
    pub language: &'a str,
    pub cues: &'a [Cue],
}

/// Copies `input` to `output` (MKV) and adds `tracks` as subtitle tracks. The first
/// track becomes the default subtitle track.
///
/// # Errors
/// Returns an error if a file cannot be read or written.
pub fn embed(
    input: &Path,
    output: &Path,
    tracks: &[SubtitleTrack],
    progress: &dyn Progress,
) -> Result<()> {
    crate::audio::init()?;
    remove_on_error(output, write_embedded(input, output, tracks, progress))
}

/// Re-encodes the main video stream of `input` with `cues` drawn into the picture, and
/// writes MP4. Input subtitle streams are dropped, so the burned text is the only one.
///
/// # Errors
/// Returns an error if the video cannot be decoded or encoded.
pub fn burn(input: &Path, output: &Path, cues: &[Cue], progress: &dyn Progress) -> Result<()> {
    crate::audio::init()?;
    remove_on_error(output, write_burned(input, output, cues, progress))
}

/// Checks before the slow steps that `burn` can decode the video of `input`. It
/// decodes the first frame, because some codecs (for example AV1 on Linux) have no
/// decoder in this build.
///
/// # Errors
/// Returns an error that says why `burn` would fail.
pub fn check_burn(input: &Path) -> Result<()> {
    crate::audio::init()?;
    let mut ictx = open_input(input)?;
    let ist = ictx
        .streams()
        .best(media::Type::Video)
        .context("the input has no video stream")?;
    let index = ist.index();
    let codec = ist.parameters().id().name();
    let cannot = || {
        format!(
            "--burn cannot decode the {codec} video in {}; use --embed instead, \
             which copies the video without decoding it",
            input.display()
        )
    };
    let mut decoder = open_video_decoder(&ist).with_context(cannot)?;
    let mut frame = frame::Video::empty();
    for (_, packet) in ictx.packets().filter(|(s, _)| s.index() == index).take(500) {
        decoder
            .send_packet(&packet)
            .with_kind(ErrorKind::Other)
            .with_context(cannot)?;
        if decoder.receive_frame(&mut frame).is_ok() {
            return Ok(());
        }
    }
    decoder
        .send_eof()
        .with_kind(ErrorKind::Other)
        .with_context(cannot)?;
    decoder
        .receive_frame(&mut frame)
        .with_kind(ErrorKind::Other)
        .with_context(cannot)
}

/// Deletes `output` if writing it failed, so no broken file stays behind.
pub(super) fn remove_on_error(output: &Path, result: Result<()>) -> Result<()> {
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

pub(super) fn open_input(path: &Path) -> Result<format::context::Input> {
    format::input(path)
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("cannot open {}", path.display()))
}

/// Writes the header. `strict=experimental` lets MKV accept codec tags from other
/// containers (for example `avc1` from MP4); MKV does not use those tags anyway.
/// `default_mode=infer` marks the first subtitle track as the default one.
pub(super) fn write_header(octx: &mut format::context::Output) -> Result<()> {
    let mut options = Dictionary::new();
    options.set("strict", "experimental");
    options.set("default_mode", "infer");
    octx.write_header_with(options)
        .with_kind(ErrorKind::Other)
        .context("cannot write the output header")?;
    Ok(())
}

/// Maps input stream indexes to output stream indexes for copied streams.
pub(super) struct StreamMap(Vec<Option<usize>>);

impl StreamMap {
    fn new(ictx: &format::context::Input) -> Self {
        Self(vec![None; ictx.nb_streams() as usize])
    }

    fn copy_stream(
        &mut self,
        ist: &format::stream::Stream,
        octx: &mut format::context::Output,
    ) -> Result<()> {
        let mut ost = octx
            .add_stream(encoder::find(codec::Id::None))
            .with_kind(ErrorKind::Other)?;
        ost.set_parameters(ist.parameters());
        ost.set_metadata(ist.metadata().to_owned());
        self.0[ist.index()] = Some(ost.index());
        Ok(())
    }

    fn output_index(&self, input: usize) -> Option<usize> {
        self.0.get(input).copied().flatten()
    }
}

/// The start time of a stream in ms. Whisper times start at the first audio sample,
/// so the subtitles move by this amount (it is not zero in, for example, MPEG-TS files).
pub(super) fn media_start_ms(stream: &format::stream::Stream) -> i64 {
    match stream.start_time() {
        ffmpeg::ffi::AV_NOPTS_VALUE => 0,
        t => rescale_ms(t, stream.time_base()),
    }
}

/// The start time of the first audio stream, which is where the transcript starts.
pub(super) fn audio_start_ms(ictx: &format::context::Input) -> i64 {
    ictx.streams()
        .find(|s| s.parameters().medium() == media::Type::Audio)
        .map_or(0, |s| media_start_ms(&s))
}

pub(super) fn duration_secs(ictx: &format::context::Input) -> f64 {
    ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
}

pub(super) fn rescale_ms(ts: i64, tb: Rational) -> i64 {
    let (num, den) = (
        i128::from(tb.numerator()),
        i128::from(tb.denominator().max(1)),
    );
    (i128::from(ts) * num * 1000 / den) as i64
}

pub(super) fn report(progress: &dyn Progress, ms: i64, total_secs: f64) {
    if total_secs > 0.0 {
        let percent = ((ms as f64 / 1000.0 / total_secs) * 100.0).clamp(0.0, 100.0) as u64;
        progress.update(percent, 100);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stopped;

    impl Progress for Stopped {
        fn update(&self, _done: u64, _total: u64) {}

        fn cancelled(&self) -> bool {
            true
        }
    }

    /// Makes a test video with the ffmpeg command, or returns `None` when it is missing.
    pub(crate) fn test_video(
        name: &str,
        seconds: u32,
        size: &str,
        audio: bool,
    ) -> Option<std::path::PathBuf> {
        let path = std::env::temp_dir().join(name);
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size={size}:rate=25:duration={seconds}"));
        if audio {
            cmd.args(["-f", "lavfi", "-i"])
                .arg(format!("sine=frequency=440:duration={seconds}"));
        }
        cmd.args(["-pix_fmt", "yuv420p"]).arg(&path);
        match cmd.status() {
            Ok(s) if s.success() => Some(path),
            _ => {
                eprintln!("skipped: the ffmpeg command is needed to make the test video");
                None
            }
        }
    }

    #[test]
    fn burn_removes_the_output_when_cancelled() {
        let Some(input) = test_video("verbacue-burn-cancel-in.mp4", 2, "320x240", true) else {
            return;
        };
        let output = std::env::temp_dir().join("verbacue-burn-cancel-out.mp4");
        let cues = [Cue::new(0, 1500, "hello")];
        let err = burn(&input, &output, &cues, &Stopped).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert!(!output.exists());
    }

    #[test]
    fn rescales_timestamps_to_ms() {
        assert_eq!(rescale_ms(90_000, Rational::new(1, 90_000)), 1000);
        assert_eq!(rescale_ms(1, Rational::new(1001, 24_000)), 41);
    }
}
