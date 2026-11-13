//! Chinese, Japanese and Korean number words.

use super::cards::{Cards, Pair};

const ZH_DIGITS: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/// One group of four digits, such as "一千九百五十六". `lead` is true for the
/// highest group, where "两" is used for 2 in front of a unit.
fn zh_below_10000(n: u64) -> String {
    let mut s = String::new();
    let mut zero = false;
    for (unit, value) in [("千", 1000), ("百", 100), ("十", 10), ("", 1)] {
        let d = (n / value % 10) as usize;
        if d == 0 {
            zero = !s.is_empty();
            continue;
        }
        if zero {
            s.push('零');
            zero = false;
        }
        s.push(if d == 2 && unit == "千" {
            '两'
        } else {
            ZH_DIGITS[d]
        });
        s.push_str(unit);
    }
    s
}

pub(super) fn zh(n: u64) -> String {
    if n == 0 {
        return "零".into();
    }
    const UNITS: [&str; 5] = ["", "万", "亿", "万亿", "亿亿"];
    let mut groups = Vec::new();
    let mut rest = n;
    while rest > 0 {
        groups.push(rest % 10_000);
        rest /= 10_000;
    }
    let mut s = String::new();
    let mut gap = false;
    for (i, &g) in groups.iter().enumerate().rev() {
        if g == 0 {
            gap = !s.is_empty();
            continue;
        }
        if !s.is_empty() && (gap || g < 1000) {
            s.push('零');
        }
        gap = false;
        let mut words = zh_below_10000(g);
        if g == 2 && i > 0 {
            words = "两".into();
        }
        s.push_str(&words);
        s.push_str(UNITS[i]);
    }
    // "一十五" is said "十五" at the start of a number.
    match s.strip_prefix("一十") {
        Some(rest) => format!("十{rest}"),
        None => s,
    }
}

// Japanese ----------------------------------------------------------------------------

/// Kanji, as num2words writes them. Coqui keeps digits for Japanese, but the romanize
/// step can read kanji, so the voice gets words.
pub(super) fn ja(n: u64) -> String {
    let cards = Cards::new(
        &[
            (10_000_000_000_000_000, "京"),
            (1_000_000_000_000, "兆"),
            (100_000_000, "億"),
            (10_000, "万"),
        ],
        &[(1000, "千"), (100, "百")],
        &[
            "零", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十",
        ],
    );
    cards.cardinal(n, ja_merge)
}

fn ja_merge(left: &Pair, right: &Pair) -> Pair {
    let (lnum, rnum) = (left.1, right.1);
    // "百" and "千" have no "一" in front, but "一万" has one.
    if lnum == 1 && rnum < 10_000 {
        return right.clone();
    }
    let value = if lnum > rnum {
        lnum + rnum
    } else {
        lnum * rnum
    };
    (format!("{}{}", left.0, right.0), value)
}

// Korean ------------------------------------------------------------------------------

/// Sino-Korean in Hangul. The groups of four digits are separated by spaces after 만.
pub(super) fn ko(n: u64) -> String {
    let cards = Cards::new(
        &[
            (10_000_000_000_000_000, "경"),
            (1_000_000_000_000, "조"),
            (100_000_000, "억"),
            (10_000, "만"),
        ],
        &[(1000, "천"), (100, "백")],
        &[
            "영", "일", "이", "삼", "사", "오", "육", "칠", "팔", "구", "십",
        ],
    );
    cards.cardinal(n, ko_merge)
}

fn ko_merge(left: &Pair, right: &Pair) -> Pair {
    let (lnum, rnum) = (left.1, right.1);
    if lnum == 1 && rnum <= 10_000 {
        right.clone()
    } else if lnum > rnum && lnum < 10_000 {
        (format!("{}{}", left.0, right.0), lnum + rnum)
    } else if lnum > rnum {
        (format!("{} {}", left.0, right.0), lnum + rnum)
    } else {
        (format!("{}{}", left.0, right.0), lnum * rnum)
    }
}

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

    #[test]
    fn numbers_zh() {
        for (n, words) in [
            (0, "零"),
            (2, "二"),
            (10, "十"),
            (15, "十五"),
            (105, "一百零五"),
            (110, "一百一十"),
            (1956, "一千九百五十六"),
            (2002, "两千零二"),
            (10_010, "一万零一十"),
            (20_003, "两万零三"),
            (100_000, "十万"),
            (1_000_000, "一百万"),
            (123_456_789, "一亿两千三百四十五万六千七百八十九"),
            (200_000_000, "两亿"),
        ] {
            assert_eq!(to_words(n, "zh-cn").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_ja() {
        for (n, words) in [
            (0, "零"),
            (1, "一"),
            (2, "二"),
            (3, "三"),
            (4, "四"),
            (5, "五"),
            (6, "六"),
            (7, "七"),
            (8, "八"),
            (9, "九"),
            (10, "十"),
            (11, "十一"),
            (12, "十二"),
            (13, "十三"),
            (14, "十四"),
            (15, "十五"),
            (16, "十六"),
            (17, "十七"),
            (18, "十八"),
            (19, "十九"),
            (20, "二十"),
            (21, "二十一"),
            (22, "二十二"),
            (23, "二十三"),
            (24, "二十四"),
            (25, "二十五"),
            (30, "三十"),
            (40, "四十"),
            (50, "五十"),
            (60, "六十"),
            (70, "七十"),
            (80, "八十"),
            (90, "九十"),
            (99, "九十九"),
            (100, "百"),
            (101, "百一"),
            (110, "百十"),
            (200, "二百"),
            (999, "九百九十九"),
            (1_000, "千"),
            (1_001, "千一"),
            (1_100, "千百"),
            (1_956, "千九百五十六"),
            (2_000, "二千"),
            (2_026, "二千二十六"),
            (10_000, "一万"),
            (12_345, "一万二千三百四十五"),
            (100_000, "十万"),
            (1_000_000, "百万"),
            (2_500_000, "二百五十万"),
            (1_234_567, "百二十三万四千五百六十七"),
            (1_000_000_000, "十億"),
        ] {
            assert_eq!(to_words(n, "ja").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_ko() {
        for (n, words) in [
            (0, "영"),
            (1, "일"),
            (2, "이"),
            (3, "삼"),
            (4, "사"),
            (5, "오"),
            (6, "육"),
            (7, "칠"),
            (8, "팔"),
            (9, "구"),
            (10, "십"),
            (11, "십일"),
            (12, "십이"),
            (13, "십삼"),
            (14, "십사"),
            (15, "십오"),
            (16, "십육"),
            (17, "십칠"),
            (18, "십팔"),
            (19, "십구"),
            (20, "이십"),
            (21, "이십일"),
            (22, "이십이"),
            (23, "이십삼"),
            (24, "이십사"),
            (25, "이십오"),
            (30, "삼십"),
            (40, "사십"),
            (50, "오십"),
            (60, "육십"),
            (70, "칠십"),
            (80, "팔십"),
            (90, "구십"),
            (99, "구십구"),
            (100, "백"),
            (101, "백일"),
            (110, "백십"),
            (200, "이백"),
            (999, "구백구십구"),
            (1_000, "천"),
            (1_001, "천일"),
            (1_100, "천백"),
            (1_956, "천구백오십육"),
            (2_000, "이천"),
            (2_026, "이천이십육"),
            (10_000, "만"),
            (12_345, "만 이천삼백사십오"),
            (100_000, "십만"),
            (1_000_000, "백만"),
            (2_500_000, "이백오십만"),
            (1_234_567, "백이십삼만 사천오백육십칠"),
            (1_000_000_000, "십억"),
        ] {
            assert_eq!(to_words(n, "ko").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn ja_kanji_are_read_as_latin_letters() {
        for (n, latin) in [
            (2026, "ni sen ni juu roku"),
            (100_000, "juu man"),
            (
                1_234_567,
                "hyaku ni juu san man yon sen go hyaku roku juu nana",
            ),
        ] {
            let words = to_words(n, "ja").unwrap();
            let read = crate::romanize::to_latin(&words, "ja");
            assert_eq!(read, latin, "{n}");
            assert!(read.chars().all(|c| c.is_ascii_lowercase() || c == ' '));
        }
    }
}
