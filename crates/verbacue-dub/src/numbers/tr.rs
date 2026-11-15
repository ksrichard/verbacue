//! Turkish number words. Turkish writes a number as one word.

const ONES: [&str; 10] = [
    "", "bir", "iki", "üç", "dört", "beş", "altı", "yedi", "sekiz", "dokuz",
];
const TENS: [&str; 10] = [
    "", "on", "yirmi", "otuz", "kırk", "elli", "altmış", "yetmiş", "seksen", "doksan",
];
/// 10^3, 10^6 and so on.
const SCALES: [&str; 6] = [
    "bin",
    "milyon",
    "milyar",
    "trilyon",
    "katrilyon",
    "kentilyon",
];

pub(super) fn tr(n: u64) -> String {
    if n == 0 {
        return "sıfır".into();
    }
    let mut groups = Vec::new();
    let mut rest = n;
    while rest > 0 {
        groups.push(rest % 1000);
        rest /= 1000;
    }
    let mut out = String::new();
    for (level, &x) in groups.iter().enumerate().rev() {
        if x == 0 {
            continue;
        }
        // "bin", not "birbin". Other scales keep the "bir": "birmilyon".
        if level == 1 && x == 1 {
            out.push_str(SCALES[0]);
            continue;
        }
        let (h, t, o) = (
            (x / 100) as usize,
            (x / 10 % 10) as usize,
            (x % 10) as usize,
        );
        if h > 1 {
            out.push_str(ONES[h]);
        }
        if h > 0 {
            out.push_str("yüz");
        }
        out.push_str(TENS[t]);
        out.push_str(ONES[o]);
        if level > 0 {
            out.push_str(SCALES[level - 1]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

    #[test]
    fn numbers_tr() {
        for (n, words) in [
            (0, "sıfır"),
            (1, "bir"),
            (2, "iki"),
            (3, "üç"),
            (4, "dört"),
            (5, "beş"),
            (6, "altı"),
            (7, "yedi"),
            (8, "sekiz"),
            (9, "dokuz"),
            (10, "on"),
            (11, "onbir"),
            (12, "oniki"),
            (13, "onüç"),
            (14, "ondört"),
            (15, "onbeş"),
            (16, "onaltı"),
            (17, "onyedi"),
            (18, "onsekiz"),
            (19, "ondokuz"),
            (20, "yirmi"),
            (21, "yirmibir"),
            (22, "yirmiiki"),
            (23, "yirmiüç"),
            (24, "yirmidört"),
            (25, "yirmibeş"),
            (30, "otuz"),
            (40, "kırk"),
            (50, "elli"),
            (60, "altmış"),
            (70, "yetmiş"),
            (80, "seksen"),
            (90, "doksan"),
            (99, "doksandokuz"),
            (100, "yüz"),
            (101, "yüzbir"),
            (110, "yüzon"),
            (200, "ikiyüz"),
            (999, "dokuzyüzdoksandokuz"),
            (1_000, "bin"),
            (1_001, "binbir"),
            (1_100, "binyüz"),
            (1_956, "bindokuzyüzellialtı"),
            (2_000, "ikibin"),
            (2_026, "ikibinyirmialtı"),
            (10_000, "onbin"),
            (12_345, "onikibinüçyüzkırkbeş"),
            (100_000, "yüzbin"),
            (1_000_000, "birmilyon"),
            (2_500_000, "ikimilyonbeşyüzbin"),
            (1_234_567, "birmilyonikiyüzotuzdörtbinbeşyüzaltmışyedi"),
            (1_000_000_000, "birmilyar"),
            // num2words drops the "bir" in front of "bin" here ("yüzbinyüz..."). That reads as 100126, so the "bir" stays.
            (101_126, "yüzbirbinyüzyirmialtı"),
            // num2words drops "üç" here. The count of the millions is lost.
            (
                1_003_197_245,
                "birmilyarüçmilyonyüzdoksanyedibinikiyüzkırkbeş",
            ),
        ] {
            assert_eq!(to_words(n, "tr").unwrap(), words, "{n}");
        }
    }
}
