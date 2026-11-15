//! Draws subtitle text with an outline into video frames.

use ab_glyph::{Font, point};
use ffmpeg_next::{Rational, format::Pixel, frame, software::scaling};

use verbacue_core::{
    error::{Context as _, ErrorKind, Result, WithKind as _},
    subtitle::Cue,
};

use super::{
    SubtitleAnimation, SubtitleLook, SubtitleStyle,
    fonts::{Fonts, PlacedGlyph},
    rescale_ms,
};

/// Converts decoded frames to YUV 4:2:0 and draws the active cue into them.
///
/// The frame is always copied through the scaler first. Drawing straight into the
/// decoded frame would change the decoder's reference frames and break later frames.
pub(super) struct Painter<'a> {
    cues: &'a [Cue],
    fonts: Fonts,
    offset_ms: i64,
    time_base: Rational,
    look: SubtitleLook,
    /// The last drawn state and its bitmap. Without an animation, a cue is drawn once.
    cached: Option<(Frame, Overlay)>,
}

impl<'a> Painter<'a> {
    pub(super) fn new(
        cues: &'a [Cue],
        offset_ms: i64,
        time_base: Rational,
        look: SubtitleLook,
    ) -> Result<Self> {
        Ok(Self {
            cues,
            fonts: Fonts::for_cues(cues)?,
            offset_ms,
            time_base,
            look,
            cached: None,
        })
    }

    pub(super) fn paint(
        &mut self,
        decoded: &frame::Video,
        scaler: &mut Option<scaling::Context>,
    ) -> Result<frame::Video> {
        let (w, h) = (decoded.width(), decoded.height());
        let rebuild = scaler.as_ref().is_none_or(|s| {
            s.input().format != decoded.format() || s.input().width != w || s.input().height != h
        });
        if rebuild {
            *scaler = Some(
                scaling::Context::get(
                    decoded.format(),
                    w,
                    h,
                    Pixel::YUV420P,
                    w,
                    h,
                    scaling::Flags::BILINEAR,
                )
                .with_kind(ErrorKind::Other)?,
            );
        }
        let mut out = frame::Video::empty();
        scaler
            .as_mut()
            .context("missing scaler")?
            .run(decoded, &mut out)
            .with_kind(ErrorKind::Other)?;
        let pts = decoded.timestamp();
        out.set_pts(pts);

        if let Some(pts) = pts {
            let ms = rescale_ms(pts, self.time_base) - self.offset_ms;
            if let Some(i) = self.active_cue(ms) {
                let cue = &self.cues[i];
                let since = u64::try_from(ms).unwrap_or(0) - cue.start_ms;
                let state = Frame::at(
                    self.look.animation,
                    i,
                    &cue.text,
                    since,
                    cue.end_ms - cue.start_ms,
                );
                if self
                    .cached
                    .as_ref()
                    .is_none_or(|(c, _)| !c.same_drawing(&state))
                {
                    let overlay = Overlay::render(&self.fonts, &cue.text, w, h, self.look, &state);
                    self.cached = Some((state, overlay));
                }
                if let Some((_, overlay)) = &self.cached {
                    overlay.blend(&mut out, state.opacity);
                }
            }
        }
        Ok(out)
    }

    pub(super) fn active_cue(&self, ms: i64) -> Option<usize> {
        let ms = u64::try_from(ms).ok()?;
        // Cues are sorted and do not overlap, so a binary search finds the candidate.
        let i = self
            .cues
            .partition_point(|c| c.start_ms <= ms)
            .checked_sub(1)?;
        (ms < self.cues[i].end_ms).then_some(i)
    }
}

/// How long Fade takes in and out, and how long Pop grows.
const FADE_MS: u64 = 200;
const POP_MS: u64 = 150;
/// Typewriter writes this many letters per second, faster for a long line, so that the
/// line is complete after 70 % of its time.
const TYPE_CPS: u64 = 25;
/// Karaoke lights the words over this share of the line, in percent.
const KARAOKE_SHARE: u64 = 90;

/// What one frame of a cue shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Frame {
    cue: usize,
    /// 0-255, applied when the bitmap is blended.
    opacity: u8,
    /// The text size in percent of the normal size.
    scale: u8,
    /// The number of letters shown.
    letters: usize,
    /// The number of words lit by Karaoke.
    lit: usize,
}

impl Frame {
    /// The state `since` ms after the start of a cue of `length` ms.
    pub(super) fn at(
        animation: SubtitleAnimation,
        cue: usize,
        text: &str,
        since: u64,
        length: u64,
    ) -> Frame {
        let letters = text.chars().filter(|c| !c.is_whitespace()).count();
        let mut frame = Frame {
            cue,
            opacity: 255,
            scale: 100,
            letters,
            lit: 0,
        };
        let ramp = |ms: u64, of: u64| (ms.min(of) * 255 / of.max(1)) as u8;
        match animation {
            SubtitleAnimation::None => {}
            SubtitleAnimation::Fade => {
                let left = length.saturating_sub(since);
                frame.opacity = ramp(since, FADE_MS).min(ramp(left, FADE_MS));
            }
            SubtitleAnimation::Pop => {
                frame.scale = (70 + since.min(POP_MS) * 30 / POP_MS) as u8;
                frame.opacity = ramp(since, POP_MS / 2);
            }
            SubtitleAnimation::Typewriter => {
                let span = (letters as u64 * 1000 / TYPE_CPS)
                    .min(length * 7 / 10)
                    .max(1);
                frame.letters = (letters as u64 * since.min(span)).div_ceil(span) as usize;
                frame.letters = frame.letters.clamp(1.min(letters), letters);
            }
            SubtitleAnimation::Karaoke => {
                // Each word lights when the share of the letters before it has passed.
                let words: Vec<usize> =
                    text.split_whitespace().map(|w| w.chars().count()).collect();
                let span = (length * KARAOKE_SHARE / 100).max(1);
                let mut before = 0usize;
                for len in &words {
                    let start = before as u64 * span / letters.max(1) as u64;
                    if since >= start {
                        frame.lit += 1;
                    }
                    before += len;
                }
            }
        }
        frame
    }

    /// True if both states give the same bitmap; only the opacity may differ.
    fn same_drawing(&self, o: &Frame) -> bool {
        (self.cue, self.scale, self.letters, self.lit) == (o.cue, o.scale, o.letters, o.lit)
    }
}

/// A colour as Y, U, V (BT.601, limited range).
type Yuv = (u8, u8, u8);
const WHITE: Yuv = (235, 128, 128);
const YELLOW: Yuv = (210, 16, 146);
const BLACK: u8 = 16;
/// The opacity of the box of the Box style.
const BOX_ALPHA: u8 = 170;

/// A rendered cue: the text, the lit words, and the outline or the box, as alpha masks.
pub(super) struct Overlay {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    fill: Vec<u8>,
    /// The part of `fill` that Karaoke has lit.
    lit: Vec<u8>,
    /// The black outline, or the box of the Box style.
    back: Vec<u8>,
    color: Yuv,
    lit_color: Yuv,
}

impl Overlay {
    pub(super) fn render(
        fonts: &Fonts,
        text: &str,
        frame_w: u32,
        frame_h: u32,
        look: SubtitleLook,
        state: &Frame,
    ) -> Self {
        let size = match look.style {
            SubtitleStyle::Large => 0.075,
            _ => 0.055,
        };
        let px = (frame_h as f32 * size * f32::from(state.scale) / 100.0).max(10.0);
        let text_lines: Vec<&str> = text.lines().collect();
        let lines: Vec<(Vec<PlacedGlyph>, f32)> =
            text_lines.iter().map(|l| fonts.shape(l, px)).collect();
        // Line metrics come from the tallest font in the text, so lines drawn with a
        // fallback font (for example CJK) do not touch each other.
        let (ascent, line_height) =
            fonts.metrics(lines.iter().flat_map(|(g, _)| g).map(|g| g.font), px);
        let radius = (px / 14.0).round().max(2.0) as usize;
        let boxed = look.style == SubtitleStyle::Box;
        let pad = if boxed {
            (px * 0.35) as usize
        } else {
            radius + 2
        };

        let max_w = lines.iter().map(|(_, w)| *w).fold(0.0, f32::max);
        let width = (max_w.ceil() as usize + 2 * pad).min(frame_w as usize);
        let height =
            ((line_height * lines.len() as f32).ceil() as usize + 2 * pad).min(frame_h as usize);
        let mut fill = vec![0u8; width * height];
        let mut lit = vec![0u8; width * height];

        // The letter and word number of each byte, over all lines, without spaces.
        let mut letters_before = 0;
        let mut words_before = 0;
        for (n, ((glyphs, line_w), line)) in lines.iter().zip(&text_lines).enumerate() {
            let mut letter_at = vec![usize::MAX; line.len() + 1];
            let mut word_at = vec![usize::MAX; line.len() + 1];
            let mut in_word = false;
            for (b, c) in line.char_indices() {
                if c.is_whitespace() {
                    in_word = false;
                    continue;
                }
                if !in_word {
                    in_word = true;
                    words_before += 1;
                }
                letter_at[b] = letters_before;
                word_at[b] = words_before - 1;
                letters_before += 1;
            }
            let left = (width as f32 - line_w) / 2.0;
            let baseline = pad as f32 + ascent + line_height * n as f32;
            for g in glyphs {
                let letter = letter_at.get(g.byte).copied().unwrap_or(usize::MAX);
                if letter != usize::MAX && letter >= state.letters {
                    continue;
                }
                let is_lit = word_at.get(g.byte).is_some_and(|&w| w < state.lit);
                let loaded = &fonts.0[g.font];
                let font = &loaded.font;
                let glyph =
                    g.id.with_scale_and_position(loaded.px(px), point(left + g.x, baseline - g.y));
                let Some(outlined) = font.outline_glyph(glyph) else {
                    continue;
                };
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px_x = bounds.min.x as i64 + i64::from(gx);
                    let px_y = bounds.min.y as i64 + i64::from(gy);
                    if px_x >= 0 && px_y >= 0 && (px_x as usize) < width && (px_y as usize) < height
                    {
                        let i = px_y as usize * width + px_x as usize;
                        let a = (coverage * 255.0) as u8;
                        fill[i] = fill[i].max(a);
                        if is_lit {
                            lit[i] = lit[i].max(a);
                        }
                    }
                });
            }
        }
        let back = if boxed {
            vec![BOX_ALPHA; width * height]
        } else {
            dilate(&fill, width, height, radius)
        };
        let color = if look.style == SubtitleStyle::Yellow {
            YELLOW
        } else {
            WHITE
        };
        let lit_color = if look.style == SubtitleStyle::Yellow {
            WHITE
        } else {
            YELLOW
        };
        let margin = (frame_h as f32 * 0.06) as usize;
        Self {
            x: (frame_w as usize).saturating_sub(width) / 2,
            y: (frame_h as usize).saturating_sub(height + margin),
            width,
            height,
            fill,
            lit,
            back,
            color,
            lit_color,
        }
    }

    /// Draws the overlay into a YUV 4:2:0 frame: first the black outline or box, then
    /// the text, then the lit words. `opacity` (0-255) scales all of it.
    pub(super) fn blend(&self, frame: &mut frame::Video, opacity: u8) {
        if opacity == 0 {
            return;
        }
        let scale = |a: u8| (u16::from(a) * u16::from(opacity) / 255) as u8;
        let (fw, fh) = (frame.width() as usize, frame.height() as usize);
        let stride = frame.stride(0);
        let luma = frame.data_mut(0);
        for row in 0..self.height {
            let y = self.y + row;
            if y >= fh {
                break;
            }
            for col in 0..self.width {
                let x = self.x + col;
                if x >= fw {
                    break;
                }
                let i = row * self.width + col;
                let p = &mut luma[y * stride + x];
                *p = mix(*p, BLACK, scale(self.back[i]));
                *p = mix(*p, self.color.0, scale(self.fill[i]));
                *p = mix(*p, self.lit_color.0, scale(self.lit[i]));
            }
        }
        for plane in 1..3 {
            let pick = |c: Yuv| if plane == 1 { c.1 } else { c.2 };
            let stride = frame.stride(plane);
            let chroma = frame.data_mut(plane);
            for row in (0..self.height).step_by(2) {
                let y = (self.y + row) / 2;
                if y >= fh.div_ceil(2) {
                    break;
                }
                for col in (0..self.width).step_by(2) {
                    let x = (self.x + col) / 2;
                    if x >= fw.div_ceil(2) {
                        break;
                    }
                    let i = row * self.width + col;
                    let p = &mut chroma[y * stride + x];
                    *p = mix(*p, 128, scale(self.back[i]));
                    *p = mix(*p, pick(self.color), scale(self.fill[i]));
                    *p = mix(*p, pick(self.lit_color), scale(self.lit[i]));
                }
            }
        }
    }
}

/// Grows a mask by `radius` pixels in all directions (a square max filter).
pub(super) fn dilate(mask: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let mut rows = vec![0u8; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(width - 1);
            rows[y * width + x] = mask[y * width + lo..=y * width + hi]
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
        }
    }
    let mut out = vec![0u8; mask.len()];
    for x in 0..width {
        for y in 0..height {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(height - 1);
            out[y * width + x] = (lo..=hi).map(|yy| rows[yy * width + x]).max().unwrap_or(0);
        }
    }
    out
}

pub(super) fn mix(base: u8, color: u8, alpha: u8) -> u8 {
    let a = u16::from(alpha);
    ((u16::from(base) * (255 - a) + u16::from(color) * a) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_ramps_in_and_out() {
        let at = |t| Frame::at(SubtitleAnimation::Fade, 0, "Szia", t, 2000).opacity;
        assert_eq!(at(0), 0);
        assert!(at(100) > 100 && at(100) < 155);
        assert_eq!(at(1000), 255);
        assert!(at(1950) < 80);
    }

    #[test]
    fn pop_grows_to_full_size() {
        let at = |t| Frame::at(SubtitleAnimation::Pop, 0, "Szia", t, 2000).scale;
        assert_eq!(at(0), 70);
        assert_eq!(at(150), 100);
        assert_eq!(at(1500), 100);
    }

    #[test]
    fn typewriter_writes_the_letters_in_turn() {
        // 10 letters at 25 per second: all are there after 400 ms.
        let at = |t| Frame::at(SubtitleAnimation::Typewriter, 0, "Hello world", t, 3000).letters;
        assert_eq!(at(0), 1);
        assert_eq!(at(200), 5);
        assert_eq!(at(400), 10);
        assert_eq!(at(2000), 10);
    }

    #[test]
    fn karaoke_lights_the_words_in_turn() {
        // 3 words of 4, 4 and 2 letters over 90 % of 1000 ms: starts at 0, 360, 720.
        let at = |t| Frame::at(SubtitleAnimation::Karaoke, 0, "abcd efgh ij", t, 1000).lit;
        assert_eq!(at(0), 1);
        assert_eq!(at(359), 1);
        assert_eq!(at(360), 2);
        assert_eq!(at(720), 3);
    }

    #[test]
    fn styles_change_the_drawing() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        let text = "Hello world";
        let full = Frame::at(SubtitleAnimation::None, 0, text, 0, 2000);
        let render = |style, state: &Frame| {
            let look = SubtitleLook {
                style,
                animation: SubtitleAnimation::None,
            };
            Overlay::render(&fonts, text, 640, 360, look, state)
        };
        let classic = render(SubtitleStyle::Classic, &full);
        assert_eq!(classic.color, WHITE);
        assert_eq!(render(SubtitleStyle::Yellow, &full).color, YELLOW);
        // The box covers the whole overlay; an outline does not.
        assert!(render(SubtitleStyle::Box, &full).back.iter().all(|&a| a == BOX_ALPHA));
        assert!(classic.back.contains(&0));
        assert!(render(SubtitleStyle::Large, &full).height > classic.height);
        // Typewriter with 5 letters draws less text; Karaoke lights a part of it.
        let ink = |o: &Overlay| o.fill.iter().filter(|&&a| a > 128).count();
        let five = Frame { letters: 5, ..full };
        assert!(ink(&render(SubtitleStyle::Classic, &five)) < ink(&classic));
        let one = Frame { lit: 1, ..full };
        let lit = render(SubtitleStyle::Classic, &one);
        let lit_ink = lit.lit.iter().filter(|&&a| a > 128).count();
        assert!(lit_ink > 0 && lit_ink < ink(&lit));
    }

    #[test]
    fn dilate_grows_a_point_into_a_square() {
        let mut mask = vec![0u8; 25];
        mask[12] = 255;
        let out = dilate(&mask, 5, 5, 1);
        let lit: Vec<usize> = (0..25).filter(|&i| out[i] == 255).collect();
        assert_eq!(lit, [6, 7, 8, 11, 12, 13, 16, 17, 18]);
    }

    #[test]
    fn mix_blends_between_colors() {
        assert_eq!(mix(0, 200, 0), 0);
        assert_eq!(mix(0, 200, 255), 200);
        assert_eq!(mix(100, 200, 128), 150);
    }

    #[test]
    fn overlay_fits_the_frame_and_has_text_pixels() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        let look = SubtitleLook::default();
        let state = Frame::at(
            SubtitleAnimation::None,
            0,
            "Szia, hol voltál?\nŐ űr",
            0,
            2000,
        );
        let o = Overlay::render(&fonts, "Szia, hol voltál?\nŐ űr", 640, 360, look, &state);
        assert!(o.x + o.width <= 640 && o.y + o.height <= 360);
        assert!(o.fill.iter().any(|&a| a > 200));
        // The outline covers every text pixel and more.
        assert!(o.fill.iter().zip(&o.back).all(|(f, o)| o >= f));
        assert!(
            o.back.iter().filter(|&&a| a > 0).count() > o.fill.iter().filter(|&&a| a > 0).count()
        );
    }
}
