//! ASS (Advanced SubStation Alpha) files: subtitles with a style and an animation, for
//! players such as VLC and mpv.
//!
//! Each cue is also stored as a `Comment:` line with its plain text. Players skip these
//! lines, and [`parse`] reads them back, so a later run can reuse the file.

use std::fmt::Write as _;

use crate::{
    error::{Context as _, Result},
    look::{
        BOUNCE_LOW, BOUNCE_MS, BOUNCE_TOP, FADE_MS, POP_MS, SubtitleAnimation, SubtitleLook,
        SubtitleStyle, display_text, groups, typewriter_span, word_starts,
    },
    subtitle::Cue,
};

/// The picture size that the sizes in the file refer to. Players scale it to the video.
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

/// Colours as ASS writes them: &HAABBGGRR, where alpha 00 is opaque.
const WHITE: &str = "&H00FFFFFF";
const YELLOW: &str = "&H0000FFFF";
const BLACK: &str = "&H00000000";
const GREEN: &str = "&H0046C828";
const PINK: &str = "&H00A028FF";
const CYAN: &str = "&H00FFE628";
const SHADOW: &str = "&H69000000";
const BOX: &str = "&H55000000";

/// Writes `cues` in the language `lang` as an ASS file in `look`.
pub fn write(cues: &[Cue], lang: &str, look: SubtitleLook) -> String {
    let style = look.style;
    let size = (style.size() * HEIGHT as f32 * 1.25).round() as u32;
    let capitals = style.capitals();
    let (outline, shadow) = if capitals {
        (size / 9, size / 14)
    } else {
        (size / 16, 0)
    };
    let (color, outline_color, back) = match style {
        SubtitleStyle::Yellow => (YELLOW, BLACK, BLACK),
        SubtitleStyle::Box => (WHITE, BOX, BOX),
        SubtitleStyle::Neon => (WHITE, PINK, BLACK),
        SubtitleStyle::Bold | SubtitleStyle::Highlight => (WHITE, BLACK, SHADOW),
        _ => (WHITE, BLACK, BLACK),
    };
    let border_style = if style == SubtitleStyle::Box { 3 } else { 1 };
    // The short-video styles are centred at 72 % of the height, the others sit near
    // the bottom, as in the burned-in picture.
    let margin = if capitals {
        (HEIGHT as f32 * 0.28) as u32 - size / 2
    } else {
        (HEIGHT as f32 * 0.06) as u32
    };

    let mut out = String::new();
    let _ = write!(
        out,
        "[Script Info]\n\
         ScriptType: v4.00+\n\
         PlayResX: {WIDTH}\n\
         PlayResY: {HEIGHT}\n\
         ScaledBorderAndShadow: yes\n\
         WrapStyle: 0\n\n\
         [V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Arial,{size},{color},{color},{outline_color},{back},-1,0,0,0,100,100,\
         0,0,{border_style},{outline},{shadow},2,40,40,{margin},1\n\n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
    );
    for cue in cues {
        let plain = cue.text.replace('\n', "\\N");
        let (start, end) = (time(cue.start_ms), time(cue.end_ms));
        let _ = writeln!(out, "Comment: 0,{start},{end},Default,,0,0,0,,{plain}");
        let text = display_text(&cue.text, style, lang);
        for (from, to, line) in events(&text, cue.end_ms - cue.start_ms, look) {
            let (from, to) = (time(cue.start_ms + from), time(cue.start_ms + to));
            let _ = writeln!(out, "Dialogue: 0,{from},{to},Default,,0,0,0,,{line}");
        }
    }
    out
}

/// The dialogue lines of one cue as (start, end, text), with times from its start.
fn events(text: &str, length: u64, look: SubtitleLook) -> Vec<(u64, u64, String)> {
    let style = look.style;
    let words: Vec<&str> = text.split_whitespace().collect();
    let starts = word_starts(&words, length);
    // The end of word i: the start of the next word, or the end of the cue.
    let word_end = |i: usize| starts.get(i + 1).copied().unwrap_or(length);
    match look.animation {
        SubtitleAnimation::None => vec![(0, length, escape(text))],
        SubtitleAnimation::Fade => vec![(
            0,
            length,
            format!("{{\\fad({FADE_MS},{FADE_MS})}}{}", escape(text)),
        )],
        SubtitleAnimation::Pop => vec![(
            0,
            length,
            format!(
                "{{\\fscx70\\fscy70\\t(0,{POP_MS},\\fscx100\\fscy100)}}{}",
                escape(text)
            ),
        )],
        SubtitleAnimation::Bounce => vec![(0, length, format!("{}{}", bounce(), escape(text)))],
        SubtitleAnimation::Typewriter => {
            let letters = text.chars().filter(|c| !c.is_whitespace()).count();
            let span = typewriter_span(letters, length);
            let mut line = String::new();
            let mut k = 0u64;
            for c in text.chars() {
                if c == '\n' {
                    line.push_str("\\N");
                } else if c.is_whitespace() {
                    line.push(c);
                } else {
                    // Letter k shows when k of the letters have had their time.
                    let at = k * span / letters.max(1) as u64;
                    let _ = write!(line, "{{\\alpha&HFF&\\t({at},{at},\\alpha&H00&)}}");
                    line.push_str(&escape(&c.to_string()));
                    k += 1;
                }
            }
            vec![(0, length, line)]
        }
        SubtitleAnimation::Karaoke => (0..words.len())
            .map(|w| (starts[w], word_end(w), lit_line(&words, 0, w, style)))
            .collect(),
        SubtitleAnimation::Words => {
            let mut out = Vec::new();
            for (a, b) in groups(&words) {
                for (w, &start) in starts.iter().enumerate().take(b).skip(a) {
                    let mut line = lit_line(&words[a..b], w - a, w - a, style);
                    if w == a {
                        line.insert_str(0, &bounce());
                    }
                    out.push((start, word_end(w), line));
                }
            }
            out
        }
    }
}

/// The words with `from..=to` lit: in the lit colour, and the last one on a green
/// box in the Highlight style.
fn lit_line(words: &[&str], from: usize, to: usize, style: SubtitleStyle) -> String {
    let lit = match style {
        SubtitleStyle::Yellow => WHITE,
        SubtitleStyle::Neon => CYAN,
        SubtitleStyle::Bold => GREEN,
        SubtitleStyle::Highlight => WHITE,
        _ => YELLOW,
    };
    let mut line = String::new();
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        let word = escape(word);
        if style == SubtitleStyle::Highlight && i == to {
            // A thick green outline behind one word reads as a box.
            let _ = write!(line, "{{\\3c{}\\bord14}}{word}{{\\r}}", tag(GREEN));
        } else if (from..=to).contains(&i) {
            let _ = write!(line, "{{\\c{}}}{word}{{\\r}}", tag(lit));
        } else {
            line.push_str(&word);
        }
    }
    line
}

/// A style colour (&HAABBGGRR) as a colour tag needs it: &HBBGGRR&.
fn tag(color: &str) -> String {
    format!("&H{}&", &color[4..])
}

/// The tags of a Bounce: grow past the size, then settle.
fn bounce() -> String {
    let half = BOUNCE_MS / 2;
    format!(
        "{{\\fscx{BOUNCE_LOW}\\fscy{BOUNCE_LOW}\\t(0,{half},\\fscx{BOUNCE_TOP}\\fscy{BOUNCE_TOP})\
         \\t({half},{BOUNCE_MS},\\fscx100\\fscy100)}}"
    )
}

/// Text for a dialogue line. Braces start tags in ASS, so they become brackets.
fn escape(text: &str) -> String {
    text.replace('{', "(")
        .replace('}', ")")
        .replace('\n', "\\N")
}

/// Formats ms as `H:MM:SS.cc`.
fn time(ms: u64) -> String {
    let cs = ms / 10;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs / 6000 % 60,
        cs / 100 % 60,
        cs % 100
    )
}

/// Parses `H:MM:SS.cc`.
fn parse_time(text: &str) -> Option<u64> {
    let (hms, cs) = text.trim().split_once('.')?;
    let mut parts = hms.split(':').map(|p| p.parse::<u64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    let cs: u64 = cs.parse().ok()?;
    Some(((h * 60 + m) * 60 + s) * 1000 + cs * 10)
}

/// True if `text` looks like an ASS file.
pub fn is_ass(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("[Script Info]")
}

/// Reads the cues back from the `Comment:` lines that [`write`] stores.
///
/// # Errors
/// Returns an error if a comment line has bad times.
pub fn parse(text: &str) -> Result<Vec<Cue>> {
    let mut cues = Vec::new();
    for line in text.lines() {
        let Some(fields) = line.strip_prefix("Comment:") else {
            continue;
        };
        let parts: Vec<&str> = fields.splitn(10, ',').collect();
        let [_, start, end, .., text] = parts.as_slice() else {
            continue;
        };
        if parts.len() < 10 {
            continue;
        }
        let start = parse_time(start).with_context(|| format!("bad time in {line:?}"))?;
        let end = parse_time(end).with_context(|| format!("bad time in {line:?}"))?;
        cues.push(Cue::new(start, end.max(start), text.replace("\\N", "\n")));
    }
    cues.sort_by_key(|c| c.start_ms);
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(style: SubtitleStyle, animation: SubtitleAnimation) -> SubtitleLook {
        SubtitleLook { style, animation }
    }

    #[test]
    fn writes_and_reads_back_the_cues() {
        let cues = vec![
            Cue::new(1000, 2500, "Hello there\nfriend"),
            Cue::new(3000, 4000, "{odd} text"),
        ];
        let text = write(&cues, "en", SubtitleLook::default());
        assert!(is_ass(&text));
        assert!(
            text.contains("Dialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,Hello there\\Nfriend")
        );
        // Braces would start tags, so the dialogue line has brackets.
        assert!(text.contains(",,(odd) text"));
        assert_eq!(parse(&text).unwrap(), cues);
    }

    #[test]
    fn times_use_centiseconds_and_hours() {
        assert_eq!(time(3_723_456), "1:02:03.45");
        assert_eq!(parse_time("1:02:03.45"), Some(3_723_450));
        assert_eq!(parse_time("bad"), None);
    }

    #[test]
    fn animations_become_tags() {
        let one = |a| events("one two", 1000, look(SubtitleStyle::Classic, a));
        assert!(
            one(SubtitleAnimation::Fade)[0]
                .2
                .starts_with("{\\fad(200,200)}")
        );
        assert!(one(SubtitleAnimation::Pop)[0].2.contains("\\t(0,150,"));
        assert!(one(SubtitleAnimation::Bounce)[0].2.contains("\\fscx115"));
        // Typewriter: each letter has its own time; "one two" has 6 letters.
        assert_eq!(
            one(SubtitleAnimation::Typewriter)[0]
                .2
                .matches("\\alpha&HFF&")
                .count(),
            6
        );
        // Karaoke: one line per word, the earlier words stay lit.
        let karaoke = one(SubtitleAnimation::Karaoke);
        assert_eq!(karaoke.len(), 2);
        assert_eq!((karaoke[1].0, karaoke[1].1), (450, 1000));
        assert_eq!(karaoke[1].2.matches("\\c&H00FFFF&").count(), 2);
    }

    #[test]
    fn words_show_groups_and_highlight_one_word() {
        let lines = events(
            "ONE TWO SIX TEN",
            2000,
            look(SubtitleStyle::Highlight, SubtitleAnimation::Words),
        );
        assert_eq!(lines.len(), 4);
        assert!(lines[0].2.starts_with("{\\fscx60"));
        assert!(lines[0].2.ends_with("TWO SIX"));
        assert!(!lines[1].2.contains("\\fscx60"));
        assert!(lines[3].2.contains("TEN") && !lines[3].2.contains("ONE"));
        assert_eq!(lines[1].2.matches("{\\3c&H46C828&\\bord14}").count(), 1);
    }

    #[test]
    fn styles_change_the_style_line() {
        let style = |s| write(&[], "en", look(s, SubtitleAnimation::None));
        assert!(style(SubtitleStyle::Box).contains(&format!("{BOX},{BOX},-1")));
        assert!(style(SubtitleStyle::Neon).contains(PINK));
        assert!(style(SubtitleStyle::Yellow).contains(&format!("{YELLOW},{YELLOW}")));
    }
}
