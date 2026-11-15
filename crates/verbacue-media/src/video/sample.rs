//! A short subtitle sample that loops, so that the settings can show a style and an
//! animation before a video is burned.

use ffmpeg_next::{format::Pixel, frame};

use verbacue_core::error::Result;

use super::{
    PreviewFrame, SubtitleLook,
    fonts::Fonts,
    overlay::{Frame, Overlay, cue_state, draw_cue},
};

/// The sample lines as (start, end, text), in ms from the start of the loop.
const LINES: [(u64, u64, &str); 2] = [
    (300, 3000, "This is how your subtitles look"),
    (3300, 6000, "Pick a style and an animation"),
];
/// The length of the loop.
pub const SAMPLE_LOOP_MS: u64 = 6600;

/// Draws the sample lines on a dark picture.
pub struct SubtitleSample {
    fonts: Fonts,
    background: frame::Video,
    cached: Option<(Frame, Overlay)>,
    /// What the last picture showed, so that a still picture is not made again.
    last: Option<(SubtitleLook, Option<(Frame, u8)>)>,
}

impl SubtitleSample {
    /// A sample of `width` x `height` pixels. Both are rounded down to even numbers.
    ///
    /// # Errors
    /// Returns an error if the built-in font cannot be loaded.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let (width, height) = ((width & !1).max(2), (height & !1).max(2));
        Ok(Self {
            fonts: Fonts::for_cues(&[])?,
            background: background(width, height),
            cached: None,
            last: None,
        })
    }

    /// The picture at `ms` (any value; the sample loops), or `None` if it is the same
    /// as the picture of the last call.
    pub fn frame(&mut self, look: SubtitleLook, ms: u64) -> Option<PreviewFrame> {
        let at = ms % SAMPLE_LOOP_MS;
        let line = LINES
            .iter()
            .enumerate()
            .find(|(_, (start, end, _))| (*start..*end).contains(&at));
        let state = line.map(|(i, (start, end, text))| {
            let (_, frame) = cue_state(look, i, text, at - start, end - start);
            (frame, frame.opacity())
        });
        if self.last == Some((look, state)) {
            return None;
        }
        if self.last.is_some_and(|(l, _)| l != look) {
            self.cached = None;
        }
        self.last = Some((look, state));

        let mut out = self.background.clone();
        if let Some((i, (start, end, text))) = line {
            draw_cue(
                &self.fonts,
                &mut self.cached,
                &mut out,
                look,
                (i, text),
                at - start,
                end - start,
            );
        }
        Some(to_rgba(&out, at))
    }
}

/// A dark blue picture, lighter towards the bottom, like a night scene.
fn background(width: u32, height: u32) -> frame::Video {
    let mut f = frame::Video::new(Pixel::YUV420P, width, height);
    let (w, h) = (width as usize, height as usize);
    let stride = f.stride(0);
    for y in 0..h {
        let luma = (40 + y * 60 / h) as u8;
        f.data_mut(0)[y * stride..y * stride + w].fill(luma);
    }
    for (plane, value) in [(1, 140u8), (2, 120u8)] {
        let stride = f.stride(plane);
        for y in 0..h.div_ceil(2) {
            f.data_mut(plane)[y * stride..y * stride + w.div_ceil(2)].fill(value);
        }
    }
    f
}

/// Converts a YUV 4:2:0 frame (BT.601, limited range) to RGBA.
fn to_rgba(f: &frame::Video, at_ms: u64) -> PreviewFrame {
    let (w, h) = (f.width() as usize, f.height() as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let luma = f32::from(f.data(0)[y * f.stride(0) + x]) - 16.0;
            let u = f32::from(f.data(1)[y / 2 * f.stride(1) + x / 2]) - 128.0;
            let v = f32::from(f.data(2)[y / 2 * f.stride(2) + x / 2]) - 128.0;
            let l = 1.164 * luma;
            let clamp = |c: f32| c.round().clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[
                clamp(l + 1.596 * v),
                clamp(l - 0.392 * u - 0.813 * v),
                clamp(l + 2.017 * u),
                255,
            ]);
        }
    }
    PreviewFrame {
        at_ms,
        width: w as u32,
        height: h as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::{SubtitleAnimation, SubtitleStyle};

    #[test]
    fn draws_the_line_and_skips_still_pictures() {
        let mut sample = SubtitleSample::new(320, 180).unwrap();
        let look = SubtitleLook::default();
        let empty = sample.frame(look, 0).unwrap();
        assert_eq!((empty.width, empty.height), (320, 180));
        let line = sample.frame(look, 1000).unwrap();
        // White text pixels appear only with the line.
        let white = |f: &PreviewFrame| f.rgba.chunks(4).filter(|p| p[0] > 230).count();
        assert_eq!(white(&empty), 0);
        assert!(white(&line) > 50);
        // Without an animation the line does not change, so there is no new picture.
        assert!(sample.frame(look, 1500).is_none());
        // A new look gives a new picture at once.
        let bold = SubtitleLook {
            style: SubtitleStyle::Bold,
            animation: SubtitleAnimation::None,
        };
        assert!(sample.frame(bold, 1500).is_some());
        // The sample loops.
        assert!(sample.frame(bold, SAMPLE_LOOP_MS).is_some());
    }
}
