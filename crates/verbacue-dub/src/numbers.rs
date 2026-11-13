//! Numbers to words, so that the voice model reads them.
//!
//! The hyphen rules follow coqui's num2words, which made the text that XTTS-v2 saw in
//! training. Two things differ on purpose. Hungarian uses "két" only in front of a
//! multiplier ("kétezer", but "százkettő"). English has no commas between the groups,
//! because a comma makes the voice stop inside the number.

/// The words for `n` in `lang`. Only `hu` and `en` have words; other languages return
/// `None` and keep the digits.
pub fn to_words(n: u64, lang: &str) -> Option<String> {
    match lang {
        "hu" => Some(hu(n)),
        "en" => Some(en(n)),
        _ => None,
    }
}

const HU_ONES: [&str; 10] = [
    "", "egy", "kettő", "három", "négy", "öt", "hat", "hét", "nyolc", "kilenc",
];
const HU_TENS: [&str; 10] = [
    "",
    "tíz",
    "húsz",
    "harminc",
    "negyven",
    "ötven",
    "hatvan",
    "hetven",
    "nyolcvan",
    "kilencven",
];
// 10 and 20 have their own form when ones follow: "tizenegy", "huszonegy".
const HU_TENS_PREFIX: [&str; 3] = ["", "tizen", "huszon"];

fn hu_below_1000(n: u64) -> String {
    let (h, t, o) = (
        (n / 100) as usize,
        (n / 10 % 10) as usize,
        (n % 10) as usize,
    );
    let mut s = String::new();
    if h > 0 {
        if h > 1 {
            s.push_str(if h == 2 { "két" } else { HU_ONES[h] });
        }
        s.push_str("száz");
    }
    if o > 0 && t < 3 {
        s.push_str(HU_TENS_PREFIX[t]);
    } else {
        s.push_str(HU_TENS[t]);
    }
    s.push_str(HU_ONES[o]);
    s
}

/// A group in front of a multiplier ("ezer", "millió") uses "két", not "kettő".
fn hu_multiplied(n: u64) -> String {
    let s = hu_below_1000(n);
    match s.strip_suffix("kettő") {
        Some(head) => format!("{head}két"),
        None => s,
    }
}

fn hu(n: u64) -> String {
    if n == 0 {
        return "nulla".into();
    }
    const SCALES: [(u64, &str); 6] = [
        (1_000_000_000_000_000_000, "trillió"),
        (1_000_000_000_000_000, "billiárd"),
        (1_000_000_000_000, "billió"),
        (1_000_000_000, "milliárd"),
        (1_000_000, "millió"),
        (1_000, "ezer"),
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut rest = n;
    for (scale, word) in SCALES {
        let count = rest / scale;
        if count == 0 {
            continue;
        }
        rest %= scale;
        // A plain thousand is "ezer", but a plain million is "egymillió".
        if count == 1 && scale == 1_000 {
            parts.push(word.into());
        } else {
            parts.push(format!("{}{word}", hu_multiplied(count)));
        }
    }
    if rest > 0 {
        parts.push(hu_below_1000(rest));
    }
    // Hungarian spelling: up to 2000 the number is one word, above it the groups of
    // three digits are joined with hyphens.
    if n <= 2000 {
        parts.concat()
    } else {
        parts.join("-")
    }
}

const EN_ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const EN_TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn en_below_100(n: u64) -> String {
    let n = n as usize;
    match n {
        0..20 => EN_ONES[n].into(),
        _ if n.is_multiple_of(10) => EN_TENS[n / 10].into(),
        _ => format!("{}-{}", EN_TENS[n / 10], EN_ONES[n % 10]),
    }
}

fn en_below_1000(n: u64) -> String {
    match (n / 100, n % 100) {
        (0, r) => en_below_100(r),
        (h, 0) => format!("{} hundred", EN_ONES[h as usize]),
        (h, r) => format!("{} hundred and {}", EN_ONES[h as usize], en_below_100(r)),
    }
}

fn en(n: u64) -> String {
    if n == 0 {
        return "zero".into();
    }
    const SCALES: [(u64, &str); 6] = [
        (1_000_000_000_000_000_000, "quintillion"),
        (1_000_000_000_000_000, "quadrillion"),
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut rest = n;
    for (scale, word) in SCALES {
        let count = rest / scale;
        if count > 0 {
            parts.push(format!("{} {word}", en_below_1000(count)));
            rest %= scale;
        }
    }
    if rest > 0 {
        // British style: "two thousand and six".
        if !parts.is_empty() && rest < 100 {
            parts.push(format!("and {}", en_below_100(rest)));
        } else {
            parts.push(en_below_1000(rest));
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_hu() {
        for (n, words) in [
            (0, "nulla"),
            (2, "kettő"),
            (12, "tizenkettő"),
            (21, "huszonegy"),
            (32, "harminckettő"),
            (100, "száz"),
            (102, "százkettő"),
            (200, "kétszáz"),
            (1000, "ezer"),
            (1956, "ezerkilencszázötvenhat"),
            (2000, "kétezer"),
            (2002, "kétezer-kettő"),
            (2026, "kétezer-huszonhat"),
            (12345, "tizenkétezer-háromszáznegyvenöt"),
            (1_000_000, "egymillió"),
            (2_500_000, "kétmillió-ötszázezer"),
            (2_000_000_002, "kétmilliárd-kettő"),
        ] {
            assert_eq!(to_words(n, "hu").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_en() {
        for (n, words) in [
            (0, "zero"),
            (21, "twenty-one"),
            (100, "one hundred"),
            (101, "one hundred and one"),
            (1001, "one thousand and one"),
            (1100, "one thousand one hundred"),
            (1956, "one thousand nine hundred and fifty-six"),
            (2026, "two thousand and twenty-six"),
            (
                1_234_567,
                "one million two hundred and thirty-four thousand five hundred and sixty-seven",
            ),
        ] {
            assert_eq!(to_words(n, "en").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn other_languages_keep_digits() {
        assert_eq!(to_words(5, "de"), None);
    }

    #[test]
    fn largest_number_has_words() {
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
}
