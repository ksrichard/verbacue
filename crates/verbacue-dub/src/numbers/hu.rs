//! Hungarian number words.

// "két" is used only in front of a multiplier ("kétezer", but "százkettő").
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

pub(super) fn hu(n: u64) -> String {
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

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

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
}
