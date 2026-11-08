//! Checks that a translation uses the writing system (script) of the target language.
//!
//! The translation model sometimes puts a word from another language into a line, for
//! example Czech words in Hindi, or a Hindi word inside Arabic. Such a word has the
//! wrong script for the target, or is a Latin word that the source line does not have.

use unicode_script::{Script, UnicodeScript};

/// The scripts that text in the language `code` normally uses. `None` means that the
/// code is not known, so no check is possible.
fn expected_scripts(code: &str) -> Option<&'static [Script]> {
    use Script::*;
    let scripts: &'static [Script] = match code {
        "ar" | "fa" | "ur" | "ps" | "sd" => &[Arabic],
        "he" | "yi" => &[Hebrew],
        "hi" | "mr" | "ne" | "sa" => &[Devanagari],
        "bn" | "as" => &[Bengali],
        "gu" => &[Gujarati],
        "pa" => &[Gurmukhi],
        "ta" => &[Tamil],
        "te" => &[Telugu],
        "kn" => &[Kannada],
        "ml" => &[Malayalam],
        "si" => &[Sinhala],
        "th" => &[Thai],
        "lo" => &[Lao],
        "km" => &[Khmer],
        "my" => &[Myanmar],
        "bo" => &[Tibetan],
        "ka" => &[Georgian],
        "hy" => &[Armenian],
        "am" => &[Ethiopic],
        "el" => &[Greek],
        "ru" | "uk" | "be" | "bg" | "mk" | "kk" | "tg" | "ba" | "tt" | "mn" => &[Cyrillic],
        "sr" => &[Cyrillic, Latin],
        "ja" => &[Han, Hiragana, Katakana],
        "zh" | "yue" => &[Han, Bopomofo],
        "ko" => &[Hangul, Han],
        _ if verbacue_core::languages::languages()
            .iter()
            .any(|(c, _)| *c == code) =>
        {
            &[Latin]
        }
        _ => return None,
    };
    Some(scripts)
}

/// Unicode ranges of each script, for the response grammar.
fn ranges(script: Script) -> &'static [(u32, u32)] {
    use Script::*;
    match script {
        Latin => &[(0x0100, 0x024F), (0x0300, 0x036F), (0x1E00, 0x1EFF)],
        Greek => &[(0x0370, 0x03FF), (0x1F00, 0x1FFF)],
        Cyrillic => &[(0x0400, 0x052F)],
        Armenian => &[(0x0530, 0x058F)],
        Hebrew => &[(0x0590, 0x05FF), (0xFB1D, 0xFB4F)],
        Arabic => &[
            (0x0600, 0x06FF),
            (0x0750, 0x077F),
            (0x08A0, 0x08FF),
            (0xFB50, 0xFDFF),
            (0xFE70, 0xFEFF),
        ],
        Devanagari => &[(0x0900, 0x097F), (0xA8E0, 0xA8FF)],
        Bengali => &[(0x0980, 0x09FF)],
        Gurmukhi => &[(0x0A00, 0x0A7F)],
        Gujarati => &[(0x0A80, 0x0AFF)],
        Tamil => &[(0x0B80, 0x0BFF)],
        Telugu => &[(0x0C00, 0x0C7F)],
        Kannada => &[(0x0C80, 0x0CFF)],
        Malayalam => &[(0x0D00, 0x0D7F)],
        Sinhala => &[(0x0D80, 0x0DFF)],
        Thai => &[(0x0E00, 0x0E7F)],
        Lao => &[(0x0E80, 0x0EFF)],
        Tibetan => &[(0x0F00, 0x0FFF)],
        Myanmar => &[(0x1000, 0x109F)],
        Georgian => &[(0x10A0, 0x10FF), (0x1C90, 0x1CBF)],
        Ethiopic => &[(0x1200, 0x139F)],
        Khmer => &[(0x1780, 0x17FF)],
        Hangul => &[(0x1100, 0x11FF), (0x3130, 0x318F), (0xAC00, 0xD7AF)],
        Hiragana => &[(0x3040, 0x309F)],
        Katakana => &[(0x30A0, 0x30FF), (0x31F0, 0x31FF)],
        Bopomofo => &[(0x3100, 0x312F)],
        Han => &[(0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF)],
        _ => &[],
    }
}

/// Characters that every language may use: printable ASCII (for names, digits and
/// punctuation) without `"` and `\`, Latin-1, punctuation, currency, symbols such as ♪,
/// the Indic danda, CJK punctuation and full-width forms.
const SHARED_RANGES: &[(u32, u32)] = &[
    (0x20, 0x21),
    (0x23, 0x5B),
    (0x5D, 0x7E),
    (0xA0, 0xFF),
    (0x0964, 0x0965),
    (0x2000, 0x206F),
    (0x20A0, 0x20CF),
    (0x2190, 0x2BFF),
    (0x3000, 0x303F),
    (0xFF00, 0xFFEF),
];

/// A GBNF character class with the characters that a translation into `target` may
/// use, or `None` if the target is not known. With it, the model cannot write letters
/// of another script, so it must pick its next best word.
pub fn grammar_class(target: &str) -> Option<String> {
    let scripts = expected_scripts(target)?;
    let mut class = String::from("[");
    let own = scripts.iter().flat_map(|s| ranges(*s).iter());
    for (start, end) in SHARED_RANGES.iter().chain(own) {
        class.push_str(&format!("\\u{start:04X}-\\u{end:04X}"));
    }
    class.push(']');
    Some(class)
}

/// The words of `translation` that do not fit the target language `target`: words
/// with a script that the target does not use, unless the same word is in `source`
/// (for example a name). In a non-Latin target, an ASCII word also counts if the
/// source does not have it.
pub fn foreign_words(translation: &str, source: &str, target: &str) -> Vec<String> {
    let Some(allowed) = expected_scripts(target) else {
        return Vec::new();
    };
    let source = source.to_lowercase();
    let source_words: Vec<&str> = source
        .split(|c: char| c.is_whitespace() || is_separator(c))
        .collect();
    translation
        .split(|c: char| c.is_whitespace() || is_separator(c))
        .filter(|word| {
            word.chars().any(|c| {
                let script = c.script();
                !matches!(script, Script::Common | Script::Inherited | Script::Unknown)
                    && !allowed.contains(&script)
            }) || (!allowed.contains(&Script::Latin)
                && word.chars().any(|c| c.is_ascii_alphabetic()))
        })
        .filter(|word| !source_words.contains(&word.to_lowercase().as_str()))
        .map(str::to_string)
        .collect()
}

/// Punctuation between words. Letters inside a word (such as the apostrophe in
/// "don't") stay in the word.
fn is_separator(c: char) -> bool {
    c.is_ascii_punctuation() && c != '\''
        || matches!(
            c,
            '،' | '؟'
                | '。'
                | '、'
                | '，'
                | '？'
                | '！'
                | '…'
                | '«'
                | '»'
                | '“'
                | '”'
                | '„'
                | '।'
                | '॥'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_latin_words_in_hindi() {
        let line = "मैं स्टेशन पर तेरी čekar rahi.";
        let source = "I waited for you at the station.";
        assert_eq!(foreign_words(line, source, "hi"), ["čekar", "rahi"]);
    }

    #[test]
    fn finds_a_mixed_word_in_arabic() {
        let line = "انتظرتك بالستेशन.";
        assert_eq!(foreign_words(line, "at the station", "ar"), ["بالستेशन"]);
    }

    #[test]
    fn allows_names_from_the_source() {
        let line = "मैं John के साथ Netflix देखा।";
        let source = "I watched Netflix with John.";
        assert!(foreign_words(line, source, "hi").is_empty());
    }

    #[test]
    fn accepts_normal_translations() {
        assert!(foreign_words("Szia, hol voltál tegnap?", "Hi", "hu").is_empty());
        assert!(foreign_words("Где ты был вчера?", "Hi", "ru").is_empty());
        assert!(foreign_words("昨日の夜、どこにいたの？", "Hi", "ja").is_empty());
        assert!(foreign_words("어젯밤에 어디 있었어?", "Hi", "ko").is_empty());
        assert!(foreign_words("היי ג'ון, איפה היית?", "Hi", "he").is_empty());
        assert!(foreign_words("Ćao, gde si bio? Где си био?", "Hi", "sr").is_empty());
    }

    #[test]
    fn finds_cyrillic_in_hungarian() {
        assert_eq!(foreign_words("Szia, где voltál?", "Hi", "hu"), ["где"]);
    }

    #[test]
    fn finds_english_words_that_the_source_does_not_have() {
        let line = "मैं तेरी Wait कर रही थी।";
        assert_eq!(foreign_words(line, "I waited for you.", "hi"), ["Wait"]);
        // In a Latin target, an English word is not an error: it may be a loan word.
        assert!(foreign_words("Ez egy jó film, okay?", "Nice film.", "hu").is_empty());
    }

    #[test]
    fn grammar_class_has_the_target_script() {
        let class = grammar_class("hi").unwrap();
        assert!(class.contains("\\u0900-\\u097F"));
        assert!(!class.contains("\\u0600"));
        assert!(grammar_class("xx-unknown").is_none());
    }

    #[test]
    fn skips_unknown_targets() {
        assert!(foreign_words("anything", "x", "xx-unknown").is_empty());
    }
}
