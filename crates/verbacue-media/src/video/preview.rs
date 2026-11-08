//! Still frames and facts about a video, for a preview before the slow steps.

use std::path::Path;

use ffmpeg_next::{
    self as ffmpeg,
    format::Pixel,
    frame, media,
    software::scaling::{Context as Scaler, Flags},
};

use verbacue_core::error::{Context as _, ErrorKind, Result, WithKind as _, bail};

use super::{open_input, open_video_decoder, rescale_ms};
use crate::audio::{AudioTrack, list_tracks};

/// One decoded frame as RGBA pixels, row by row.
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    /// The position of the frame in the video.
    pub at_ms: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Facts about a media file.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    /// The container format, for example "matroska,webm".
    pub container: String,
    pub duration_ms: u64,
    /// The size of the main video stream, or 0 for a file without video.
    pub width: u32,
    pub height: u32,
    pub video_codec: Option<String>,
    pub audio: Vec<AudioTrack>,
}

/// Reads the facts of a media file.
///
/// # Errors
/// Returns an error if the file cannot be opened, and an `InvalidInput` error if it has
/// no audio stream.
pub fn media_info(input: &Path) -> Result<MediaInfo> {
    crate::audio::init()?;
    let audio = list_tracks(input)?;
    if audio.is_empty() {
        bail!(InvalidInput; "{} has no audio", input.display());
    }
    let ictx = open_input(input)?;
    let video = ictx.streams().best(media::Type::Video);
    let (width, height, video_codec) = match &video {
        Some(stream) => {
            let decoder = open_video_decoder(stream)?;
            (
                decoder.width(),
                decoder.height(),
                Some(stream.parameters().id().name().to_string()),
            )
        }
        None => (0, 0, None),
    };
    Ok(MediaInfo {
        container: ictx.format().name().to_string(),
        duration_ms: u64::try_from(ictx.duration().max(0)).unwrap_or(0) / 1000,
        width,
        height,
        video_codec,
        audio,
    })
}

/// Decodes `count` frames, evenly spread over the video without its first and last 5 %,
/// scaled to at most `max_width` pixels wide.
///
/// # Errors
/// Returns an error if the file has no video stream or a frame cannot be decoded.
pub fn preview_frames(input: &Path, count: usize, max_width: u32) -> Result<Vec<PreviewFrame>> {
    crate::audio::init()?;
    let mut ictx = open_input(input)?;
    let stream = ictx
        .streams()
        .best(media::Type::Video)
        .with_context(|| format!("{} has no video", input.display()))?;
    let index = stream.index();
    let time_base = stream.time_base();
    let mut decoder = open_video_decoder(&stream)?;
    let duration_us = ictx.duration().max(0);

    let width = decoder.width().min(max_width.max(2)) & !1;
    let height = (u64::from(decoder.height()) * u64::from(width)
        / u64::from(decoder.width().max(1))) as u32
        & !1;
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        // Seek in microseconds (AV_TIME_BASE), between 5 % and 95 % of the length.
        let at_us = duration_us / 20 + duration_us * 9 / 10 * i as i64 / count.max(1) as i64;
        ictx.seek(at_us, ..at_us)
            .with_kind(ErrorKind::InvalidInput)
            .context("cannot seek in the video")?;
        decoder.flush();
        let target_ms = at_us / 1000;
        let Some(decoded) = next_frame(&mut ictx, &mut decoder, index, time_base, target_ms)?
        else {
            break;
        };
        let mut scaler = Scaler::get(
            decoded.format(),
            decoded.width(),
            decoded.height(),
            Pixel::RGBA,
            width,
            height,
            Flags::BILINEAR,
        )
        .with_kind(ErrorKind::Other)?;
        let mut rgba = frame::Video::empty();
        scaler
            .run(&decoded, &mut rgba)
            .with_kind(ErrorKind::Other)?;
        let at_ms = decoded
            .pts()
            .map_or(target_ms, |pts| rescale_ms(pts, time_base));
        frames.push(PreviewFrame {
            at_ms: u64::try_from(at_ms).unwrap_or(0),
            width,
            height,
            rgba: packed(&rgba, width, height),
        });
    }
    if frames.is_empty() {
        bail!(InvalidInput; "cannot decode a frame of {}", input.display());
    }
    Ok(frames)
}

/// The first frame at or after `target_ms`, in normal memory.
fn next_frame(
    ictx: &mut ffmpeg::format::context::Input,
    decoder: &mut ffmpeg::decoder::Video,
    index: usize,
    time_base: ffmpeg::Rational,
    target_ms: i64,
) -> Result<Option<frame::Video>> {
    let mut decoded = frame::Video::empty();
    for (stream, packet) in ictx.packets() {
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet).with_kind(ErrorKind::Other)?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            let at = decoded
                .pts()
                .map_or(target_ms, |pts| rescale_ms(pts, time_base));
            if at + 1 >= target_ms {
                return to_memory(decoded).map(Some);
            }
        }
    }
    Ok(None)
}

#[cfg(any(target_os = "macos", windows))]
fn to_memory(frame: frame::Video) -> Result<frame::Video> {
    Ok(crate::hwdecode::download(&frame)?.unwrap_or(frame))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn to_memory(frame: frame::Video) -> Result<frame::Video> {
    Ok(frame)
}

/// The RGBA rows without the padding that FFmpeg adds at the end of each row.
fn packed(frame: &frame::Video, width: u32, height: u32) -> Vec<u8> {
    let row = width as usize * 4;
    let stride = frame.stride(0);
    let data = frame.data(0);
    (0..height as usize)
        .flat_map(|y| &data[y * stride..y * stride + row])
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::tests::test_video;

    #[test]
    fn preview_frames_are_scaled_and_spread() {
        let Some(input) = test_video("verbacue-preview.mp4", 10, "1280x720", true) else {
            return;
        };
        let frames = preview_frames(&input, 6, 320).unwrap();
        assert_eq!(frames.len(), 6);
        for f in &frames {
            assert_eq!((f.width, f.height), (320, 180));
            assert_eq!(f.rgba.len(), 320 * 180 * 4);
        }
        assert!(frames.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
    }

    #[test]
    fn media_info_reads_the_test_video() {
        let Some(input) = test_video("verbacue-info.mp4", 10, "1280x720", true) else {
            return;
        };
        let info = media_info(&input).unwrap();
        assert_eq!((info.width, info.height), (1280, 720));
        assert_eq!(info.audio.len(), 1);
        assert!(
            info.duration_ms.abs_diff(10_000) <= 100,
            "{}",
            info.duration_ms
        );
        assert!(info.video_codec.is_some());
    }

    #[test]
    fn media_info_rejects_a_video_without_audio() {
        let Some(input) = test_video("verbacue-silent.mp4", 2, "320x240", false) else {
            return;
        };
        let err = media_info(&input).unwrap_err();
        assert_eq!(err.kind(), verbacue_core::ErrorKind::InvalidInput);
    }
}
