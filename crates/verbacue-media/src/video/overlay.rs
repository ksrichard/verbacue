//! Draws subtitle text with an outline into video frames.

use ab_glyph::{Font, point};
use ffmpeg_next::{Rational, format::Pixel, frame, software::scaling};

use verbacue_core::{
    error::{Context as _, ErrorKind, Result, WithKind as _},
    subtitle::Cue,
};

use super::{
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
    /// The last drawn cue and its bitmap. A cue lasts many frames, so it is drawn once.
    cached: Option<(usize, Overlay)>,
}

impl<'a> Painter<'a> {
    pub(super) fn new(cues: &'a [Cue], offset_ms: i64, time_base: Rational) -> Result<Self> {
        Ok(Self {
            cues,
            fonts: Fonts::for_cues(cues)?,
            offset_ms,
            time_base,
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
                if self.cached.as_ref().is_none_or(|(c, _)| *c != i) {
                    let overlay = Overlay::render(&self.fonts, &self.cues[i].text, w, h);
                    self.cached = Some((i, overlay));
                }
                if let Some((_, overlay)) = &self.cached {
                    overlay.blend(&mut out);
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

/// A rendered cue: white text with a black outline, as two alpha masks.
pub(super) struct Overlay {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    fill: Vec<u8>,
    outline: Vec<u8>,
}

impl Overlay {
    pub(super) fn render(fonts: &Fonts, text: &str, frame_w: u32, frame_h: u32) -> Self {
        let px = (frame_h as f32 * 0.055).max(14.0);
        let lines: Vec<(Vec<PlacedGlyph>, f32)> =
            text.lines().map(|l| fonts.shape(l, px)).collect();
        // Line metrics come from the tallest font in the text, so lines drawn with a
        // fallback font (for example CJK) do not touch each other.
        let (ascent, line_height) =
            fonts.metrics(lines.iter().flat_map(|(g, _)| g).map(|g| g.font), px);
        let radius = (px / 14.0).round().max(2.0) as usize;
        let pad = radius + 2;

        let max_w = lines.iter().map(|(_, w)| *w).fold(0.0, f32::max);
        let width = (max_w.ceil() as usize + 2 * pad).min(frame_w as usize);
        let height =
            ((line_height * lines.len() as f32).ceil() as usize + 2 * pad).min(frame_h as usize);
        let mut fill = vec![0u8; width * height];

        for (n, (glyphs, line_w)) in lines.iter().enumerate() {
            let left = (width as f32 - line_w) / 2.0;
            let baseline = pad as f32 + ascent + line_height * n as f32;
            for g in glyphs {
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
                        fill[i] = fill[i].max((coverage * 255.0) as u8);
                    }
                });
            }
        }
        let outline = dilate(&fill, width, height, radius);
        let margin = (frame_h as f32 * 0.06) as usize;
        Self {
            x: (frame_w as usize).saturating_sub(width) / 2,
            y: (frame_h as usize).saturating_sub(height + margin),
            width,
            height,
            fill,
            outline,
        }
    }

    /// Draws the overlay into a YUV 4:2:0 frame: first the black outline, then the
    /// white text. The chroma planes move toward gray where the text is.
    pub(super) fn blend(&self, frame: &mut frame::Video) {
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
                *p = mix(*p, 16, self.outline[i]);
                *p = mix(*p, 235, self.fill[i]);
            }
        }
        for plane in 1..3 {
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
                    let alpha = self.outline[row * self.width + col];
                    let p = &mut chroma[y * stride + x];
                    *p = mix(*p, 128, alpha);
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
        let o = Overlay::render(&fonts, "Szia, hol voltál?\nŐ űr", 640, 360);
        assert!(o.x + o.width <= 640 && o.y + o.height <= 360);
        assert!(o.fill.iter().any(|&a| a > 200));
        // The outline covers every text pixel and more.
        assert!(o.fill.iter().zip(&o.outline).all(|(f, o)| o >= f));
        assert!(
            o.outline.iter().filter(|&&a| a > 0).count()
                > o.fill.iter().filter(|&&a| a > 0).count()
        );
    }
}
