//! English, German and Dutch number words.

use super::cards::{Cards, Pair};

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

pub(super) fn en(n: u64) -> String {
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

// German ------------------------------------------------------------------------------

pub(super) fn de(n: u64) -> String {
    let cards = Cards::new(
        &[
            (1_000_000_000_000_000_000, "Trillion"),
            (1_000_000_000_000_000, "Billiarde"),
            (1_000_000_000_000, "Billion"),
            (1_000_000_000, "Milliarde"),
            (1_000_000, "Million"),
        ],
        &[
            (1000, "tausend"),
            (100, "hundert"),
            (90, "neunzig"),
            (80, "achtzig"),
            (70, "siebzig"),
            (60, "sechzig"),
            (50, "fünfzig"),
            (40, "vierzig"),
            (30, "dreißig"),
        ],
        &[
            "null",
            "eins",
            "zwei",
            "drei",
            "vier",
            "fünf",
            "sechs",
            "sieben",
            "acht",
            "neun",
            "zehn",
            "elf",
            "zwölf",
            "dreizehn",
            "vierzehn",
            "fünfzehn",
            "sechzehn",
            "siebzehn",
            "achtzehn",
            "neunzehn",
            "zwanzig",
        ],
    );
    cards.cardinal(n, de_merge)
}

fn de_merge(curr: &Pair, next: &Pair) -> Pair {
    let (mut ctext, cnum) = (curr.0.clone(), curr.1);
    let (mut ntext, nnum) = (next.0.clone(), next.1);
    if cnum == 1 {
        if nnum == 100 || nnum == 1000 {
            return (format!("ein{ntext}"), nnum);
        } else if nnum < 1_000_000 {
            return next.clone();
        }
        ctext = "eine".into();
    }
    let value = if nnum > cnum {
        if nnum >= 1_000_000 {
            if cnum > 1 {
                ntext.push_str(if ntext.ends_with('e') { "n" } else { "en" });
            }
            ctext.push(' ');
        }
        cnum * nnum
    } else {
        if nnum < 10 && 10 < cnum && cnum < 100 {
            if nnum == 1 {
                ntext = "ein".into();
            }
            // "einundzwanzig": the ones come first.
            (ntext, ctext) = (ctext, format!("{ntext}und"));
        } else if cnum >= 1_000_000 {
            ctext.push(' ');
        }
        cnum + nnum
    };
    (ctext + &ntext, value)
}

// Dutch -------------------------------------------------------------------------------

pub(super) fn nl(n: u64) -> String {
    let cards = Cards::new(
        &[
            (1_000_000_000_000_000_000, "triljoen"),
            (1_000_000_000_000_000, "biljard"),
            (1_000_000_000_000, "biljoen"),
            (1_000_000_000, "miljard"),
            (1_000_000, "miljoen"),
        ],
        &[
            (1000, "duizend"),
            (100, "honderd"),
            (90, "negentig"),
            (80, "tachtig"),
            (70, "zeventig"),
            (60, "zestig"),
            (50, "vijftig"),
            (40, "veertig"),
            (30, "dertig"),
        ],
        &[
            "nul",
            "één",
            "twee",
            "drie",
            "vier",
            "vijf",
            "zes",
            "zeven",
            "acht",
            "negen",
            "tien",
            "elf",
            "twaalf",
            "dertien",
            "veertien",
            "vijftien",
            "zestien",
            "zeventien",
            "achttien",
            "negentien",
            "twintig",
        ],
    );
    cards.cardinal(n, nl_merge)
}

fn nl_merge(curr: &Pair, next: &Pair) -> Pair {
    let (mut ctext, cnum) = (curr.0.clone(), curr.1);
    let (mut ntext, nnum) = (next.0.clone(), next.1);
    if cnum == 1 {
        if nnum < 1_000_000 {
            return next.clone();
        }
        ctext = "een".into();
    }
    let value = if nnum > cnum {
        if nnum >= 1_000_000 {
            ctext.push(' ');
        }
        cnum * nnum
    } else {
        if nnum < 10 && 10 < cnum && cnum < 100 {
            if nnum == 1 {
                ntext = "een".into();
            }
            ntext.push_str(if ntext.ends_with('e') { "ën" } else { "en" });
            // "eenentwintig": the ones come first.
            (ntext, ctext) = (ctext, ntext);
        } else if cnum >= 1_000_000 {
            ctext.push(' ');
        }
        cnum + nnum
    };
    (ctext + &ntext, value)
}

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

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
    fn numbers_de() {
        for (n, words) in [
            (0, "null"),
            (1, "eins"),
            (2, "zwei"),
            (3, "drei"),
            (4, "vier"),
            (5, "fünf"),
            (6, "sechs"),
            (7, "sieben"),
            (8, "acht"),
            (9, "neun"),
            (10, "zehn"),
            (11, "elf"),
            (12, "zwölf"),
            (13, "dreizehn"),
            (14, "vierzehn"),
            (15, "fünfzehn"),
            (16, "sechzehn"),
            (17, "siebzehn"),
            (18, "achtzehn"),
            (19, "neunzehn"),
            (20, "zwanzig"),
            (21, "einundzwanzig"),
            (22, "zweiundzwanzig"),
            (23, "dreiundzwanzig"),
            (24, "vierundzwanzig"),
            (25, "fünfundzwanzig"),
            (30, "dreißig"),
            (40, "vierzig"),
            (50, "fünfzig"),
            (60, "sechzig"),
            (70, "siebzig"),
            (80, "achtzig"),
            (90, "neunzig"),
            (99, "neunundneunzig"),
            (100, "einhundert"),
            (101, "einhunderteins"),
            (110, "einhundertzehn"),
            (200, "zweihundert"),
            (999, "neunhundertneunundneunzig"),
            (1_000, "eintausend"),
            (1_001, "eintausendeins"),
            (1_100, "eintausendeinhundert"),
            (1_956, "eintausendneunhundertsechsundfünfzig"),
            (2_000, "zweitausend"),
            (2_026, "zweitausendsechsundzwanzig"),
            (10_000, "zehntausend"),
            (12_345, "zwölftausenddreihundertfünfundvierzig"),
            (100_000, "einhunderttausend"),
            (1_000_000, "eine Million"),
            (2_500_000, "zwei Millionen fünfhunderttausend"),
            (
                1_234_567,
                "eine Million zweihundertvierunddreißigtausendfünfhundertsiebenundsechzig",
            ),
            (1_000_000_000, "eine Milliarde"),
        ] {
            assert_eq!(to_words(n, "de").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_nl() {
        for (n, words) in [
            (0, "nul"),
            (1, "één"),
            (2, "twee"),
            (3, "drie"),
            (4, "vier"),
            (5, "vijf"),
            (6, "zes"),
            (7, "zeven"),
            (8, "acht"),
            (9, "negen"),
            (10, "tien"),
            (11, "elf"),
            (12, "twaalf"),
            (13, "dertien"),
            (14, "veertien"),
            (15, "vijftien"),
            (16, "zestien"),
            (17, "zeventien"),
            (18, "achttien"),
            (19, "negentien"),
            (20, "twintig"),
            (21, "eenentwintig"),
            (22, "tweeëntwintig"),
            (23, "drieëntwintig"),
            (24, "vierentwintig"),
            (25, "vijfentwintig"),
            (30, "dertig"),
            (40, "veertig"),
            (50, "vijftig"),
            (60, "zestig"),
            (70, "zeventig"),
            (80, "tachtig"),
            (90, "negentig"),
            (99, "negenennegentig"),
            (100, "honderd"),
            (101, "honderdéén"),
            (110, "honderdtien"),
            (200, "tweehonderd"),
            (999, "negenhonderdnegenennegentig"),
            (1_000, "duizend"),
            (1_001, "duizendéén"),
            (1_100, "duizendhonderd"),
            (1_956, "duizendnegenhonderdzesenvijftig"),
            (2_000, "tweeduizend"),
            (2_026, "tweeduizendzesentwintig"),
            (10_000, "tienduizend"),
            (12_345, "twaalfduizenddriehonderdvijfenveertig"),
            (100_000, "honderdduizend"),
            (1_000_000, "een miljoen"),
            (2_500_000, "twee miljoen vijfhonderdduizend"),
            (
                1_234_567,
                "een miljoen tweehonderdvierendertigduizendvijfhonderdzevenenzestig",
            ),
            (1_000_000_000, "een miljard"),
        ] {
            assert_eq!(to_words(n, "nl").unwrap(), words, "{n}");
        }
    }
}
