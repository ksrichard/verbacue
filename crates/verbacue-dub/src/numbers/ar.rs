//! Arabic number words, as num2words writes them. The rules for "two" in front of a
//! scale word ("ألفان", "ألفا") are the same as in num2words.

const ONES: [&str; 20] = [
    "",
    "واحد",
    "اثنان",
    "ثلاثة",
    "أربعة",
    "خمسة",
    "ستة",
    "سبعة",
    "ثمانية",
    "تسعة",
    "عشرة",
    "أحد عشر",
    "اثنا عشر",
    "ثلاثة عشر",
    "أربعة عشر",
    "خمسة عشر",
    "ستة عشر",
    "سبعة عشر",
    "ثمانية عشر",
    "تسعة عشر",
];
const TENS: [&str; 8] = [
    "عشرون",
    "ثلاثون",
    "أربعون",
    "خمسون",
    "ستون",
    "سبعون",
    "ثمانون",
    "تسعون",
];
const HUNDREDS: [&str; 10] = [
    "",
    "مائة",
    "مئتان",
    "ثلاثمائة",
    "أربعمائة",
    "خمسمائة",
    "ستمائة",
    "سبعمائة",
    "ثمانمائة",
    "تسعمائة",
];
/// Index 0 is "200"; the others are 10^3, 10^6 and so on.
const TWOS_APPENDED: [&str; 7] = [
    "مئتا",
    "ألفا",
    "مليونا",
    "مليارا",
    "تريليونا",
    "كوادريليونا",
    "كوينتليونا",
];
const TWOS: [&str; 7] = [
    "مئتان",
    "ألفان",
    "مليونان",
    "ملياران",
    "تريليونان",
    "كوادريليونان",
    "كوينتليونان",
];
const GROUP: [&str; 7] = [
    "مائة",
    "ألف",
    "مليون",
    "مليار",
    "تريليون",
    "كوادريليون",
    "كوينتليون",
];
const GROUP_APPENDED: [&str; 7] = [
    "",
    "ألفاً",
    "مليوناً",
    "ملياراً",
    "تريليوناً",
    "كوادريليوناً",
    "كوينتليوناً",
];
const GROUP_PLURAL: [&str; 7] = [
    "",
    "آلاف",
    "ملايين",
    "مليارات",
    "تريليونات",
    "كوادريليونات",
    "كوينتليونات",
];

pub(super) fn ar(n: u64) -> String {
    if n == 0 {
        return "صفر".into();
    }
    let mut ret = String::new();
    let mut rest = n;
    let mut level = 0;
    while rest > 0 {
        let group = rest % 1000;
        rest /= 1000;
        let desc = group_words(group, level, n);
        if !desc.is_empty() {
            if level > 0 {
                if !ret.is_empty() {
                    ret = format!("و {ret}");
                }
                if group != 2 && group != 1 {
                    if group % 100 != 1 {
                        if (3..=10).contains(&group) {
                            ret = format!("{} {ret}", GROUP_PLURAL[level]);
                        } else if ret.is_empty() {
                            ret = format!("{} {ret}", GROUP[level]);
                        } else {
                            ret = format!("{} {ret}", GROUP_APPENDED[level]);
                        }
                    } else {
                        ret = format!("{} {ret}", GROUP[level]);
                    }
                }
            }
            ret = format!("{desc} {ret}");
        }
        level += 1;
    }
    ret.trim().to_string()
}

/// One group of three digits. `whole` is the full number.
fn group_words(group: u64, level: usize, whole: u64) -> String {
    let (tens, hundreds) = (group % 100, (group / 100) as usize);
    let mut ret = String::new();
    if hundreds > 0 {
        if tens == 0 && hundreds == 2 {
            ret.push_str(TWOS_APPENDED[0]);
        } else {
            ret.push_str(HUNDREDS[hundreds]);
            if tens != 0 {
                ret.push_str(" و ");
            }
        }
    }
    if tens == 0 {
        return ret;
    }
    if tens < 20 {
        if tens == 2 && hundreds == 0 && level > 0 {
            let power = whole.ilog10();
            let exact = whole > 10 && power.is_multiple_of(3) && whole == 2 * 10u64.pow(power);
            ret = if exact {
                TWOS_APPENDED[level]
            } else {
                TWOS[level]
            }
            .to_string();
        } else if tens == 1 && level > 0 {
            ret.push_str(GROUP[level]);
        } else {
            ret.push_str(ONES[tens as usize]);
        }
    } else {
        let (ones, ten) = ((tens % 10) as usize, (tens / 10 - 2) as usize);
        if ones > 0 {
            ret.push_str(ONES[ones]);
            ret.push_str(" و ");
        }
        ret.push_str(TENS[ten]);
    }
    ret
}

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

    #[test]
    fn numbers_ar() {
        for (n, words) in [
            (0, "صفر"),
            (1, "واحد"),
            (2, "اثنان"),
            (3, "ثلاثة"),
            (4, "أربعة"),
            (5, "خمسة"),
            (6, "ستة"),
            (7, "سبعة"),
            (8, "ثمانية"),
            (9, "تسعة"),
            (10, "عشرة"),
            (11, "أحد عشر"),
            (12, "اثنا عشر"),
            (13, "ثلاثة عشر"),
            (14, "أربعة عشر"),
            (15, "خمسة عشر"),
            (16, "ستة عشر"),
            (17, "سبعة عشر"),
            (18, "ثمانية عشر"),
            (19, "تسعة عشر"),
            (20, "عشرون"),
            (21, "واحد و عشرون"),
            (22, "اثنان و عشرون"),
            (23, "ثلاثة و عشرون"),
            (24, "أربعة و عشرون"),
            (25, "خمسة و عشرون"),
            (30, "ثلاثون"),
            (40, "أربعون"),
            (50, "خمسون"),
            (60, "ستون"),
            (70, "سبعون"),
            (80, "ثمانون"),
            (90, "تسعون"),
            (99, "تسعة و تسعون"),
            (100, "مائة"),
            (101, "مائة و واحد"),
            (110, "مائة و عشرة"),
            (200, "مئتا"),
            (999, "تسعمائة و تسعة و تسعون"),
            (1_000, "ألف"),
            (1_001, "ألف و واحد"),
            (1_100, "ألف و مائة"),
            (1_956, "ألف و تسعمائة و ستة و خمسون"),
            (2_000, "ألفا"),
            (2_026, "ألفان و ستة و عشرون"),
            (10_000, "عشرة آلاف"),
            (12_345, "اثنا عشر ألفاً و ثلاثمائة و خمسة و أربعون"),
            (100_000, "مائة ألف"),
            (1_000_000, "مليون"),
            (2_500_000, "مليونان و خمسمائة ألف"),
            (
                1_234_567,
                "مليون و مئتان و أربعة و ثلاثون ألفاً و خمسمائة و سبعة و ستون",
            ),
            (1_000_000_000, "مليار"),
        ] {
            assert_eq!(to_words(n, "ar").unwrap(), words, "{n}");
        }
    }
}
