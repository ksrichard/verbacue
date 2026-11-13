//! Makes subtitle text ready for the voice model.
//!
//! The rules follow coqui's `multilingual_cleaners`, which made the text that XTTS-v2
//! saw in training. Subtitles also hold text that nobody speaks, such as `[music]` or
//! song lyrics between `♪` signs. `clean` removes that text.

use crate::numbers;

/// The languages that XTTS-v2 speaks.
pub const LANGUAGES: &[&str] = &[
    "en", "es", "fr", "de", "it", "pt", "pl", "tr", "ru", "nl", "cs", "ar", "zh-cn", "ja", "hu",
    "ko", "hi",
];

/// The XTTS-v2 code for a language code, or `None` if XTTS-v2 does not speak it. The
/// region is ignored ("pt-BR" is "pt"), and all Chinese is "zh-cn", as in coqui.
pub fn xtts_code(lang: &str) -> Option<&'static str> {
    let base = lang.split(['-', '_']).next()?.to_lowercase();
    let code = if base == "zh" { "zh-cn" } else { base.as_str() };
    LANGUAGES.iter().copied().find(|l| *l == code)
}

/// True if XTTS-v2 speaks this language.
pub fn supports(lang: &str) -> bool {
    xtts_code(lang).is_some()
}

/// The longest text in characters that XTTS-v2 speaks well in one pass. These are
/// coqui's `char_limits`. A longer text gets cut off audio.
pub fn char_limit(lang: &str) -> usize {
    match xtts_code(lang).unwrap_or("en") {
        "de" => 253,
        "fr" => 273,
        "es" => 239,
        "it" => 213,
        "pt" => 203,
        "pl" | "hu" => 224,
        "zh-cn" => 82,
        "ar" => 166,
        "cs" => 186,
        "ru" => 182,
        "nl" => 251,
        "tr" => 226,
        "ja" => 71,
        "ko" => 95,
        "hi" => 150,
        _ => 250,
    }
}

/// Makes a subtitle line ready for the voice model: lower case, numbers as words (in
/// `hu` and `en`), symbols as words, and one space between words. Returns `None` for
/// a line with nothing to speak.
pub fn clean(text: &str, lang: &str) -> Option<String> {
    let lang = xtts_code(lang).unwrap_or(lang);
    let text = remove_non_speech(text);
    let mut text = text.replace('"', "");
    if lang == "tr" {
        // Rust lower-cases "İ" to "i̇" (with a dot above), which XTTS-v2 does not know.
        text = text.replace('İ', "i");
    }
    let text = text.to_lowercase();
    let text = expand_numbers(&text, lang);
    let text = expand_abbreviations(&text, lang);
    let text = expand_symbols(&text, lang);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().any(char::is_alphanumeric).then_some(text)
}

/// Removes `[...]`, `(...)` and the text between two `♪` signs.
fn remove_non_speech(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut close = None;
    for c in text.chars() {
        match (close, c) {
            (None, '[') => close = Some(']'),
            (None, '(') => close = Some(')'),
            (None, '♪' | '♫') => close = Some(c),
            (Some(end), _) if c == end => close = None,
            (Some(_), _) => {}
            (None, _) => out.push(c),
        }
    }
    out
}

/// Turns digit runs into words. A run with thousands separators ("1.000" in `hu`,
/// "1,000" in `en`) is one number. A decimal number ("12,5" in `hu`, "12.5" in `en`)
/// is read in two parts. English, Chinese, Japanese, Korean and Hindi write numbers with
/// "," for thousands and "." for decimals. The other languages use the opposite.
fn expand_numbers(text: &str, lang: &str) -> String {
    if numbers::to_words(0, lang).is_none() {
        return text.to_string();
    }
    let (thousands, decimal) = match lang {
        "en" | "zh-cn" | "ja" | "ko" | "hi" => (',', '.'),
        _ => ('.', ','),
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (whole, next) = read_number(&chars, i, thousands);
        i = next;
        let Some(n) = whole.parse::<u64>().ok() else {
            out.push_str(&whole);
            continue;
        };
        out.push_str(&numbers::to_words(n, lang).unwrap_or(whole));
        if chars.get(i) == Some(&decimal) && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
            let start = i + 1;
            let mut end = start;
            while chars.get(end).is_some_and(char::is_ascii_digit) {
                end += 1;
            }
            let frac: String = chars[start..end].iter().collect();
            out.push_str(&decimal_words(&frac, n, lang));
            i = end;
        }
    }
    out
}

/// Reads the digits from `start`, with groups of three digits after `thousands`.
/// Returns the digits without separators and the index after the number.
fn read_number(chars: &[char], start: usize, thousands: char) -> (String, usize) {
    let mut digits = String::new();
    let mut i = start;
    while chars.get(i).is_some_and(char::is_ascii_digit) {
        digits.push(chars[i]);
        i += 1;
    }
    if digits.len() > 3 {
        return (digits, i);
    }
    loop {
        let group = chars.get(i + 1..i + 4);
        let is_group = chars.get(i) == Some(&thousands)
            && group.is_some_and(|g| g.iter().all(char::is_ascii_digit))
            && !chars.get(i + 4).is_some_and(char::is_ascii_digit);
        if !is_group {
            return (digits, i);
        }
        digits.extend(&chars[i + 1..i + 4]);
        i += 4;
    }
}

/// The words after the decimal mark. Most languages say their "point" word and read
/// the digits one by one. `whole` is the number in front of the mark.
fn decimal_words(frac: &str, whole: u64, lang: &str) -> String {
    let n: u64 = frac.parse().unwrap_or(0);
    let digits: Vec<String> = frac
        .chars()
        .filter_map(|c| numbers::to_words(c.to_digit(10)?.into(), lang))
        .collect();
    match lang {
        "hu" => {
            let unit = match frac.len() {
                1 => "tized",
                2 => "század",
                3 => "ezred",
                _ => "",
            };
            let words = numbers::to_words(n, lang).unwrap_or_default();
            format!(" egész {words} {unit}").trim_end().to_string()
        }
        // No spaces: the digits are single kanji or hanzi.
        "zh-cn" | "ja" => format!("点{}", digits.concat()),
        // Russian: "целая" after a number that ends in 1 (but not 11), else "целых".
        "ru" => {
            let word = if whole % 10 == 1 && whole % 100 != 11 {
                "целая"
            } else {
                "целых"
            };
            format!(" {word} {}", digits.join(" "))
        }
        _ => {
            let point = match lang {
                "ar" => "فاصلة",
                "cs" => "celá",
                "de" => "Komma",
                "es" => "punto",
                "fr" => "virgule",
                "hi" => "दशमलव",
                "it" => "virgola",
                "ko" => "점",
                "nl" => "komma",
                "pl" => "przecinek",
                "pt" => "vírgula",
                "tr" => "virgül",
                _ => "point",
            };
            format!(" {point} {}", digits.join(" "))
        }
    }
}

/// Coqui's abbreviation list. The text is already lower case.
fn expand_abbreviations(text: &str, lang: &str) -> String {
    let list: &[(&str, &str)] = match lang {
        "en" => &[
            ("mrs", "misess"),
            ("mr", "mister"),
            ("dr", "doctor"),
            ("st", "saint"),
            ("co", "company"),
            ("jr", "junior"),
            ("maj", "major"),
            ("gen", "general"),
            ("drs", "doctors"),
            ("rev", "reverend"),
            ("lt", "lieutenant"),
            ("hon", "honorable"),
            ("sgt", "sergeant"),
            ("capt", "captain"),
            ("esq", "esquire"),
            ("ltd", "limited"),
            ("col", "colonel"),
            ("ft", "fort"),
        ],
        "hu" => &[("dr", "doktor"), ("b", "bácsi"), ("nőv", "nővér")],
        _ => return text.to_string(),
    };
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
            continue;
        }
        let long = (c == '.')
            .then(|| list.iter().find(|(short, _)| *short == word))
            .flatten();
        match long {
            Some((_, long)) => out.push_str(long),
            None => {
                out.push_str(&word);
                out.push(c);
            }
        }
        word.clear();
    }
    out.push_str(&word);
    out
}

/// Coqui's symbol list, with "%" added for English.
fn expand_symbols(text: &str, lang: &str) -> String {
    let list: &[(char, &str)] = match lang {
        "en" => &[
            ('&', "and"),
            ('@', "at"),
            ('%', "percent"),
            ('$', "dollar"),
            ('£', "pound"),
            ('°', "degrees"),
        ],
        "hu" => &[
            ('&', "és"),
            ('@', "kukac"),
            ('%', "százalék"),
            ('#', "kettőskereszt"),
            ('$', "dollár"),
            ('£', "font"),
            ('°', "fok"),
        ],
        _ => return text.to_string(),
    };
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match list.iter().find(|(s, _)| *s == c) {
            Some((_, word)) => {
                out.push(' ');
                out.push_str(word);
                out.push(' ');
            }
            None => out.push(c),
        }
    }
    out
}

/// Splits a clean text into parts that are each at most [`char_limit`] characters.
/// It splits after `.`, `!`, `?` and `…` first, then between words, and cuts a word
/// only if the word alone is longer than the limit.
pub fn split(text: &str, lang: &str) -> Vec<String> {
    let limit = char_limit(lang);
    let mut parts: Vec<String> = Vec::new();
    for sentence in sentences(text) {
        for piece in wrap(&sentence, limit) {
            match parts.last_mut() {
                Some(last) if last.chars().count() + 1 + piece.chars().count() <= limit => {
                    last.push(' ');
                    last.push_str(&piece);
                }
                _ => parts.push(piece),
            }
        }
    }
    parts
}

fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        let end = matches!(c, '.' | '!' | '?' | '…');
        // "wait...", "what?!": the sentence ends after the last mark.
        let more_marks = chars
            .peek()
            .is_some_and(|n| matches!(n, '.' | '!' | '?' | '…'));
        if end && !more_marks {
            push_trimmed(&mut out, &current);
            current.clear();
        }
    }
    push_trimmed(&mut out, &current);
    out
}

fn push_trimmed(out: &mut Vec<String>, s: &str) {
    let s = s.trim();
    if !s.is_empty() {
        out.push(s.to_string());
    }
}

fn wrap(sentence: &str, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in sentence.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        while word.len() > limit {
            push_trimmed(&mut out, &line);
            line.clear();
            out.push(word.drain(..limit).collect());
        }
        let word: String = word.into_iter().collect();
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > limit {
            push_trimmed(&mut out, &line);
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    push_trimmed(&mut out, &line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_skips_non_speech() {
        for (text, lang) in [
            ("[music]", "hu"),
            ("♪ la la ♪", "en"),
            ("...", "hu"),
            ("  ", "en"),
            ("(sighs)", "en"),
        ] {
            assert_eq!(clean(text, lang), None, "{text:?}");
        }
    }

    #[test]
    fn clean_keeps_speech() {
        assert_eq!(
            clean("Van 3 macskám.", "hu").as_deref(),
            Some("van három macskám.")
        );
        assert_eq!(
            clean("[door opens] Hello,\nJohn!", "en").as_deref(),
            Some("hello, john!")
        );
        assert_eq!(
            clean("1956-ban születtem, most 2026 van.", "hu").as_deref(),
            Some("ezerkilencszázötvenhat-ban születtem, most kétezer-huszonhat van.")
        );
    }

    #[test]
    fn decimals_use_the_point_word_of_the_language() {
        for (text, lang, expected) in [
            ("2,5", "de", "zwei Komma fünf"),
            ("3,14", "fr", "trois virgule un quatre"),
            ("1,5", "es", "uno punto cinco"),
            ("2,5", "ru", "два целых пять"),
            ("1,5", "ru", "один целая пять"),
            ("11,5", "ru", "одиннадцать целых пять"),
            ("3,14", "pl", "trzy przecinek jeden cztery"),
            ("3,14", "cs", "tři celá jedna čtyři"),
            ("3.14", "ja", "三点一四"),
            ("3.14", "zh-cn", "三点一四"),
            ("3.14", "ko", "삼 점 일 사"),
            ("3.14", "hi", "तीन दशमलव एक चार"),
        ] {
            assert_eq!(clean(text, lang).as_deref(), Some(expected), "{lang}");
        }
    }

    #[test]
    fn thousands_separator_follows_the_language() {
        for (text, lang, expected) in [
            ("1.000", "de", "eintausend"),
            ("1,000", "ko", "천"),
            ("1,000", "hi", "एक हज़ार"),
            ("1,000", "ja", "千"),
            ("1,000", "zh-cn", "一千"),
        ] {
            assert_eq!(clean(text, lang).as_deref(), Some(expected), "{lang}");
        }
    }

    #[test]
    fn clean_expands_symbols_and_abbreviations() {
        assert_eq!(
            clean("Dr. Kiss 14% & 1.000 Ft", "hu").as_deref(),
            Some("doktor kiss tizennégy százalék és ezer ft")
        );
        assert_eq!(
            clean("Mr. Smith paid $12.50", "en").as_deref(),
            Some("mister smith paid dollar twelve point five zero")
        );
        assert_eq!(
            clean("12,5 perc", "hu").as_deref(),
            Some("tizenkettő egész öt tized perc")
        );
    }

    #[test]
    fn german_numbers_become_words() {
        assert_eq!(
            clean("Ich habe 5 Äpfel.", "de").as_deref(),
            Some("ich habe fünf äpfel.")
        );
    }

    #[test]
    fn language_codes() {
        assert_eq!(xtts_code("zh"), Some("zh-cn"));
        assert_eq!(xtts_code("zh-TW"), Some("zh-cn"));
        assert_eq!(xtts_code("pt-BR"), Some("pt"));
        assert!(supports("hu"));
        assert!(!supports("fi"));
        assert_eq!(LANGUAGES.len(), 17);
    }

    #[test]
    fn split_respects_limit() {
        let sentence = "Ez egy hosszú magyar mondat, amely sok szót tartalmaz, hogy a teszt \
                        biztosan átlépje a határt, és még egy kicsit tovább is tart.";
        let text = [sentence; 6].join(" ");
        assert!(text.chars().count() > 600);
        let parts = split(&text, "hu");
        assert!(parts.len() > 1);
        for p in &parts {
            assert!(p.chars().count() <= 224, "{} chars", p.chars().count());
        }
        let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(words(&parts.join(" ")), words(&text));
    }

    #[test]
    fn split_wraps_a_long_sentence_and_cuts_a_long_word() {
        let text = format!("{} {}", "szó ".repeat(100), "a".repeat(300));
        for p in split(&text, "hu") {
            assert!(p.chars().count() <= 224);
        }
        assert_eq!(split("Wait... what?! Yes.", "en"), ["Wait... what?! Yes."]);
    }
}
