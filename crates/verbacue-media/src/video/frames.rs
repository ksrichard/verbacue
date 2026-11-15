//! Frame access for lip sync: a scan of small RGB frames for face finding, and a hook
//! that changes frames while the video is encoded again.

use std::path::Path;

use ffmpeg_next::{format::Pixel, frame, media, software::scaling};
use verbacue_core::{
    Progress, check_cancel,
    error::{Context as _, ErrorKind, Result, WithKind as _},
};

#[cfg(any(target_os = "macos", windows))]
use crate::hwdecode;

use super::{
    audio_start_ms, burn::open_video_decoder, duration_secs, open_input, report, rescale_ms,
};

/// An RGB frame, 3 bytes per pixel, rows without padding.
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// A YUV 4:2:0 frame. `u` and `v` have half the width and half the height (rounded up).
pub struct Yuv {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Changes some frames of a video while [`super::dub`] encodes it again. The times are
/// in ms on the subtitle time line (the first audio sample is 0).
pub trait FrameEditor {
    /// True if the frame at `ms` changes. Other frames are only encoded again.
    fn wants(&self, ms: i64) -> bool;
    /// Changes the frame at `ms`.
    ///
    /// # Errors
    /// An error stops the encoding.
    fn edit(&mut self, ms: i64, frame: &mut Yuv) -> Result<()>;
}

/// Gives `editor` the frame (YUV420P) if it wants it, and writes the result back.
pub(super) fn apply(editor: &mut dyn FrameEditor, ms: i64, out: &mut frame::Video) -> Result<()> {
    if !editor.wants(ms) {
        return Ok(());
    }
    let (width, height) = (out.width() as usize, out.height() as usize);
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let read = |out: &frame::Video, plane: usize, w: usize, h: usize| -> Vec<u8> {
        let stride = out.stride(plane);
        let data = out.data(plane);
        (0..h)
            .flat_map(|r| data[r * stride..r * stride + w].iter().copied())
            .collect()
    };
    let mut yuv = Yuv {
        width,
        height,
        y: read(out, 0, width, height),
        u: read(out, 1, cw, ch),
        v: read(out, 2, cw, ch),
    };
    editor.edit(ms, &mut yuv)?;
    for (plane, values, w, h) in [
        (0, &yuv.y, width, height),
        (1, &yuv.u, cw, ch),
        (2, &yuv.v, cw, ch),
    ] {
        let stride = out.stride(plane);
        let data = out.data_mut(plane);
        for r in 0..h {
            data[r * stride..r * stride + w].copy_from_slice(&values[r * w..(r + 1) * w]);
        }
    }
    Ok(())
}

/// Decodes the video of `input` and gives `f` each frame that `wants` asks for, scaled
/// to `width` pixels wide (same aspect), as RGB. The time is on the subtitle time line.
///
/// # Errors
/// Returns an error if the video cannot be decoded, or the error of `f`.
pub fn scan(
    input: &Path,
    width: u32,
    wants: &dyn Fn(i64) -> bool,
    f: &mut dyn FnMut(i64, &Rgb) -> Result<()>,
    progress: &dyn Progress,
) -> Result<()> {
    crate::audio::init()?;
    let mut ictx = open_input(input)?;
    let ist = ictx
        .streams()
        .best(media::Type::Video)
        .context("the input has no video stream")?;
    let index = ist.index();
    let time_base = ist.time_base();
    let mut decoder = open_video_decoder(&ist)?;
    let offset_ms = audio_start_ms(&ictx);
    let total = duration_secs(&ictx);
    let mut scaler: Option<scaling::Context> = None;
    let mut decoded = frame::Video::empty();
    let mut handle = |decoder: &mut ffmpeg_next::decoder::Video,
                      scaler: &mut Option<scaling::Context>|
     -> Result<()> {
        while decoder.receive_frame(&mut decoded).is_ok() {
            let Some(pts) = decoded.timestamp() else {
                continue;
            };
            let ms = rescale_ms(pts, time_base) - offset_ms;
            report(progress, ms + offset_ms, total);
            if !wants(ms) {
                continue;
            }
            #[cfg(any(target_os = "macos", windows))]
            let downloaded = hwdecode::download(&decoded)?;
            #[cfg(not(any(target_os = "macos", windows)))]
            let downloaded: Option<frame::Video> = None;
            let source = downloaded.as_ref().unwrap_or(&decoded);
            let (w, h) = (source.width(), source.height());
            let out_w = width.min(w).max(2) & !1;
            let out_h =
                ((u64::from(h) * u64::from(out_w) / u64::from(w.max(1))) as u32).max(2) & !1;
            if scaler.as_ref().is_none_or(|s| {
                s.input().format != source.format() || s.input().width != w || s.input().height != h
            }) {
                *scaler = Some(
                    scaling::Context::get(
                        source.format(),
                        w,
                        h,
                        Pixel::RGB24,
                        out_w,
                        out_h,
                        scaling::Flags::BILINEAR,
                    )
                    .with_kind(ErrorKind::Other)?,
                );
            }
            let mut rgb = frame::Video::empty();
            scaler
                .as_mut()
                .context("missing scaler")?
                .run(source, &mut rgb)
                .with_kind(ErrorKind::Other)?;
            let (rw, rh, stride) = (rgb.width() as usize, rgb.height() as usize, rgb.stride(0));
            let data = rgb.data(0);
            let image = Rgb {
                width: rw,
                height: rh,
                data: (0..rh)
                    .flat_map(|r| data[r * stride..r * stride + rw * 3].iter().copied())
                    .collect(),
            };
            f(ms, &image)?;
        }
        Ok(())
    };
    for (stream, packet) in ictx.packets() {
        check_cancel(progress)?;
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet).with_kind(ErrorKind::Other)?;
        handle(&mut decoder, &mut scaler)?;
    }
    decoder.send_eof().with_kind(ErrorKind::Other)?;
    handle(&mut decoder, &mut scaler)
}
