//! Finds fonts for the subtitle text and shapes the text into placed glyphs.

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};
use rustybuzz::{Direction, UnicodeBuffer, ttf_parser};
use unicode_bidi::BidiInfo;

use verbacue_core::{
    error::{Context as _, ErrorKind, Result, WithKind as _, bail},
    subtitle::Cue,
};

/// DejaVu Sans Bold (free license, see assets/DejaVu-LICENSE.txt). It covers Latin,
/// Greek and Cyrillic, including the Hungarian letters ő and ű.
pub(super) const FONT: &[u8] = include_bytes!("../../assets/DejaVuSans-Bold.ttf");

/// System fonts for scripts that the embedded font does not cover: Chinese, Japanese,
/// Korean, Arabic, Indic, Thai and others. verbacue loads a file only if the subtitles
/// need it, and picks the boldest upright face in a font collection (.ttc).
pub(super) const FALLBACK_FONTS: &[&str] = &[
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/GeezaPro.ttc",
    "/System/Library/Fonts/ArialHB.ttc",
    "/System/Library/Fonts/Kohinoor.ttc",
    "/System/Library/Fonts/KohinoorBangla.ttc",
    "/System/Library/Fonts/KohinoorGujarati.ttc",
    "/System/Library/Fonts/KohinoorTelugu.ttc",
    "/System/Library/Fonts/MuktaMahee.ttc",
    "/System/Library/Fonts/NotoSansKannada.ttc",
    "/System/Library/Fonts/NotoSansOriya.ttc",
    "/System/Library/Fonts/NotoSansMyanmar.ttc",
    "/System/Library/Fonts/Supplemental/Tamil Sangam MN.ttc",
    "/System/Library/Fonts/Supplemental/Malayalam Sangam MN.ttc",
    "/System/Library/Fonts/Supplemental/Sinhala Sangam MN.ttc",
    "/System/Library/Fonts/Supplemental/Thonburi.ttc",
    "/System/Library/Fonts/Supplemental/Lao Sangam MN.ttf",
    "/System/Library/Fonts/Supplemental/Khmer Sangam MN.ttf",
    "/System/Library/Fonts/Supplemental/Kailasa.ttc",
    "/System/Library/Fonts/Supplemental/KefaIII.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

/// Font files in the Windows font folder, for the same scripts. Windows 10 and 11
/// have all of them.
pub(super) const WINDOWS_FONTS: &[&str] = &[
    "YuGothB.ttc",
    "msyhbd.ttc",
    "malgunbd.ttf",
    "NirmalaB.ttf",
    "Nirmala.ttc",
    "LeelaUIb.ttf",
    "mmrtextb.ttf",
    "ebrimabd.ttf",
    "himalaya.ttf",
];

/// Folders and file names of the Noto fonts on Linux (Debian, Arch and Fedora).
pub(super) const NOTO_DIRS: &[&str] = &[
    "/usr/share/fonts/truetype/noto",
    "/usr/share/fonts/noto",
    "/usr/share/fonts/google-noto",
];
pub(super) const NOTO_FILES: &[&str] = &[
    "Arabic",
    "Hebrew",
    "Devanagari",
    "Bengali",
    "Gujarati",
    "Gurmukhi",
    "Tamil",
    "Telugu",
    "Kannada",
    "Malayalam",
    "Oriya",
    "Sinhala",
    "Thai",
    "Lao",
    "Khmer",
    "Myanmar",
    "Ethiopic",
];

/// A glyph after shaping. `x` is from the start of the line and `y` is up from the
/// baseline, both in pixels.
pub(super) struct PlacedGlyph {
    pub(super) font: usize,
    pub(super) id: GlyphId,
    pub(super) x: f32,
    pub(super) y: f32,
}

pub(super) struct LoadedFont {
    pub(super) font: FontVec,
    /// The face index in a font collection, for the shaper.
    pub(super) index: u32,
    /// ab_glyph scales a font by its line height. Many Indic and Thai fonts have very
    /// tall lines, so their letters look small. This factor gives all fonts the same
    /// em size as the main font.
    pub(super) scale: f32,
}

impl LoadedFont {
    pub(super) fn new(font: FontVec, index: u32, main: Option<&FontVec>) -> Self {
        let em_ratio = |f: &FontVec| f.height_unscaled() / f.units_per_em().unwrap_or(1000.0);
        let scale = main.map_or(1.0, |m| em_ratio(&font) / em_ratio(m));
        Self { font, index, scale }
    }

    pub(super) fn px(&self, px: f32) -> PxScale {
        PxScale::from(px * self.scale)
    }
}

/// The embedded font first, then system fonts for characters it does not have.
pub(super) struct Fonts(pub(super) Vec<LoadedFont>);

impl Fonts {
    /// Loads the embedded font, and the system fonts that the cues need.
    ///
    /// # Errors
    /// Returns an error if no font has some letters of the cues, because those would
    /// be blank in the video.
    pub(super) fn for_cues(cues: &[Cue]) -> Result<Self> {
        let main = FontVec::try_from_vec(FONT.to_vec())
            .with_kind(ErrorKind::Other)
            .context("the embedded font is invalid")?;
        let mut fonts = Self(vec![LoadedFont::new(main, 0, None)]);
        let mut missing = fonts.missing(cues);
        let noto = NOTO_DIRS.iter().flat_map(|dir| {
            NOTO_FILES.iter().flat_map(move |name| {
                ["Bold", "Regular"].map(|w| format!("{dir}/NotoSans{name}-{w}.ttf"))
            })
        });
        let tibetan = NOTO_DIRS
            .iter()
            .map(|dir| format!("{dir}/NotoSerifTibetan-Bold.ttf"));
        // WINDIR is set only on Windows.
        let windows = std::env::var("WINDIR").into_iter().flat_map(|dir| {
            WINDOWS_FONTS
                .iter()
                .map(move |name| format!("{dir}\\Fonts\\{name}"))
        });
        let candidates = FALLBACK_FONTS
            .iter()
            .map(|p| (*p).to_string())
            .chain(windows)
            .chain(noto)
            .chain(tibetan);
        for path in candidates {
            if missing.is_empty() {
                break;
            }
            // Skip font files that do not exist or cannot be read.
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let index = boldest_face(&data);
            let Ok(font) = FontVec::try_from_vec_and_index(data, index) else {
                continue;
            };
            if missing.iter().any(|&c| font.glyph_id(c).0 != 0) {
                tracing::debug!("using fallback font {path}");
                let loaded = LoadedFont::new(font, index, Some(&fonts.0[0].font));
                fonts.0.push(loaded);
                missing = fonts.missing(cues);
            }
        }
        let letters: String = missing.iter().filter(|c| c.is_alphabetic()).collect();
        if !letters.is_empty() {
            let sample: String = letters.chars().take(20).collect();
            bail!(
                "no font on this system has these letters: {sample}; install a font for \
                 this script (for example Noto Sans), or use --embed instead"
            );
        }
        if !missing.is_empty() {
            let sample: String = missing.iter().take(20).collect();
            tracing::warn!("no font has these characters, they will be blank: {sample}");
        }
        Ok(fonts)
    }

    /// The characters of `cues` that no loaded font has.
    pub(super) fn missing(&self, cues: &[Cue]) -> Vec<char> {
        let mut chars: Vec<char> = cues
            .iter()
            .flat_map(|c| c.text.chars())
            .filter(|&c| !c.is_whitespace() && !is_format_char(c) && self.pick(c).1 == GlyphId(0))
            .collect();
        chars.sort_unstable();
        chars.dedup();
        chars
    }

    /// The largest ascent and line height among the main font and `used` fonts.
    pub(super) fn metrics(&self, used: impl Iterator<Item = usize>, px: f32) -> (f32, f32) {
        let mut used: Vec<usize> = std::iter::once(0).chain(used).collect();
        used.sort_unstable();
        used.dedup();
        used.iter().fold((0.0f32, 0.0f32), |(ascent, height), &i| {
            let scaled = self.0[i].font.as_scaled(self.0[i].px(px));
            (
                ascent.max(scaled.ascent()),
                height.max(scaled.height() + scaled.line_gap()),
            )
        })
    }

    /// The first font that has a glyph for `c`, and that glyph. If no font has it, the
    /// main font and glyph 0 (the empty "missing" glyph).
    pub(super) fn pick(&self, c: char) -> (usize, GlyphId) {
        self.0
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f.font.glyph_id(c)))
            .find(|(_, id)| id.0 != 0)
            .unwrap_or((0, GlyphId(0)))
    }

    /// Lays out one line: it puts mixed left-to-right and right-to-left text in visual
    /// order, splits it by font, and shapes each part. Shaping joins Arabic letters,
    /// builds Indic syllables and applies kerning. It returns the glyphs and the width.
    pub(super) fn shape(&self, line: &str, px: f32) -> (Vec<PlacedGlyph>, f32) {
        let bidi = BidiInfo::new(line, None);
        let mut glyphs = Vec::new();
        let mut x = 0.0;
        for para in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(para, para.range.clone());
            for run in runs {
                let rtl = levels[run.start].is_rtl();
                let mut parts = self.font_runs(&line[run.clone()]);
                if rtl {
                    parts.reverse();
                }
                for (font, range) in parts {
                    let text = &line[run.start + range.start..run.start + range.end];
                    x = self.shape_run(font, text, rtl, px, x, &mut glyphs);
                }
            }
        }
        (glyphs, x)
    }

    /// Splits text into parts that use one font each. A non-ASCII character stays in
    /// the font of the character before it if that font has it. This keeps combining
    /// marks and joiners with their letter, so the shaper sees the whole syllable.
    pub(super) fn font_runs(&self, text: &str) -> Vec<(usize, std::ops::Range<usize>)> {
        let mut runs: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
        for (i, c) in text.char_indices() {
            let end = i + c.len_utf8();
            let font = match runs.last() {
                Some((prev, _))
                    if is_format_char(c)
                        || (!c.is_ascii() && self.0[*prev].font.glyph_id(c).0 != 0) =>
                {
                    *prev
                }
                _ => self.pick(c).0,
            };
            match runs.last_mut() {
                Some((prev, range)) if *prev == font => range.end = end,
                _ => runs.push((font, i..end)),
            }
        }
        runs
    }

    /// Shapes `text` with one font and appends the glyphs, starting at `x`. It returns
    /// the `x` after the last glyph.
    pub(super) fn shape_run(
        &self,
        font: usize,
        text: &str,
        rtl: bool,
        px: f32,
        mut x: f32,
        out: &mut Vec<PlacedGlyph>,
    ) -> f32 {
        let loaded = &self.0[font];
        let Some(face) = rustybuzz::Face::from_slice(loaded.font.as_slice(), loaded.index) else {
            return x;
        };
        let scaled = loaded.font.as_scaled(loaded.px(px));
        let (sx, sy) = (scaled.h_scale_factor(), scaled.v_scale_factor());
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_direction(if rtl {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        });
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&face, &[], buffer);
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            out.push(PlacedGlyph {
                font,
                id: GlyphId(u16::try_from(info.glyph_id).unwrap_or(0)),
                x: x + pos.x_offset as f32 * sx,
                y: pos.y_offset as f32 * sy,
            });
            x += pos.x_advance as f32 * sx;
        }
        x
    }
}

/// Invisible characters that control joining and direction (ZWJ, ZWNJ, marks).
pub(super) fn is_format_char(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// The index of the boldest upright face in a font file. A plain font file has one
/// face, index 0.
pub(super) fn boldest_face(data: &[u8]) -> u32 {
    let count = ttf_parser::fonts_in_collection(data).unwrap_or(1);
    (0..count)
        .filter_map(|i| ttf_parser::Face::parse(data, i).ok().map(|f| (i, f)))
        .filter(|(_, f)| !f.is_italic())
        .min_by_key(|(_, f)| (i32::from(f.weight().to_number()) - 700).abs())
        .map_or(0, |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_covers_latin_greek_and_cyrillic() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        for c in "ŐőŰűÁéíóúüöÄßçñøłşğΩПривет".chars() {
            assert_ne!(fonts.pick(c).1, GlyphId(0), "missing {c}");
        }
    }

    #[test]
    fn shapes_arabic_into_joined_forms() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        // Lam and alef join into one ligature glyph.
        let (glyphs, _) = fonts.shape("لا", 30.0);
        assert_eq!(glyphs.len(), 1);
        // A letter between two others takes its middle form, not the alone form.
        let (glyphs, _) = fonts.shape("بيت", 30.0);
        let alone = fonts.0[0].font.glyph_id('ي');
        assert_eq!(glyphs.len(), 3);
        assert!(glyphs.iter().all(|g| g.id != alone));
    }

    #[test]
    fn puts_right_to_left_text_in_visual_order() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        // Hebrew is drawn from right to left: the first letter (alef) is on the right.
        let (glyphs, width) = fonts.shape("אב 12", 30.0);
        let font = &fonts.0[0].font;
        let alef = glyphs.iter().find(|g| g.id == font.glyph_id('א')).unwrap();
        let bet = glyphs.iter().find(|g| g.id == font.glyph_id('ב')).unwrap();
        assert!(alef.x > bet.x);
        // Digits stay left to right.
        let one = glyphs.iter().find(|g| g.id == font.glyph_id('1')).unwrap();
        let two = glyphs.iter().find(|g| g.id == font.glyph_id('2')).unwrap();
        assert!(one.x < two.x);
        assert!(width > 0.0);
    }

    #[test]
    fn keeps_combining_marks_with_their_letter() {
        let fonts = Fonts::for_cues(&[]).unwrap();
        // "e" plus a combining acute accent: one font run, the mark on the letter.
        let runs = fonts.font_runs("e\u{301}x");
        assert_eq!(runs.len(), 1);
    }
}
