//! Numbers to words, so that the voice model reads them.
//!
//! XTTS-v2 was trained on text where coqui's num2words had already written the numbers
//! as words. The rules here follow num2words, so the voice sees the same words. These
//! differences are on purpose:
//! - No commas inside a number, because a comma makes the voice pause.
//! - Hungarian uses "két" only in front of a multiplier ("kétezer", but "százkettő").
//! - Spanish writes "veintiún mil", not "veintiuno mil".
//! - Turkish keeps "bir" in front of "bin" ("yüzbirbinyüz"), and keeps the count of a
//!   scale after an empty one. num2words loses digits in both cases.
//! - Japanese and Hindi: coqui keeps digits for Japanese, and num2words has no Hindi.
//!   These two have rules of their own.

mod ar;
mod cards;
mod east_asian;
mod germanic;
mod hi;
mod hu;
mod romance;
mod slavic;
mod tr;

/// The words for `n` in `lang`, or `None` if `lang` is not an XTTS-v2 language.
pub fn to_words(n: u64, lang: &str) -> Option<String> {
    Some(match lang {
        "ar" => ar::ar(n),
        "cs" => slavic::cs(n),
        "de" => germanic::de(n),
        "en" => germanic::en(n),
        "es" => romance::es(n),
        "fr" => romance::fr(n),
        "hi" => hi::hi(n),
        "hu" => hu::hu(n),
        "it" => romance::it(n),
        "ja" => east_asian::ja(n),
        "ko" => east_asian::ko(n),
        "nl" => germanic::nl(n),
        "pl" => slavic::pl(n),
        "pt" => romance::pt(n),
        "ru" => slavic::ru(n),
        "tr" => tr::tr(n),
        "zh" | "zh-cn" => east_asian::zh(n),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::LANGUAGES;

    #[test]
    fn every_xtts_language_has_words() {
        for lang in LANGUAGES {
            assert!(to_words(7, lang).is_some(), "{lang}");
        }
    }

    #[test]
    fn other_languages_have_no_words() {
        assert_eq!(to_words(5, "sv"), None);
    }

    #[test]
    fn largest_number_has_words_in_every_language() {
        for lang in LANGUAGES {
            let words = to_words(u64::MAX, lang).unwrap();
            assert!(!words.is_empty(), "{lang}");
        }
        assert!(
            to_words(u64::MAX, "hu")
                .unwrap()
                .starts_with("tizennyolctrillió")
        );
        assert!(
            to_words(u64::MAX, "en")
                .unwrap()
                .starts_with("eighteen quintillion")
        );
    }

    #[test]
    fn no_language_writes_a_comma() {
        for lang in LANGUAGES {
            for n in [1_002_500_000, 1_234_567, 2_000_000_002, u64::MAX] {
                assert!(!to_words(n, lang).unwrap().contains(','), "{lang} {n}");
            }
        }
    }
}
