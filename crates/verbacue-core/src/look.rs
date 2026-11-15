//! The style and the animation of styled subtitles, and the timing that the animations
//! share. Burned-in subtitles and ASS files use the same rules, so both look alike.

/// The look of styled subtitles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SubtitleStyle {
    /// White text with a black outline.
    #[default]
    Classic,
    /// Yellow text with a black outline.
    Yellow,
    /// White text on a dark, half-transparent box.
    Box,
    /// Classic, about 1.4 times larger.
    Large,
    /// Large white capitals with a thick outline and a shadow, as in short videos.
    Bold,
    /// Bold, and the spoken word is on a green box.
    Highlight,
    /// White capitals with a pink glow.
    Neon,
}

/// How styled subtitles appear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SubtitleAnimation {
    #[default]
    None,
    /// The line fades in and out.
    Fade,
    /// The line grows quickly to its size.
    Pop,
    /// The letters appear one after the other.
    Typewriter,
    /// The words light up one after the other, in time with the speech.
    Karaoke,
    /// The line grows past its size and then settles.
    Bounce,
    /// Up to three words at a time, in time with the speech. Each group bounces in,
    /// and the spoken word lights up.
    Words,
}

/// The style and the animation of styled subtitles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubtitleLook {
    pub style: SubtitleStyle,
    pub animation: SubtitleAnimation,
}

impl SubtitleStyle {
    /// The text height as a share of the picture height.
    pub fn size(self) -> f32 {
        match self {
            Self::Classic | Self::Yellow | Self::Box => 0.055,
            Self::Large | Self::Bold | Self::Highlight => 0.075,
            Self::Neon => 0.07,
        }
    }

    /// The short-video styles: capitals, placed lower in the middle of the picture.
    pub fn capitals(self) -> bool {
        matches!(self, Self::Bold | Self::Highlight | Self::Neon)
    }
}

/// The text that a line shows in `style`: capitals for the short-video styles. Turkish
/// and Azerbaijani (`lang`) write the capital of "i" with a dot: "İ".
pub fn display_text(text: &str, style: SubtitleStyle, lang: &str) -> String {
    if !style.capitals() {
        return text.to_string();
    }
    let base = lang.split(['-', '_']).next().unwrap_or(lang);
    if matches!(base, "tr" | "az") {
        text.replace('i', "İ").to_uppercase()
    } else {
        text.to_uppercase()
    }
}

/// How long Fade takes in and out, and how long Pop grows.
pub const FADE_MS: u64 = 200;
pub const POP_MS: u64 = 150;
/// Typewriter writes this many letters per second, faster for a long line, so that the
/// line is complete after 70 % of its time.
pub const TYPE_CPS: u64 = 25;
/// Karaoke and Words go through the words over this share of the line, in percent.
pub const KARAOKE_SHARE: u64 = 90;
/// Bounce starts at BOUNCE_LOW percent, grows to BOUNCE_TOP in the first half of
/// BOUNCE_MS, then settles at 100.
pub const BOUNCE_LOW: u64 = 60;
pub const BOUNCE_TOP: u64 = 115;
pub const BOUNCE_MS: u64 = 180;
/// A group of Words has at most this many words and letters.
const GROUP_WORDS: usize = 3;
const GROUP_LETTERS: usize = 16;

/// When each word starts, in ms from the start of a line of `length` ms. The subtitles
/// store no word times, so each word starts when the share of the letters before it
/// has passed.
pub fn word_starts(words: &[&str], length: u64) -> Vec<u64> {
    let lens: Vec<u64> = words.iter().map(|w| w.chars().count() as u64).collect();
    let letters = lens.iter().sum::<u64>().max(1);
    let span = (length * KARAOKE_SHARE / 100).max(1);
    let mut before = 0;
    lens.iter()
        .map(|len| {
            let start = before * span / letters;
            before += len;
            start
        })
        .collect()
}

/// The time in ms after which Typewriter shows all `letters` of a line of `length` ms.
pub fn typewriter_span(letters: usize, length: u64) -> u64 {
    (letters as u64 * 1000 / TYPE_CPS)
        .min(length * 7 / 10)
        .max(1)
}

/// The size in percent, `ms` after a Bounce starts.
pub fn bounce(ms: u64) -> u8 {
    let half = BOUNCE_MS / 2;
    let size = if ms < half {
        BOUNCE_LOW + ms * (BOUNCE_TOP - BOUNCE_LOW) / half
    } else {
        BOUNCE_TOP - (ms.min(BOUNCE_MS) - half) * (BOUNCE_TOP - 100) / half
    };
    size as u8
}

/// Splits the words into groups for Words, as (first, end) word numbers. A group ends
/// after GROUP_WORDS words, before GROUP_LETTERS letters, or after a punctuation mark.
pub fn groups(words: &[&str]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut start, mut letters) = (0, 0);
    for (i, word) in words.iter().enumerate() {
        let len = word.chars().count();
        let after_mark = i > 0 && words[i - 1].ends_with(['.', ',', '!', '?', ';', ':']);
        if i > start && (i - start == GROUP_WORDS || letters + len > GROUP_LETTERS || after_mark) {
            out.push((start, i));
            (start, letters) = (i, 0);
        }
        letters += len;
    }
    if start < words.len() {
        out.push((start, words.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_start_by_their_share_of_letters() {
        // 3 words of 4, 4 and 2 letters over 90 % of 1000 ms.
        assert_eq!(word_starts(&["abcd", "efgh", "ij"], 1000), [0, 360, 720]);
        assert!(word_starts(&[], 1000).is_empty());
    }

    #[test]
    fn capitals_follow_the_language() {
        let bold = SubtitleStyle::Bold;
        assert_eq!(
            display_text("rivayetler ılık", bold, "tr"),
            "RİVAYETLER ILIK"
        );
        assert_eq!(display_text("it is", bold, "en"), "IT IS");
        assert_eq!(display_text("it is", SubtitleStyle::Classic, "tr"), "it is");
    }

    #[test]
    fn bounce_overshoots_and_settles() {
        assert_eq!(bounce(0), 60);
        assert_eq!(bounce(90), 115);
        assert_eq!(bounce(180), 100);
        assert_eq!(bounce(1000), 100);
    }

    #[test]
    fn groups_split_on_size_and_punctuation() {
        assert_eq!(groups(&["a", "b", "c", "d"]), [(0, 3), (3, 4)]);
        assert_eq!(groups(&["hi,", "you", "there"]), [(0, 1), (1, 3)]);
        assert_eq!(groups(&["extraordinarily", "long"]), [(0, 1), (1, 2)]);
        assert!(groups(&[]).is_empty());
    }
}
