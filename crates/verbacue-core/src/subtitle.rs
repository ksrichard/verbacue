//! Subtitle cues: SRT/VTT read and write, cue splitting, line wrapping and timing fixes.
//!
//! All times are integer milliseconds. Lengths are counted in characters, not bytes, so
//! accented letters (for example in Hungarian) count as one.

use crate::error::{Context as _, Result, bail};

/// Longest cue that [`split_long`] keeps in one piece.
pub const MAX_CUE_MS: u64 = 6_000;
/// Widest cue text, in columns, that [`split_long`] keeps in one piece (two lines of
/// 42). A CJK character takes two columns.
pub const MAX_CUE_CHARS: usize = 84;
pub const MAX_LINE_CHARS: usize = 42;
pub const MIN_CUE_MS: u64 = 800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl Cue {
    pub fn new(start_ms: u64, end_ms: u64, text: impl Into<String>) -> Self {
        Self {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Format {
    Srt,
    Vtt,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }
}

/// Formats milliseconds as `HH:MM:SS,mmm` (SRT) or `HH:MM:SS.mmm` (VTT).
pub fn format_timestamp(ms: u64, format: Format) -> String {
    let sep = match format {
        Format::Srt => ',',
        Format::Vtt => '.',
    };
    let (h, m, s, frac) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h:02}:{m:02}:{s:02}{sep}{frac:03}")
}

/// Parses `HH:MM:SS,mmm`. It also accepts `.` as the separator and a missing hours part.
///
/// # Errors
/// Returns an error if the text is not a valid timestamp.
pub fn parse_timestamp(text: &str) -> Result<u64> {
    let text = text.trim();
    let (clock, frac) = text.split_once([',', '.']).unwrap_or((text, "0"));
    let parts: Vec<&str> = clock.split(':').collect();
    let nums = parts
        .iter()
        .map(|p| p.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("bad timestamp {text:?}"))?;
    let (h, m, s) = match nums.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => bail!("bad timestamp {text:?}"),
    };
    // Pad or cut the fraction to exactly 3 digits, so ",5" is 500 ms.
    let frac: String = frac.chars().chain("000".chars()).take(3).collect();
    let frac: u64 = frac
        .parse()
        .with_context(|| format!("bad timestamp {text:?}"))?;
    Ok(((h * 60 + m) * 60 + s) * 1000 + frac)
}

/// Parses SRT text. It accepts a UTF-8 BOM, CRLF line ends and missing cue numbers.
///
/// # Errors
/// Returns an error if a timing line is malformed.
pub fn parse_srt(text: &str) -> Result<Vec<Cue>> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n").map(str::trim).filter(|b| !b.is_empty()) {
        let mut lines = block.lines();
        let mut timing = lines.next().unwrap_or_default();
        if !timing.contains("-->") {
            timing = lines.next().unwrap_or_default();
        }
        let (start, end) = timing
            .split_once("-->")
            .with_context(|| format!("cue without a timing line: {block:?}"))?;
        // VTT-style cue settings can follow the end time; keep only the time.
        let end = end.split_whitespace().next().unwrap_or_default();
        let body: Vec<&str> = lines.collect();
        cues.push(Cue::new(
            parse_timestamp(start)?,
            parse_timestamp(end)?,
            body.join("\n"),
        ));
    }
    Ok(cues)
}

/// Writes cues as SRT or WebVTT. Cue numbers start at 1.
pub fn write(cues: &[Cue], format: Format) -> String {
    let mut out = String::new();
    if format == Format::Vtt {
        out.push_str("WEBVTT\n\n");
    }
    for (i, cue) in cues.iter().enumerate() {
        let start = format_timestamp(cue.start_ms, format);
        let end = format_timestamp(cue.end_ms, format);
        out.push_str(&format!("{}\n{start} --> {end}\n{}\n\n", i + 1, cue.text));
    }
    out
}

/// Whisper often ends a segment in the middle of a sentence. This joins a cue with the
/// next one when the cue does not end a sentence and the pause between them is short.
/// [`split_long`] then cuts the result at real sentence ends. A joined cue never grows
/// past `max_chars`, so text without punctuation does not become one huge cue.
pub fn merge_unfinished(cues: &[Cue], max_gap_ms: u64, max_chars: usize) -> Vec<Cue> {
    let mut out: Vec<Cue> = Vec::with_capacity(cues.len());
    for cue in cues {
        let text = normalize_space(&cue.text);
        if let Some(last) = out.last_mut()
            && !ends_sentence(&last.text)
            && cue.start_ms.saturating_sub(last.end_ms) <= max_gap_ms
            && last.text.chars().count() + 1 + text.chars().count() <= max_chars
        {
            last.text = format!("{} {text}", last.text);
            last.end_ms = cue.end_ms;
            continue;
        }
        out.push(Cue {
            text,
            ..cue.clone()
        });
    }
    out
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', '”', '’', ')', '»'])
        .ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

/// Splits cues that are longer than `max_ms` or wider than `max_width` columns.
///
/// It cuts at sentence punctuation, then at clause punctuation, then at any break
/// point. Each part gets a share of the time that matches its share of the width.
pub fn split_long(cues: &[Cue], max_ms: u64, max_width: usize) -> Vec<Cue> {
    let mut out = Vec::new();
    for cue in cues {
        split_into(cue.clone(), max_ms, max_width, &mut out);
    }
    out
}

fn split_into(cue: Cue, max_ms: u64, max_width: usize, out: &mut Vec<Cue>) {
    let text = normalize_space(&cue.text);
    let chars: Vec<char> = text.chars().collect();
    // Text that fits on one line is not split for time. The parts would be single words
    // that stay on screen for seconds each.
    let too_wide = width(&chars) > max_width;
    let too_slow = cue.end_ms - cue.start_ms > max_ms && width(&chars) > max_width / 2;
    let too_long = too_wide || too_slow;
    let Some((at, skip)) = too_long.then(|| best_split(&chars)).flatten() else {
        out.push(Cue { text, ..cue });
        return;
    };
    let left = &chars[..at];
    let right = &chars[at + skip..];
    let total = (width(left) + width(right)).max(1) as u64;
    let mid = cue.start_ms + (cue.end_ms - cue.start_ms) * width(left) as u64 / total;
    split_into(
        Cue::new(cue.start_ms, mid, collect(left)),
        max_ms,
        max_width,
        out,
    );
    split_into(
        Cue::new(mid, cue.end_ms, collect(right)),
        max_ms,
        max_width,
        out,
    );
}

/// Finds the best break point: close to the middle, after punctuation if possible.
fn best_split(chars: &[char]) -> Option<(usize, usize)> {
    let total = width(chars) as f64;
    break_points(chars)
        .into_iter()
        .map(|(i, skip)| {
            // Distance from the middle, as a fraction of the width. A punctuation mark
            // before the break makes the point more attractive.
            let dist = (width(&chars[..i]) as f64 - total / 2.0).abs() / total;
            let bonus = match chars[i - 1] {
                '.' | '?' | '!' | '…' | '。' | '？' | '！' => 0.35,
                ',' | ';' | ':' | '–' | '-' | '、' | '，' | '；' | '：' => 0.2,
                _ => 0.0,
            };
            ((i, skip), dist - bonus)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(point, _)| point)
}

/// Wraps text to at most two lines. One line is used when it fits in `max_width`
/// columns. Otherwise the text breaks at the point that gives the most balanced lines.
/// Text wider than two full lines still gets two lines; [`split_long`] prevents that.
pub fn wrap(text: &str, max_width: usize) -> String {
    let text = normalize_space(text);
    let chars: Vec<char> = text.chars().collect();
    if width(&chars) <= max_width {
        return text;
    }
    let best = break_points(&chars).into_iter().min_by_key(|&(i, skip)| {
        let longest = width(&chars[..i]).max(width(&chars[i + skip..]));
        // A line over the limit costs much more than an unbalanced pair. A break after
        // punctuation is worth a few columns of imbalance.
        let bonus = match chars[i - 1] {
            '.' | '?' | '!' | '…' | '。' | '？' | '！' => 8,
            ',' | ';' | ':' | '、' | '，' | '；' | '：' => 5,
            _ => 0,
        };
        (longest.saturating_sub(max_width) * 100 + longest) as i64 - bonus
    });
    match best {
        Some((i, skip)) => format!("{}\n{}", collect(&chars[..i]), collect(&chars[i + skip..])),
        None => text,
    }
}

/// Places where a line may break, as (index, chars to drop). A space is a break point
/// and is dropped. Chinese and Japanese use no spaces, so a break may also go between
/// two characters when one of them is wide, but never before closing punctuation.
fn break_points(chars: &[char]) -> Vec<(usize, usize)> {
    (1..chars.len())
        .filter_map(|i| {
            if chars[i] == ' ' {
                (i + 1 < chars.len()).then_some((i, 1))
            } else if (breaks_anywhere(chars[i - 1]) || breaks_anywhere(chars[i]))
                && chars[i - 1] != ' '
                && !NO_LINE_START.contains(&chars[i])
            {
                Some((i, 0))
            } else {
                None
            }
        })
        .collect()
}

/// Chinese and Japanese text may break between any two characters. Korean is wide
/// too, but it puts spaces between words, so it breaks only at spaces.
fn breaks_anywhere(c: char) -> bool {
    is_wide(c) && !matches!(u32::from(c), 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7A3)
}

/// Characters that must not start a line in Chinese and Japanese text.
const NO_LINE_START: &[char] = &[
    '、', '。', '，', '．', '！', '？', '：', '；', '）', '」', '』', '】', '〉', '》', '・', 'ー',
    'ゃ', 'ゅ', 'ょ', 'っ', 'ャ', 'ュ', 'ョ', 'ッ', '…', '!', '?', '.', ',',
];

/// Display width in columns. CJK and full-width characters take two columns.
pub fn width(chars: &[char]) -> usize {
    chars.iter().map(|&c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn is_wide(c: char) -> bool {
    matches!(u32::from(c),
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD)
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect::<String>().trim().to_string()
}

/// Sorts the cues and fixes timing: no cue overlaps the next one, and every cue lasts
/// at least `min_ms` if the gap to the next cue allows it.
pub fn fix_timing(cues: &mut Vec<Cue>, min_ms: u64) {
    cues.retain(|c| !c.text.trim().is_empty());
    cues.sort_by_key(|c| c.start_ms);
    for i in 0..cues.len() {
        let next_start = cues.get(i + 1).map_or(u64::MAX, |n| n.start_ms);
        let cue = &mut cues[i];
        if cue.end_ms < cue.start_ms + min_ms {
            cue.end_ms = cue.start_ms + min_ms;
        }
        cue.end_ms = cue.end_ms.min(next_start);
    }
}

fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lays out a Whisper transcript: joins sentences that Whisper cut in two, splits
/// long cues, fixes the timing, and wraps every cue to at most two lines.
pub fn layout_transcript(cues: &[Cue]) -> Vec<Cue> {
    layout(&merge_unfinished(cues, 1_000, 250), MAX_CUE_MS)
}

/// Lays out translated cues. Translations are often longer than the source, so this
/// splits and wraps them again. The cue times stay those of the source cues, which
/// already meet the time limit, so only the length limit applies.
pub fn layout_translation(cues: &[Cue]) -> Vec<Cue> {
    layout(cues, u64::MAX)
}

/// Splits long cues, fixes timing, and wraps every cue to at most two lines.
fn layout(cues: &[Cue], max_ms: u64) -> Vec<Cue> {
    let mut cues = split_long(cues, max_ms, MAX_CUE_CHARS);
    fix_timing(&mut cues, MIN_CUE_MS);
    for cue in &mut cues {
        cue.text = wrap(&cue.text, MAX_LINE_CHARS);
    }
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_timestamp(0, Format::Srt), "00:00:00,000");
        assert_eq!(format_timestamp(3_723_045, Format::Srt), "01:02:03,045");
        assert_eq!(format_timestamp(3_723_045, Format::Vtt), "01:02:03.045");
        assert_eq!(
            format_timestamp(100 * 3_600_000, Format::Srt),
            "100:00:00,000"
        );
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_timestamp("01:02:03,045").unwrap(), 3_723_045);
        assert_eq!(parse_timestamp(" 00:00:01.5 ").unwrap(), 1_500);
        assert_eq!(parse_timestamp("02:03.100").unwrap(), 123_100);
        assert!(parse_timestamp("abc").is_err());
        assert!(parse_timestamp("1:2:3:4,000").is_err());
    }

    #[test]
    fn srt_round_trip() {
        let cues = vec![
            Cue::new(1_000, 2_500, "Hello."),
            Cue::new(3_000, 4_000, "Két sor\nszöveg."),
        ];
        let text = write(&cues, Format::Srt);
        assert_eq!(
            text,
            "1\n00:00:01,000 --> 00:00:02,500\nHello.\n\n\
             2\n00:00:03,000 --> 00:00:04,000\nKét sor\nszöveg.\n\n"
        );
        assert_eq!(parse_srt(&text).unwrap(), cues);
    }

    #[test]
    fn parses_bom_crlf_and_missing_numbers() {
        let text = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,000\r\nA\r\n\r\n\
                    00:00:03,000 --> 00:00:04,000 align:start\r\nB\r\nC\r\n";
        let cues = parse_srt(text).unwrap();
        assert_eq!(
            cues,
            vec![Cue::new(1_000, 2_000, "A"), Cue::new(3_000, 4_000, "B\nC")]
        );
    }

    #[test]
    fn rejects_block_without_timing() {
        assert!(parse_srt("1\njust text\n").is_err());
    }

    #[test]
    fn writes_vtt_header() {
        let text = write(&[Cue::new(0, 1_000, "Hi")], Format::Vtt);
        assert!(text.starts_with("WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\nHi\n"));
    }

    #[test]
    fn merges_a_sentence_that_whisper_cut_in_two() {
        let cues = vec![
            Cue::new(0, 4_000, "My train was late. Next time, please call me"),
            Cue::new(4_000, 7_000, "from somewhere. We have to talk."),
            Cue::new(7_000, 8_000, "Fine."),
        ];
        let merged = merge_unfinished(&cues, 1_000, 250);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[0],
            Cue::new(
                0,
                7_000,
                "My train was late. Next time, please call me from somewhere. We have to talk."
            )
        );
        let split = split_long(&merged, MAX_CUE_MS, MAX_CUE_CHARS);
        let texts: Vec<&str> = split.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "My train was late.",
                "Next time, please call me from somewhere. We have to talk.",
                "Fine."
            ]
        );
    }

    #[test]
    fn does_not_merge_across_long_pauses_or_past_the_limit() {
        let cues = vec![
            Cue::new(0, 1_000, "and then"),
            Cue::new(5_000, 6_000, "later"),
        ];
        assert_eq!(merge_unfinished(&cues, 1_000, 250).len(), 2);
        let cues = vec![Cue::new(0, 1_000, "a b c"), Cue::new(1_000, 2_000, "d e f")];
        assert_eq!(merge_unfinished(&cues, 1_000, 8).len(), 2);
    }

    #[test]
    fn short_cue_is_not_split() {
        let cues = vec![Cue::new(0, 2_000, "Short line.")];
        assert_eq!(split_long(&cues, MAX_CUE_MS, MAX_CUE_CHARS), cues);
    }

    #[test]
    fn long_cue_splits_at_sentence_end_with_proportional_time() {
        let text = "I waited for you at the station for two hours. My train was late again.";
        let cues = split_long(&[Cue::new(0, 8_000, text)], MAX_CUE_MS, MAX_CUE_CHARS);
        assert_eq!(cues.len(), 2);
        assert_eq!(
            cues[0].text,
            "I waited for you at the station for two hours."
        );
        assert_eq!(cues[1].text, "My train was late again.");
        assert_eq!(cues[0].start_ms, 0);
        assert_eq!(cues[0].end_ms, cues[1].start_ms);
        assert_eq!(cues[1].end_ms, 8_000);
        // 46 of 70 characters are in the first part.
        assert_eq!(cues[0].end_ms, 8_000 * 46 / 70);
    }

    #[test]
    fn very_long_cue_splits_until_all_parts_fit() {
        let text = "word ".repeat(60);
        let cues = split_long(&[Cue::new(0, 30_000, text)], MAX_CUE_MS, MAX_CUE_CHARS);
        assert!(cues.len() > 2);
        for c in &cues {
            assert!(c.text.chars().count() <= MAX_CUE_CHARS);
            assert!(c.end_ms - c.start_ms <= MAX_CUE_MS);
        }
        assert_eq!(cues.last().unwrap().end_ms, 30_000);
    }

    #[test]
    fn text_without_space_is_kept_whole() {
        let text = "x".repeat(100);
        let cues = split_long(
            &[Cue::new(0, 9_000, text.clone())],
            MAX_CUE_MS,
            MAX_CUE_CHARS,
        );
        assert_eq!(cues, vec![Cue::new(0, 9_000, text)]);
    }

    #[test]
    fn wrap_keeps_short_text_on_one_line() {
        assert_eq!(
            wrap("Hello  there\nfriend", MAX_LINE_CHARS),
            "Hello there friend"
        );
    }

    #[test]
    fn wrap_balances_two_lines_and_counts_chars_not_bytes() {
        let text = "Tegnap este két órát vártalak az állomáson, hol voltál?";
        let wrapped = wrap(text, MAX_LINE_CHARS);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.chars().count() <= MAX_LINE_CHARS));
        let diff = lines[0].chars().count().abs_diff(lines[1].chars().count());
        assert!(diff <= 8, "lines are not balanced: {lines:?}");
    }

    #[test]
    fn wraps_japanese_between_characters_and_counts_double_width() {
        let text = "こんばんは、ミューラーさん。今日、弟くんを見ましたか？";
        assert_eq!(width(&text.chars().collect::<Vec<_>>()), 54);
        let wrapped = wrap(text, MAX_LINE_CHARS);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert_eq!(
            lines,
            ["こんばんは、ミューラーさん。", "今日、弟くんを見ましたか？"]
        );
    }

    #[test]
    fn prefers_breaking_after_punctuation() {
        let wrapped = wrap(
            "ごめん。電車が遅れて、スマホの電池が切れてたんだ。",
            MAX_LINE_CHARS,
        );
        assert_eq!(
            wrapped,
            "ごめん。電車が遅れて、\nスマホの電池が切れてたんだ。"
        );
    }

    #[test]
    fn korean_breaks_only_at_spaces() {
        let wrapped = wrap("집 얘기 해야 해. 엄마가 아침에 또 전화 왔어.", 20);
        assert!(wrapped.lines().all(|l| !l.starts_with('가')), "{wrapped}");
        // Joining the two lines with a space gives back the original text.
        assert_eq!(
            wrapped.replace('\n', " "),
            "집 얘기 해야 해. 엄마가 아침에 또 전화 왔어."
        );
    }

    #[test]
    fn never_starts_a_line_with_closing_punctuation() {
        let chars: Vec<char> = "見た。うん".chars().collect();
        let points: Vec<usize> = break_points(&chars).into_iter().map(|(i, _)| i).collect();
        assert!(!points.contains(&2), "break before 。: {points:?}");
        assert!(points.contains(&3));
    }

    #[test]
    fn does_not_split_one_line_of_text_for_time() {
        let cues = [Cue::new(0, 11_000, "Every week I came back")];
        assert_eq!(split_long(&cues, MAX_CUE_MS, MAX_CUE_CHARS), cues);
    }

    #[test]
    fn splits_long_chinese_cue_at_sentence_end() {
        let text = "我昨天在车站等了你两个小时。你的电话为什么一直打不通呢？我真的很担心你。";
        // 36 characters are 72 columns, so a 40-column limit needs a split.
        let cues = split_long(&[Cue::new(0, 5_000, text)], MAX_CUE_MS, 40);
        assert!(cues.len() >= 2);
        assert!(
            cues.iter()
                .all(|c| width(&c.text.chars().collect::<Vec<_>>()) <= 40)
        );
        assert!(cues[0].text.ends_with('。'));
    }

    #[test]
    fn fix_timing_extends_short_cues_and_removes_overlaps() {
        let mut cues = vec![
            Cue::new(5_000, 5_200, "C"),
            Cue::new(0, 300, "A"),
            Cue::new(500, 2_000, "B"),
            Cue::new(4_000, 6_000, "overlaps C"),
            Cue::new(7_000, 7_100, " "),
        ];
        fix_timing(&mut cues, MIN_CUE_MS);
        assert_eq!(
            cues,
            vec![
                // The gap to B is only 500 ms, so A cannot reach the minimum.
                Cue::new(0, 500, "A"),
                Cue::new(500, 2_000, "B"),
                Cue::new(4_000, 5_000, "overlaps C"),
                Cue::new(5_000, 5_800, "C"),
            ]
        );
    }
}
