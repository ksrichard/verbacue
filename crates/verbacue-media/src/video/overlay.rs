//! Draws subtitle text with an outline into video frames.

use ab_glyph::{Font, point};
use ffmpeg_next::{Rational, format::Pixel, frame, software::scaling};

use verbacue_core::{
    error::{Context as _, ErrorKind, Result, WithKind as _},
    subtitle::Cue,
};

use verbacue_core::look::{
    BOUNCE_MS, FADE_MS, POP_MS, bounce, display_text, groups, typewriter_span, word_starts,
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
    /// The text of each cue as it shows in the look.
    texts: Vec<String>,
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
        lang: &str,
    ) -> Result<Self> {
        Ok(Self {
            cues,
            texts: cues
                .iter()
                .map(|c| display_text(&c.text, look.style, lang))
                .collect(),
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
                draw_cue(
                    &self.fonts,
                    &mut self.cached,
                    &mut out,
                    self.look,
                    (i, &self.texts[i]),
                    since,
                    cue.end_ms - cue.start_ms,
                );
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

/// Draws cue `cue` (its number and its text in the look) into a YUV 4:2:0 frame,
/// `since` ms after its start. `cached` keeps the last bitmap, so a still cue is drawn
/// only once.
pub(super) fn draw_cue(
    fonts: &Fonts,
    cached: &mut Option<(Frame, Overlay)>,
    out: &mut frame::Video,
    look: SubtitleLook,
    (cue, text): (usize, &str),
    since: u64,
    length: u64,
) {
    let state = Frame::at(look.animation, cue, text, since, length);
    if cached.as_ref().is_none_or(|(c, _)| !c.same_drawing(&state)) {
        let shown = state.shown(text);
        let overlay = Overlay::render(fonts, &shown, out.width(), out.height(), look, &state);
        *cached = Some((state, overlay));
    }
    if let Some((_, overlay)) = cached {
        overlay.blend(out, state.opacity);
    }
}

/// What one frame of a cue shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Frame {
    cue: usize,
    /// 0-255, applied when the bitmap is blended.
    opacity: u8,
    /// The text size in percent of the normal size.
    scale: u8,
    /// The words of the cue that are shown: all of them, or one group of Words.
    words: (usize, usize),
    /// The number of letters shown, counted in the shown words.
    letters: usize,
    /// The lit words, counted in the shown words.
    lit: (usize, usize),
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
        let words: Vec<&str> = text.split_whitespace().collect();
        let lens: Vec<usize> = words.iter().map(|w| w.chars().count()).collect();
        let letters: usize = lens.iter().sum();
        let starts = word_starts(&words, length);
        let mut frame = Frame {
            cue,
            opacity: 255,
            scale: 100,
            words: (0, words.len()),
            letters,
            lit: (0, 0),
        };
        let spoken = starts.iter().filter(|&&s| since >= s).count();
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
            SubtitleAnimation::Bounce => {
                frame.scale = bounce(since);
                frame.opacity = ramp(since, BOUNCE_MS / 3);
            }
            SubtitleAnimation::Typewriter => {
                let span = typewriter_span(letters, length);
                frame.letters = (letters as u64 * since.min(span)).div_ceil(span) as usize;
                frame.letters = frame.letters.clamp(1.min(letters), letters);
            }
            SubtitleAnimation::Karaoke => frame.lit = (0, spoken),
            SubtitleAnimation::Words => {
                let current = spoken.saturating_sub(1);
                if let Some(&(a, b)) = groups(&words)
                    .iter()
                    .find(|(a, b)| (*a..*b).contains(&current))
                {
                    frame.words = (a, b);
                    frame.letters = lens[a..b].iter().sum();
                    frame.lit = (current - a, current - a + 1);
                    frame.scale = bounce(since - starts[a]);
                }
            }
        }
        frame
    }

    pub(super) fn opacity(&self) -> u8 {
        self.opacity
    }

    /// The text of the shown words. All words keep the line breaks of the cue.
    pub(super) fn shown(&self, text: &str) -> String {
        let words: Vec<&str> = text.split_whitespace().collect();
        if self.words == (0, words.len()) {
            text.to_string()
        } else {
            words[self.words.0..self.words.1].join(" ")
        }
    }

    /// True if both states give the same bitmap; only the opacity may differ.
    pub(super) fn same_drawing(&self, o: &Frame) -> bool {
        (self.cue, self.scale, self.words, self.letters, self.lit)
            == (o.cue, o.scale, o.words, o.letters, o.lit)
    }
}

/// A colour as Y, U, V (BT.601, limited range).
type Yuv = (u8, u8, u8);
const WHITE: Yuv = (235, 128, 128);
const YELLOW: Yuv = (210, 16, 146);
const BLACK: Yuv = (16, 128, 128);
/// RGB (40, 200, 70).
const GREEN: Yuv = (134, 95, 67);
/// RGB (255, 40, 160).
const PINK: Yuv = (117, 149, 214);
/// RGB (40, 230, 255).
const CYAN: Yuv = (167, 167, 43);
/// The opacity of the shadow of the short-video styles.
const SHADOW_ALPHA: u16 = 150;
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
    /// The outline, the shadow, the glow, or the box of the Box style.
    back: Vec<u8>,
    /// The boxes behind the lit words of the Highlight style.
    boxes: Vec<u8>,
    back_color: Yuv,
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
        let style = look.style;
        let px = (frame_h as f32 * style.size() * f32::from(state.scale) / 100.0).max(10.0);
        let text_lines: Vec<&str> = text.lines().collect();
        let lines: Vec<(Vec<PlacedGlyph>, f32)> =
            text_lines.iter().map(|l| fonts.shape(l, px)).collect();
        // Line metrics come from the tallest font in the text, so lines drawn with a
        // fallback font (for example CJK) do not touch each other.
        let (ascent, line_height) =
            fonts.metrics(lines.iter().flat_map(|(g, _)| g).map(|g| g.font), px);
        let radius = if style.capitals() {
            (px / 8.0).round().max(2.0) as usize
        } else {
            (px / 14.0).round().max(2.0) as usize
        };
        let shadow = if style.capitals() {
            (px / 14.0) as usize
        } else {
            0
        };
        let pad = match style {
            SubtitleStyle::Box => (px * 0.35) as usize,
            SubtitleStyle::Highlight | SubtitleStyle::Neon => (px * 0.4) as usize,
            _ => radius + shadow + 2,
        };

        let max_w = lines.iter().map(|(_, w)| *w).fold(0.0, f32::max);
        let width = (max_w.ceil() as usize + 2 * pad).min(frame_w as usize);
        let height =
            ((line_height * lines.len() as f32).ceil() as usize + 2 * pad).min(frame_h as usize);
        let mut fill = vec![0u8; width * height];
        let mut lit = vec![0u8; width * height];
        // The left, top, right and bottom of each lit word, for the Highlight boxes.
        let mut lit_rects: Vec<(usize, [f32; 4])> = Vec::new();

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
                let word = word_at.get(g.byte).copied().unwrap_or(usize::MAX);
                let is_lit = (state.lit.0..state.lit.1).contains(&word);
                let loaded = &fonts.0[g.font];
                let font = &loaded.font;
                let glyph =
                    g.id.with_scale_and_position(loaded.px(px), point(left + g.x, baseline - g.y));
                let Some(outlined) = font.outline_glyph(glyph) else {
                    continue;
                };
                let bounds = outlined.px_bounds();
                if is_lit {
                    let top = baseline - ascent;
                    let rect = [bounds.min.x, top, bounds.max.x, top + line_height];
                    match lit_rects.iter_mut().find(|(w, _)| *w == word) {
                        Some((_, r)) => {
                            r[0] = r[0].min(rect[0]);
                            r[2] = r[2].max(rect[2]);
                        }
                        None => lit_rects.push((word, rect)),
                    }
                }
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

        let mut boxes = vec![0u8; width * height];
        let back = match style {
            SubtitleStyle::Box => vec![BOX_ALPHA; width * height],
            SubtitleStyle::Neon => {
                let glow = blur(
                    &dilate(&fill, width, height, radius / 2),
                    width,
                    height,
                    radius * 2,
                );
                glow.iter().map(|&a| a.saturating_mul(2)).collect()
            }
            _ => {
                let outline = dilate(&fill, width, height, radius);
                with_shadow(&outline, width, height, shadow)
            }
        };
        if style == SubtitleStyle::Highlight {
            // Only the spoken word has a box, also when Karaoke has lit the earlier ones.
            let grow = px * 0.14;
            let last = lit_rects.iter().map(|(w, _)| *w).max();
            for &(_, [l, t, r, b]) in lit_rects.iter().filter(|(w, _)| Some(*w) == last) {
                fill_round_rect(
                    &mut boxes,
                    width,
                    height,
                    [l - grow, t, r + grow, b],
                    px * 0.22,
                );
            }
        }
        let (back_color, color, lit_color) = match style {
            SubtitleStyle::Yellow => (BLACK, YELLOW, WHITE),
            SubtitleStyle::Highlight => (BLACK, WHITE, WHITE),
            SubtitleStyle::Neon => (PINK, WHITE, CYAN),
            SubtitleStyle::Bold => (BLACK, WHITE, GREEN),
            _ => (BLACK, WHITE, YELLOW),
        };
        let x = (frame_w as usize).saturating_sub(width) / 2;
        let y = if style.capitals() {
            // Centred at 72 % of the height, where short videos put their captions.
            ((frame_h as f32 * 0.72) as usize)
                .saturating_sub(height / 2)
                .min((frame_h as usize).saturating_sub(height))
        } else {
            let margin = (frame_h as f32 * 0.06) as usize;
            (frame_h as usize).saturating_sub(height + margin)
        };
        Self {
            x,
            y,
            width,
            height,
            fill,
            lit,
            back,
            boxes,
            back_color,
            color,
            lit_color,
        }
    }

    /// Draws the overlay into a YUV 4:2:0 frame: first the outline, the glow or the box,
    /// then the word boxes, the text and the lit words. `opacity` (0-255) scales all.
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
                *p = mix(*p, self.back_color.0, scale(self.back[i]));
                *p = mix(*p, GREEN.0, scale(self.boxes[i]));
                *p = mix(*p, self.color.0, scale(self.fill[i]));
                *p = mix(*p, self.lit_color.0, scale(self.lit[i]));
            }
        }
        for plane in 1..3 {
            let pick = |c: Yuv| if plane == 1 { c.1 } else { c.2 };
            let stride = frame.stride(plane);
            let chroma = frame.data_mut(plane);
            for row in (0..self.height).step_by(2) {
                let y = usize::midpoint(self.y, row);
                if y >= fh.div_ceil(2) {
                    break;
                }
                for col in (0..self.width).step_by(2) {
                    let x = usize::midpoint(self.x, col);
                    if x >= fw.div_ceil(2) {
                        break;
                    }
                    let i = row * self.width + col;
                    let p = &mut chroma[y * stride + x];
                    *p = mix(*p, pick(self.back_color), scale(self.back[i]));
                    *p = mix(*p, pick(GREEN), scale(self.boxes[i]));
                    *p = mix(*p, pick(self.color), scale(self.fill[i]));
                    *p = mix(*p, pick(self.lit_color), scale(self.lit[i]));
                }
            }
        }
    }
}

impl Overlay {
    /// Draws the overlay into a transparent RGBA picture of `frame_w` x `frame_h`, with
    /// the same layers and order as [`Overlay::blend`].
    pub(super) fn to_rgba(&self, frame_w: u32, frame_h: u32, opacity: u8) -> Vec<u8> {
        let (fw, fh) = (frame_w as usize, frame_h as usize);
        let mut out = vec![0u8; fw * fh * 4];
        let layers = [
            (&self.back, rgb(self.back_color)),
            (&self.boxes, rgb(GREEN)),
            (&self.fill, rgb(self.color)),
            (&self.lit, rgb(self.lit_color)),
        ];
        let opacity = f32::from(opacity) / 255.0;
        for row in 0..self.height.min(fh.saturating_sub(self.y)) {
            for col in 0..self.width.min(fw.saturating_sub(self.x)) {
                let i = row * self.width + col;
                // Premultiplied colour and alpha, each layer over the ones before it.
                let (mut c, mut a) = ([0f32; 3], 0f32);
                for (mask, color) in &layers {
                    let la = f32::from(mask[i]) / 255.0 * opacity;
                    for k in 0..3 {
                        c[k] = color[k] * la + c[k] * (1.0 - la);
                    }
                    a = la + a * (1.0 - la);
                }
                let o = ((self.y + row) * fw + self.x + col) * 4;
                for k in 0..3 {
                    out[o + k] = if a > 0.0 { (c[k] / a).round() as u8 } else { 0 };
                }
                out[o + 3] = (a * 255.0).round() as u8;
            }
        }
        out
    }
}

/// Converts a colour to RGB (BT.601, limited range).
fn rgb((y, u, v): Yuv) -> [f32; 3] {
    let l = 1.164 * (f32::from(y) - 16.0);
    let (u, v) = (f32::from(u) - 128.0, f32::from(v) - 128.0);
    [
        (l + 1.596 * v).clamp(0.0, 255.0),
        (l - 0.392 * u - 0.813 * v).clamp(0.0, 255.0),
        (l + 2.017 * u).clamp(0.0, 255.0),
    ]
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

/// Adds a copy of `outline`, moved `offset` pixels right and down, as a shadow.
fn with_shadow(outline: &[u8], width: usize, height: usize, offset: usize) -> Vec<u8> {
    let mut out = outline.to_vec();
    if offset == 0 {
        return out;
    }
    for y in offset..height {
        for x in offset..width {
            let a =
                (u16::from(outline[(y - offset) * width + x - offset]) * SHADOW_ALPHA / 255) as u8;
            let p = &mut out[y * width + x];
            *p = (*p).max(a);
        }
    }
    out
}

/// Averages a mask over a square of `radius` pixels in all directions (a box blur).
fn blur(mask: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let pass = |src: &[u8], len: usize, lines: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![0u8; src.len()];
        for line in 0..lines {
            for i in 0..len {
                let lo = i.saturating_sub(radius);
                let hi = (i + radius).min(len - 1);
                let sum: u32 = (lo..=hi).map(|k| u32::from(src[at(line, k)])).sum();
                out[at(line, i)] = (sum / (2 * radius as u32 + 1)) as u8;
            }
        }
        out
    };
    let rows = pass(mask, width, height, &|y, x| y * width + x);
    pass(&rows, height, width, &|x, y| y * width + x)
}

/// Fills a rectangle with round corners of `radius` into a mask, with soft edges.
fn fill_round_rect(mask: &mut [u8], width: usize, height: usize, rect: [f32; 4], radius: f32) {
    let [l, t, r, b] = rect;
    let y0 = t.floor().max(0.0) as usize;
    let y1 = (b.ceil().max(0.0) as usize).min(height);
    let x0 = l.floor().max(0.0) as usize;
    let x1 = (r.ceil().max(0.0) as usize).min(width);
    for y in y0..y1 {
        for x in x0..x1 {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (l + radius - cx).max(cx - (r - radius)).max(0.0);
            let dy = (t + radius - cy).max(cy - (b - radius)).max(0.0);
            let inside = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let p = &mut mask[y * width + x];
            *p = (*p).max((inside * 255.0) as u8);
        }
    }
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
        assert_eq!(at(0), (0, 1));
        assert_eq!(at(359), (0, 1));
        assert_eq!(at(360), (0, 2));
        assert_eq!(at(720), (0, 3));
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
        assert!(
            render(SubtitleStyle::Box, &full)
                .back
                .iter()
                .all(|&a| a == BOX_ALPHA)
        );
        assert!(classic.back.contains(&0));
        assert!(render(SubtitleStyle::Large, &full).height > classic.height);
        // Typewriter with 5 letters draws less text; Karaoke lights a part of it.
        let ink = |o: &Overlay| o.fill.iter().filter(|&&a| a > 128).count();
        let five = Frame { letters: 5, ..full };
        assert!(ink(&render(SubtitleStyle::Classic, &five)) < ink(&classic));
        let one = Frame {
            lit: (0, 1),
            ..full
        };
        let lit = render(SubtitleStyle::Classic, &one);
        let lit_ink = lit.lit.iter().filter(|&&a| a > 128).count();
        assert!(lit_ink > 0 && lit_ink < ink(&lit));
    }

    #[test]
    fn words_show_one_group_and_light_the_spoken_word() {
        // 6 words of 3 letters over 90 % of 1800 ms: word n starts at n * 270 ms.
        let text = "one two six ten red big";
        let at = |t| Frame::at(SubtitleAnimation::Words, 0, text, t, 1800);
        assert_eq!(at(0).words, (0, 3));
        assert_eq!(at(0).lit, (0, 1));
        assert_eq!(at(0).shown(text), "one two six");
        assert_eq!(at(560).lit, (2, 3));
        assert_eq!(at(820).words, (3, 6));
        assert_eq!(at(820).lit, (0, 1));
        assert_eq!(at(820).scale, 66);
        assert_eq!(at(820).shown(text), "ten red big");
    }

    #[test]
    fn short_video_styles_have_their_parts() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        let text = "HELLO WORLD";
        let one = Frame {
            lit: (1, 2),
            ..Frame::at(SubtitleAnimation::None, 0, text, 0, 2000)
        };
        let render = |style| {
            let look = SubtitleLook {
                style,
                animation: SubtitleAnimation::None,
            };
            Overlay::render(&fonts, text, 640, 360, look, &one)
        };
        let highlight = render(SubtitleStyle::Highlight);
        // The box sits behind the second word only: on the right half.
        let boxed: Vec<usize> = (0..highlight.boxes.len())
            .filter(|&i| highlight.boxes[i] > 128)
            .map(|i| i % highlight.width)
            .collect();
        assert!(!boxed.is_empty() && boxed.iter().all(|&x| x > highlight.width / 2 - 10));
        assert!(render(SubtitleStyle::Bold).boxes.iter().all(|&a| a == 0));
        let neon = render(SubtitleStyle::Neon);
        assert_eq!(neon.back_color, PINK);
        // The short-video styles sit above the bottom margin of the classic style.
        let classic = render(SubtitleStyle::Classic);
        assert!(render(SubtitleStyle::Bold).y + 40 < classic.y);
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
