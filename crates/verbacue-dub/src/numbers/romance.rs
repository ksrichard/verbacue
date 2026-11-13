//! Spanish, French, Italian and Portuguese number words.

use super::cards::{Cards, Pair};

/// `s` without its last `n` characters.
fn drop_chars(s: &str, n: usize) -> &str {
    let keep = s.chars().count().saturating_sub(n);
    let end = s.char_indices().nth(keep).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

// Spanish -----------------------------------------------------------------------------

pub(super) fn es(n: u64) -> String {
    let cards = Cards::new(
        &[
            (1_000_000_000_000_000_000, "trillón"),
            (1_000_000_000_000, "billón"),
            (1_000_000, "millón"),
        ],
        &[
            (1000, "mil"),
            (100, "cien"),
            (90, "noventa"),
            (80, "ochenta"),
            (70, "setenta"),
            (60, "sesenta"),
            (50, "cincuenta"),
            (40, "cuarenta"),
            (30, "treinta"),
        ],
        &[
            "cero",
            "uno",
            "dos",
            "tres",
            "cuatro",
            "cinco",
            "seis",
            "siete",
            "ocho",
            "nueve",
            "diez",
            "once",
            "doce",
            "trece",
            "catorce",
            "quince",
            "dieciséis",
            "diecisiete",
            "dieciocho",
            "diecinueve",
            "veinte",
            "veintiuno",
            "veintidós",
            "veintitrés",
            "veinticuatro",
            "veinticinco",
            "veintiséis",
            "veintisiete",
            "veintiocho",
            "veintinueve",
        ],
    );
    cards.cardinal(n, es_merge)
}

/// "uno" becomes "un" in front of "mil" and "millón", as in "veintiún mil". num2words
/// keeps "uno" there, which is wrong.
fn es_apocope(text: &str) -> String {
    if let Some(head) = text.strip_suffix("veintiuno") {
        format!("{head}veintiún")
    } else if let Some(head) = text.strip_suffix("uno") {
        format!("{head}un")
    } else {
        text.to_string()
    }
}

fn es_merge(curr: &Pair, next: &Pair) -> Pair {
    let (mut ctext, cnum) = (curr.0.clone(), curr.1);
    let (mut ntext, nnum) = (next.0.clone(), next.1);
    if cnum == 1 {
        if nnum < 1_000_000 {
            return next.clone();
        }
        ctext = "un".into();
    } else if cnum == 100 && nnum % 1000 != 0 {
        ctext.push_str("to");
    }
    if nnum < cnum {
        let joiner = if cnum < 100 { " y " } else { " " };
        return (format!("{ctext}{joiner}{ntext}"), cnum + nnum);
    }
    if nnum >= 1000 && cnum > 1 {
        ctext = es_apocope(&ctext);
    }
    if nnum % 1_000_000 == 0 && cnum > 1 {
        ntext = format!("{}lones", drop_chars(&ntext, 3));
    }
    if nnum == 100 {
        match cnum {
            5 => {
                ctext = "quinien".into();
                ntext.clear();
            }
            7 => ctext = "sete".into(),
            9 => ctext = "nove".into(),
            _ => {}
        }
        ntext.push_str("tos");
    } else {
        ntext = format!(" {ntext}");
    }
    (ctext + &ntext, cnum * nnum)
}

// French ------------------------------------------------------------------------------

pub(super) fn fr(n: u64) -> String {
    let cards = Cards::new(
        &[
            (1_000_000_000_000_000_000, "trillion"),
            (1_000_000_000_000_000, "billiard"),
            (1_000_000_000_000, "billion"),
            (1_000_000_000, "milliard"),
            (1_000_000, "million"),
        ],
        &[
            (1000, "mille"),
            (100, "cent"),
            (80, "quatre-vingts"),
            (60, "soixante"),
            (50, "cinquante"),
            (40, "quarante"),
            (30, "trente"),
        ],
        &[
            "zéro", "un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix",
            "onze", "douze", "treize", "quatorze", "quinze", "seize", "dix-sept", "dix-huit",
            "dix-neuf", "vingt",
        ],
    );
    cards.cardinal(n, fr_merge)
}

fn fr_merge(curr: &Pair, next: &Pair) -> Pair {
    let (mut ctext, cnum) = (curr.0.clone(), curr.1);
    let (mut ntext, nnum) = (next.0.clone(), next.1);
    if cnum == 1 {
        if nnum < 1_000_000 {
            return next.clone();
        }
    } else {
        let plural_end =
            (cnum as i64 - 80).rem_euclid(100) == 0 || (cnum % 100 == 0 && cnum < 1000);
        if plural_end && nnum < 1_000_000 && ctext.ends_with('s') {
            ctext.pop();
        }
        if cnum < 1000 && nnum != 1000 && !ntext.ends_with('s') && nnum % 100 == 0 {
            ntext.push('s');
        }
    }
    if nnum < cnum && cnum < 100 {
        if nnum % 10 == 1 && cnum != 80 {
            return (format!("{ctext} et {ntext}"), cnum + nnum);
        }
        return (format!("{ctext}-{ntext}"), cnum + nnum);
    }
    if nnum > cnum {
        return (format!("{ctext} {ntext}"), cnum * nnum);
    }
    (format!("{ctext} {ntext}"), cnum + nnum)
}

// Italian -----------------------------------------------------------------------------

const IT_ONES: [&str; 20] = [
    "zero",
    "uno",
    "due",
    "tre",
    "quattro",
    "cinque",
    "sei",
    "sette",
    "otto",
    "nove",
    "dieci",
    "undici",
    "dodici",
    "tredici",
    "quattordici",
    "quindici",
    "sedici",
    "diciassette",
    "diciotto",
    "diciannove",
];

/// The words for 1 000 000 and above, which are written as separate words.
const IT_EXPONENT_PREFIXES: [&str; 4] = ["zero", "m", "b", "tr"];

pub(super) fn it(n: u64) -> String {
    let words = match n {
        0..20 => IT_ONES[n as usize].to_string(),
        20..100 => it_tens(n),
        100..1000 => it_hundreds(n),
        1000..1_000_000 => it_thousands(n),
        _ => it_big(n),
    };
    it_accent(&words)
}

fn it_omit_zero(words: String) -> String {
    if words == "zero" {
        String::new()
    } else {
        words
    }
}

/// Two vowels that meet are joined: "ventotto", "trentuno", "centotto".
fn it_contract(s: &str) -> String {
    s.replace("oo", "o")
        .replace("ao", "o")
        .replace("io", "o")
        .replace("au", "u")
        .replace("iu", "u")
}

fn it_tens(n: u64) -> String {
    let (tens, units) = ((n / 10) as usize, (n % 10) as usize);
    let prefix = match tens {
        2 => "venti".to_string(),
        3 => "trenta".to_string(),
        4 => "quaranta".to_string(),
        6 => "sessanta".to_string(),
        _ => format!("{}anta", drop_chars(IT_ONES[tens], 1)),
    };
    let postfix = it_omit_zero(IT_ONES[units].to_string());
    it_contract(&format!("{prefix}{postfix}"))
}

fn it_hundreds(n: u64) -> String {
    let hundreds = (n / 100) as usize;
    let prefix = if hundreds == 1 {
        "cento".to_string()
    } else {
        format!("{}cento", IT_ONES[hundreds])
    };
    let postfix = it_omit_zero(it(n % 100));
    it_contract(&format!("{prefix}{postfix}"))
}

fn it_thousands(n: u64) -> String {
    let thousands = n / 1000;
    let prefix = if thousands == 1 {
        "mille".to_string()
    } else {
        format!("{}mila", it(thousands))
    };
    format!("{prefix}{}", it_omit_zero(it(n % 1000)))
}

fn it_big(n: u64) -> String {
    let digits = n.to_string();
    let predigits = if digits.len().is_multiple_of(3) {
        3
    } else {
        digits.len() % 3
    };
    let (multiplier, exponent) = digits.split_at(predigits);
    let prefix = IT_EXPONENT_PREFIXES[exponent.len() / 6];
    let mut infix = if exponent.len() % 6 == 0 {
        format!("{prefix}ilione")
    } else {
        format!("{prefix}iliardo")
    };
    let head = if multiplier == "1" {
        "un ".to_string()
    } else {
        let plural = format!("{}i", drop_chars(&infix, 1));
        infix = format!(" {plural}");
        it(multiplier.parse().expect("digits"))
    };
    let mut tail = String::new();
    if exponent.chars().any(|c| c != '0') {
        tail = it(exponent.parse().expect("digits"));
        // num2words writes a comma here when the rest has an "e". A comma makes the
        // voice pause, so only the space stays.
        infix.push_str(if tail.contains(" e ") { " " } else { " e " });
    }
    format!("{head}{infix}{tail}")
}

/// "tre" at the end of a word is "tré" when the word has more letters ("ventitré").
fn it_accent(words: &str) -> String {
    words
        .split(' ')
        .map(|w| {
            let plain = w.replace("tré", "tre");
            if w.ends_with("tre") && w.chars().count() > 3 {
                format!("{}tré", drop_chars(&plain, 3))
            } else {
                plain
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Portuguese --------------------------------------------------------------------------

pub(super) fn pt(n: u64) -> String {
    let cards = Cards::new(
        &[
            (1_000_000_000_000_000_000, "trilião"),
            (1_000_000_000_000, "bilião"),
            (1_000_000, "milião"),
        ],
        &[
            (1000, "mil"),
            (100, "cem"),
            (90, "noventa"),
            (80, "oitenta"),
            (70, "setenta"),
            (60, "sessenta"),
            (50, "cinquenta"),
            (40, "quarenta"),
            (30, "trinta"),
        ],
        &[
            "zero",
            "um",
            "dois",
            "três",
            "quatro",
            "cinco",
            "seis",
            "sete",
            "oito",
            "nove",
            "dez",
            "onze",
            "doze",
            "treze",
            "catorze",
            "quinze",
            "dezasseis",
            "dezassete",
            "dezoito",
            "dezanove",
            "vinte",
        ],
    );
    pt_join(cards.cardinal(n, pt_merge))
}

const PT_HUNDREDS: [&str; 10] = [
    "",
    "cento",
    "duzentos",
    "trezentos",
    "quatrocentos",
    "quinhentos",
    "seiscentos",
    "setecentos",
    "oitocentos",
    "novecentos",
];

fn pt_merge(curr: &Pair, next: &Pair) -> Pair {
    let (mut ctext, cnum) = (curr.0.clone(), curr.1);
    let (mut ntext, nnum) = (next.0.clone(), next.1);
    if cnum == 1 {
        if nnum < 1_000_000 {
            return next.clone();
        }
        ctext = "um".into();
    } else if cnum == 100 && nnum % 1000 != 0 {
        ctext = "cento".into();
    }
    if nnum < cnum {
        return (format!("{ctext} e {ntext}"), cnum + nnum);
    }
    if nnum % 1_000_000_000 == 0 && cnum > 1 {
        ntext = format!("{}liões", drop_chars(&ntext, 4));
    } else if nnum % 1_000_000 == 0 && cnum > 1 {
        ntext = format!("{}lhões", drop_chars(&ntext, 4));
    }
    if ntext == "milião" {
        ntext = "milhão".into();
    }
    if nnum == 100 {
        ctext = PT_HUNDREDS[cnum as usize].to_string();
        ntext.clear();
    } else {
        ntext = format!(" {ntext}");
    }
    (ctext + &ntext, cnum * nnum)
}

/// Drops the "e" between a group and the hundreds after it: "mil e cento e catorze"
/// becomes "mil cento e catorze". The "e" stays when no other "e" follows.
fn pt_join(mut result: String) -> String {
    for group in [
        "mil",
        "milhão",
        "milhões",
        "mil milhões",
        "bilião",
        "biliões",
        "mil biliões",
    ] {
        let marker = format!("{group} e ");
        let matched = result.match_indices(&marker).any(|(at, _)| {
            let after = &result[at + marker.len()..];
            let word_end = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let word = &after[..word_end];
            let rest = &after[word_end..];
            (word.ends_with("ento") || word.ends_with("entos"))
                && rest.strip_prefix(' ').is_some_and(|r| r.contains('e'))
        });
        if matched {
            result = result.replace(&format!("{group} e"), group);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

    #[test]
    fn numbers_es() {
        for (n, words) in [
            (0, "cero"),
            (1, "uno"),
            (2, "dos"),
            (3, "tres"),
            (4, "cuatro"),
            (5, "cinco"),
            (6, "seis"),
            (7, "siete"),
            (8, "ocho"),
            (9, "nueve"),
            (10, "diez"),
            (11, "once"),
            (12, "doce"),
            (13, "trece"),
            (14, "catorce"),
            (15, "quince"),
            (16, "dieciséis"),
            (17, "diecisiete"),
            (18, "dieciocho"),
            (19, "diecinueve"),
            (20, "veinte"),
            (21, "veintiuno"),
            (22, "veintidós"),
            (23, "veintitrés"),
            (24, "veinticuatro"),
            (25, "veinticinco"),
            (30, "treinta"),
            (40, "cuarenta"),
            (50, "cincuenta"),
            (60, "sesenta"),
            (70, "setenta"),
            (80, "ochenta"),
            (90, "noventa"),
            (99, "noventa y nueve"),
            (100, "cien"),
            (101, "ciento uno"),
            (110, "ciento diez"),
            (200, "doscientos"),
            (999, "novecientos noventa y nueve"),
            (1_000, "mil"),
            (1_001, "mil uno"),
            (1_100, "mil cien"),
            (1_956, "mil novecientos cincuenta y seis"),
            (2_000, "dos mil"),
            (2_026, "dos mil veintiséis"),
            (10_000, "diez mil"),
            (12_345, "doce mil trescientos cuarenta y cinco"),
            (100_000, "cien mil"),
            (1_000_000, "un millón"),
            (2_500_000, "dos millones quinientos mil"),
            (
                1_234_567,
                "un millón doscientos treinta y cuatro mil quinientos sesenta y siete",
            ),
            (1_000_000_000, "mil millones"),
            // num2words writes "veintiuno mil". "uno" becomes "un" in front of "mil".
            (21_000, "veintiún mil"),
            (101_000, "ciento un mil"),
            (31_000_000, "treinta y un millones"),
        ] {
            assert_eq!(to_words(n, "es").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_fr() {
        for (n, words) in [
            (0, "zéro"),
            (1, "un"),
            (2, "deux"),
            (3, "trois"),
            (4, "quatre"),
            (5, "cinq"),
            (6, "six"),
            (7, "sept"),
            (8, "huit"),
            (9, "neuf"),
            (10, "dix"),
            (11, "onze"),
            (12, "douze"),
            (13, "treize"),
            (14, "quatorze"),
            (15, "quinze"),
            (16, "seize"),
            (17, "dix-sept"),
            (18, "dix-huit"),
            (19, "dix-neuf"),
            (20, "vingt"),
            (21, "vingt et un"),
            (22, "vingt-deux"),
            (23, "vingt-trois"),
            (24, "vingt-quatre"),
            (25, "vingt-cinq"),
            (30, "trente"),
            (40, "quarante"),
            (50, "cinquante"),
            (60, "soixante"),
            (70, "soixante-dix"),
            (80, "quatre-vingts"),
            (90, "quatre-vingt-dix"),
            (99, "quatre-vingt-dix-neuf"),
            (100, "cent"),
            (101, "cent un"),
            (110, "cent dix"),
            (200, "deux cents"),
            (999, "neuf cent quatre-vingt-dix-neuf"),
            (1_000, "mille"),
            (1_001, "mille un"),
            (1_100, "mille cent"),
            (1_956, "mille neuf cent cinquante-six"),
            (2_000, "deux mille"),
            (2_026, "deux mille vingt-six"),
            (10_000, "dix mille"),
            (12_345, "douze mille trois cent quarante-cinq"),
            (100_000, "cent mille"),
            (1_000_000, "un million"),
            (2_500_000, "deux millions cinq cent mille"),
            (
                1_234_567,
                "un million deux cent trente-quatre mille cinq cent soixante-sept",
            ),
            (1_000_000_000, "un milliard"),
        ] {
            assert_eq!(to_words(n, "fr").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_it() {
        for (n, words) in [
            (0, "zero"),
            (1, "uno"),
            (2, "due"),
            (3, "tre"),
            (4, "quattro"),
            (5, "cinque"),
            (6, "sei"),
            (7, "sette"),
            (8, "otto"),
            (9, "nove"),
            (10, "dieci"),
            (11, "undici"),
            (12, "dodici"),
            (13, "tredici"),
            (14, "quattordici"),
            (15, "quindici"),
            (16, "sedici"),
            (17, "diciassette"),
            (18, "diciotto"),
            (19, "diciannove"),
            (20, "venti"),
            (21, "ventuno"),
            (22, "ventidue"),
            (23, "ventitré"),
            (24, "ventiquattro"),
            (25, "venticinque"),
            (30, "trenta"),
            (40, "quaranta"),
            (50, "cinquanta"),
            (60, "sessanta"),
            (70, "settanta"),
            (80, "ottanta"),
            (90, "novanta"),
            (99, "novantanove"),
            (100, "cento"),
            (101, "centouno"),
            (110, "centodieci"),
            (200, "duecento"),
            (999, "novecentonovantanove"),
            (1_000, "mille"),
            (1_001, "milleuno"),
            (1_100, "millecento"),
            (1_956, "millenovecentocinquantasei"),
            (2_000, "duemila"),
            (2_026, "duemilaventisei"),
            (10_000, "diecimila"),
            (12_345, "dodicimilatrecentoquarantacinque"),
            (100_000, "centomila"),
            (1_000_000, "un milione"),
            (2_500_000, "due milioni e cinquecentomila"),
            (
                1_234_567,
                "un milione e duecentotrentaquattromilacinquecentosessantasette",
            ),
            (1_000_000_000, "un miliardo"),
            // num2words puts a comma after "miliardo". The comma is left out.
            (1_002_500_000, "un miliardo due milioni e cinquecentomila"),
        ] {
            assert_eq!(to_words(n, "it").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_pt() {
        for (n, words) in [
            (0, "zero"),
            (1, "um"),
            (2, "dois"),
            (3, "três"),
            (4, "quatro"),
            (5, "cinco"),
            (6, "seis"),
            (7, "sete"),
            (8, "oito"),
            (9, "nove"),
            (10, "dez"),
            (11, "onze"),
            (12, "doze"),
            (13, "treze"),
            (14, "catorze"),
            (15, "quinze"),
            (16, "dezasseis"),
            (17, "dezassete"),
            (18, "dezoito"),
            (19, "dezanove"),
            (20, "vinte"),
            (21, "vinte e um"),
            (22, "vinte e dois"),
            (23, "vinte e três"),
            (24, "vinte e quatro"),
            (25, "vinte e cinco"),
            (30, "trinta"),
            (40, "quarenta"),
            (50, "cinquenta"),
            (60, "sessenta"),
            (70, "setenta"),
            (80, "oitenta"),
            (90, "noventa"),
            (99, "noventa e nove"),
            (100, "cem"),
            (101, "cento e um"),
            (110, "cento e dez"),
            (200, "duzentos"),
            (999, "novecentos e noventa e nove"),
            (1_000, "mil"),
            (1_001, "mil e um"),
            (1_100, "mil e cem"),
            (1_956, "mil novecentos e cinquenta e seis"),
            (2_000, "dois mil"),
            (2_026, "dois mil e vinte e seis"),
            (10_000, "dez mil"),
            (12_345, "doze mil trezentos e quarenta e cinco"),
            (100_000, "cem mil"),
            (1_000_000, "um milhão"),
            (2_500_000, "dois milhões e quinhentos mil"),
            (
                1_234_567,
                "um milhão duzentos e trinta e quatro mil quinhentos e sessenta e sete",
            ),
            (1_000_000_000, "mil milhões"),
        ] {
            assert_eq!(to_words(n, "pt").unwrap(), words, "{n}");
        }
    }
}
