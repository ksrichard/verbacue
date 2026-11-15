//! A short subtitle sample that loops, so that the app can show a style and an
//! animation before a video is burned.

use verbacue_core::{error::Result, look::display_text};

use super::{
    PreviewFrame, SubtitleLook,
    fonts::Fonts,
    overlay::{Frame, Overlay},
};

/// The sample lines as (start, end, text), in ms from the start of the loop.
const LINES: [(u64, u64, &str); 2] = [
    (300, 3000, "This is how your subtitles look"),
    (3300, 6000, "Pick a style and an animation"),
];
/// The length of the loop.
pub const SAMPLE_LOOP_MS: u64 = 6600;

/// Draws the sample lines on a transparent picture, to put over a video frame.
pub struct SubtitleSample {
    fonts: Fonts,
    width: u32,
    height: u32,
    cached: Option<(Frame, Overlay)>,
    /// What the last picture showed, so that a still picture is not made again.
    last: Option<(SubtitleLook, Option<(Frame, u8)>)>,
}

impl SubtitleSample {
    /// A sample of `width` x `height` pixels. The text size follows the height, as on
    /// a video of this size.
    ///
    /// # Errors
    /// Returns an error if the built-in font cannot be loaded.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            fonts: Fonts::for_cues(&[])?,
            width: width.max(2),
            height: height.max(2),
            cached: None,
            last: None,
        })
    }

    /// Makes the next call draw a picture, also if nothing changed.
    pub fn forget(&mut self) {
        self.last = None;
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The RGBA picture at `ms` (any value; the sample loops), or `None` if it is the
    /// same as the picture of the last call.
    pub fn frame(&mut self, look: SubtitleLook, ms: u64) -> Option<PreviewFrame> {
        let at = ms % SAMPLE_LOOP_MS;
        let line = LINES
            .iter()
            .enumerate()
            .find(|(_, (start, end, _))| (*start..*end).contains(&at));
        let text = line.map(|(_, (_, _, text))| display_text(text, look.style, "en"));
        let state = line
            .zip(text.as_deref())
            .map(|((i, (start, end, _)), text)| {
                let frame = Frame::at(look.animation, i, text, at - start, end - start);
                (frame, frame.opacity())
            });
        if self.last == Some((look, state)) {
            return None;
        }
        if self.last.is_some_and(|(l, _)| l != look) {
            self.cached = None;
        }
        self.last = Some((look, state));

        let rgba = match (text, state) {
            (Some(text), Some((frame, opacity))) => {
                if self
                    .cached
                    .as_ref()
                    .is_none_or(|(c, _)| !c.same_drawing(&frame))
                {
                    let shown = frame.shown(&text);
                    let overlay =
                        Overlay::render(&self.fonts, &shown, self.width, self.height, look, &frame);
                    self.cached = Some((frame, overlay));
                }
                let (_, overlay) = self.cached.as_ref()?;
                overlay.to_rgba(self.width, self.height, opacity)
            }
            _ => vec![0; (self.width * self.height * 4) as usize],
        };
        Some(PreviewFrame {
            at_ms: at,
            width: self.width,
            height: self.height,
            rgba,
        })
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
        assert!(empty.rgba.iter().all(|&b| b == 0));
        let line = sample.frame(look, 1000).unwrap();
        // Opaque white text pixels and a black outline appear only with the line.
        let white = line
            .rgba
            .chunks(4)
            .filter(|p| p[3] > 250 && p[0] > 230)
            .count();
        let black = line
            .rgba
            .chunks(4)
            .filter(|p| p[3] > 250 && p[0] < 30)
            .count();
        assert!(white > 50 && black > 50);
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
