//! Polish, Czech and Russian number words. All three read groups of three digits and
//! choose the form of the group word from the group value.

/// Words for one language. Index 0 of `ones`, `tens`, `twenties` and `hundreds` is not
/// used, except `tens[0]` ("ten").
struct Words {
    zero: &'static str,
    ones: [&'static str; 10],
    tens: [&'static str; 10],
    twenties: [&'static str; 10],
    hundreds: [&'static str; 10],
    /// Singular, "2 to 4" and "5 and more" forms, for 10^3, 10^6 and so on.
    scales: &'static [[&'static str; 3]],
    /// The ones in front of "thousand". Russian uses the feminine here.
    thousand_ones: [&'static str; 10],
    /// Polish and Czech drop the "one" in front of a scale word ("tysiąc", "milion").
    drop_one: bool,
    /// Which of the three scale forms goes with a group value.
    form: fn(u64) -> usize,
}

/// The groups of three digits of `n`, largest first.
fn groups(n: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut rest = n;
    while rest > 0 {
        out.push(rest % 1000);
        rest /= 1000;
    }
    out.reverse();
    out
}

const PL_ONES: [&str; 10] = [
    "",
    "jeden",
    "dwa",
    "trzy",
    "cztery",
    "pięć",
    "sześć",
    "siedem",
    "osiem",
    "dziewięć",
];
const CS_ONES: [&str; 10] = [
    "", "jedna", "dva", "tři", "čtyři", "pět", "šest", "sedm", "osm", "devět",
];

fn west_form(n: u64) -> usize {
    if n == 1 {
        0
    } else if (2..5).contains(&(n % 10)) && !(10..=20).contains(&(n % 100)) {
        1
    } else {
        2
    }
}

fn read(n: u64, w: &Words) -> String {
    if n == 0 {
        return w.zero.to_string();
    }
    let groups = groups(n);
    let mut words: Vec<&str> = Vec::new();
    for (index, &x) in groups.iter().enumerate() {
        let level = groups.len() - 1 - index;
        if x == 0 {
            continue;
        }
        let (h, t, o) = (
            (x / 100) as usize,
            (x / 10 % 10) as usize,
            (x % 10) as usize,
        );
        if h > 0 {
            words.push(w.hundreds[h]);
        }
        if t > 1 {
            words.push(w.twenties[t]);
        }
        if t == 1 {
            words.push(w.tens[o]);
        } else if o > 0 && !(w.drop_one && level > 0 && x == 1) {
            words.push(if level == 1 {
                w.thousand_ones[o]
            } else {
                w.ones[o]
            });
        }
        if level > 0 {
            words.push(w.scales[level - 1][(w.form)(x)]);
        }
    }
    words.join(" ")
}

pub(super) fn pl(n: u64) -> String {
    read(
        n,
        &Words {
            zero: "zero",
            ones: PL_ONES,
            tens: [
                "dziesięć",
                "jedenaście",
                "dwanaście",
                "trzynaście",
                "czternaście",
                "piętnaście",
                "szesnaście",
                "siedemnaście",
                "osiemnaście",
                "dziewiętnaście",
            ],
            twenties: [
                "",
                "",
                "dwadzieścia",
                "trzydzieści",
                "czterdzieści",
                "pięćdziesiąt",
                "sześćdziesiąt",
                "siedemdziesiąt",
                "osiemdziesiąt",
                "dziewięćdziesiąt",
            ],
            hundreds: [
                "",
                "sto",
                "dwieście",
                "trzysta",
                "czterysta",
                "pięćset",
                "sześćset",
                "siedemset",
                "osiemset",
                "dziewięćset",
            ],
            scales: &[
                ["tysiąc", "tysiące", "tysięcy"],
                ["milion", "miliony", "milionów"],
                ["miliard", "miliardy", "miliardów"],
                ["bilion", "biliony", "bilionów"],
                ["biliard", "biliardy", "biliardów"],
                ["trylion", "tryliony", "trylionów"],
            ],
            thousand_ones: PL_ONES,
            drop_one: true,
            form: west_form,
        },
    )
}

pub(super) fn cs(n: u64) -> String {
    read(
        n,
        &Words {
            zero: "nula",
            ones: CS_ONES,
            tens: [
                "deset",
                "jedenáct",
                "dvanáct",
                "třináct",
                "čtrnáct",
                "patnáct",
                "šestnáct",
                "sedmnáct",
                "osmnáct",
                "devatenáct",
            ],
            twenties: [
                "",
                "",
                "dvacet",
                "třicet",
                "čtyřicet",
                "padesát",
                "šedesát",
                "sedmdesát",
                "osmdesát",
                "devadesát",
            ],
            hundreds: [
                "",
                "sto",
                "dvěstě",
                "třista",
                "čtyřista",
                "pětset",
                "šestset",
                "sedmset",
                "osmset",
                "devětset",
            ],
            scales: &[
                ["tisíc", "tisíce", "tisíc"],
                ["milion", "miliony", "milionů"],
                ["miliarda", "miliardy", "miliard"],
                ["bilion", "biliony", "bilionů"],
                ["biliarda", "biliardy", "biliard"],
                ["trilion", "triliony", "trilionů"],
            ],
            thousand_ones: CS_ONES,
            drop_one: true,
            form: west_form,
        },
    )
}

fn ru_form(n: u64) -> usize {
    if (11..=14).contains(&(n % 100)) {
        2
    } else if n % 10 == 1 {
        0
    } else if (2..5).contains(&(n % 10)) {
        1
    } else {
        2
    }
}

pub(super) fn ru(n: u64) -> String {
    let words = Words {
        zero: "ноль",
        ones: [
            "",
            "один",
            "два",
            "три",
            "четыре",
            "пять",
            "шесть",
            "семь",
            "восемь",
            "девять",
        ],
        tens: [
            "десять",
            "одиннадцать",
            "двенадцать",
            "тринадцать",
            "четырнадцать",
            "пятнадцать",
            "шестнадцать",
            "семнадцать",
            "восемнадцать",
            "девятнадцать",
        ],
        twenties: [
            "",
            "",
            "двадцать",
            "тридцать",
            "сорок",
            "пятьдесят",
            "шестьдесят",
            "семьдесят",
            "восемьдесят",
            "девяносто",
        ],
        hundreds: [
            "",
            "сто",
            "двести",
            "триста",
            "четыреста",
            "пятьсот",
            "шестьсот",
            "семьсот",
            "восемьсот",
            "девятьсот",
        ],
        scales: &[
            ["тысяча", "тысячи", "тысяч"],
            ["миллион", "миллиона", "миллионов"],
            ["миллиард", "миллиарда", "миллиардов"],
            ["триллион", "триллиона", "триллионов"],
            ["квадриллион", "квадриллиона", "квадриллионов"],
            ["квинтиллион", "квинтиллиона", "квинтиллионов"],
        ],
        thousand_ones: [
            "",
            "одна",
            "две",
            "три",
            "четыре",
            "пять",
            "шесть",
            "семь",
            "восемь",
            "девять",
        ],
        drop_one: false,
        form: ru_form,
    };
    read(n, &words)
}

#[cfg(test)]
mod tests {
    use crate::numbers::to_words;

    #[test]
    fn numbers_pl() {
        for (n, words) in [
            (0, "zero"),
            (1, "jeden"),
            (2, "dwa"),
            (3, "trzy"),
            (4, "cztery"),
            (5, "pięć"),
            (6, "sześć"),
            (7, "siedem"),
            (8, "osiem"),
            (9, "dziewięć"),
            (10, "dziesięć"),
            (11, "jedenaście"),
            (12, "dwanaście"),
            (13, "trzynaście"),
            (14, "czternaście"),
            (15, "piętnaście"),
            (16, "szesnaście"),
            (17, "siedemnaście"),
            (18, "osiemnaście"),
            (19, "dziewiętnaście"),
            (20, "dwadzieścia"),
            (21, "dwadzieścia jeden"),
            (22, "dwadzieścia dwa"),
            (23, "dwadzieścia trzy"),
            (24, "dwadzieścia cztery"),
            (25, "dwadzieścia pięć"),
            (30, "trzydzieści"),
            (40, "czterdzieści"),
            (50, "pięćdziesiąt"),
            (60, "sześćdziesiąt"),
            (70, "siedemdziesiąt"),
            (80, "osiemdziesiąt"),
            (90, "dziewięćdziesiąt"),
            (99, "dziewięćdziesiąt dziewięć"),
            (100, "sto"),
            (101, "sto jeden"),
            (110, "sto dziesięć"),
            (200, "dwieście"),
            (999, "dziewięćset dziewięćdziesiąt dziewięć"),
            (1_000, "tysiąc"),
            (1_001, "tysiąc jeden"),
            (1_100, "tysiąc sto"),
            (1_956, "tysiąc dziewięćset pięćdziesiąt sześć"),
            (2_000, "dwa tysiące"),
            (2_026, "dwa tysiące dwadzieścia sześć"),
            (10_000, "dziesięć tysięcy"),
            (12_345, "dwanaście tysięcy trzysta czterdzieści pięć"),
            (100_000, "sto tysięcy"),
            (1_000_000, "milion"),
            (2_500_000, "dwa miliony pięćset tysięcy"),
            (
                1_234_567,
                "milion dwieście trzydzieści cztery tysiące pięćset sześćdziesiąt siedem",
            ),
            (1_000_000_000, "miliard"),
        ] {
            assert_eq!(to_words(n, "pl").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_cs() {
        for (n, words) in [
            (0, "nula"),
            (1, "jedna"),
            (2, "dva"),
            (3, "tři"),
            (4, "čtyři"),
            (5, "pět"),
            (6, "šest"),
            (7, "sedm"),
            (8, "osm"),
            (9, "devět"),
            (10, "deset"),
            (11, "jedenáct"),
            (12, "dvanáct"),
            (13, "třináct"),
            (14, "čtrnáct"),
            (15, "patnáct"),
            (16, "šestnáct"),
            (17, "sedmnáct"),
            (18, "osmnáct"),
            (19, "devatenáct"),
            (20, "dvacet"),
            (21, "dvacet jedna"),
            (22, "dvacet dva"),
            (23, "dvacet tři"),
            (24, "dvacet čtyři"),
            (25, "dvacet pět"),
            (30, "třicet"),
            (40, "čtyřicet"),
            (50, "padesát"),
            (60, "šedesát"),
            (70, "sedmdesát"),
            (80, "osmdesát"),
            (90, "devadesát"),
            (99, "devadesát devět"),
            (100, "sto"),
            (101, "sto jedna"),
            (110, "sto deset"),
            (200, "dvěstě"),
            (999, "devětset devadesát devět"),
            (1_000, "tisíc"),
            (1_001, "tisíc jedna"),
            (1_100, "tisíc sto"),
            (1_956, "tisíc devětset padesát šest"),
            (2_000, "dva tisíce"),
            (2_026, "dva tisíce dvacet šest"),
            (10_000, "deset tisíc"),
            (12_345, "dvanáct tisíc třista čtyřicet pět"),
            (100_000, "sto tisíc"),
            (1_000_000, "milion"),
            (2_500_000, "dva miliony pětset tisíc"),
            (
                1_234_567,
                "milion dvěstě třicet čtyři tisíce pětset šedesát sedm",
            ),
            (1_000_000_000, "miliarda"),
        ] {
            assert_eq!(to_words(n, "cs").unwrap(), words, "{n}");
        }
    }

    #[test]
    fn numbers_ru() {
        for (n, words) in [
            (0, "ноль"),
            (1, "один"),
            (2, "два"),
            (3, "три"),
            (4, "четыре"),
            (5, "пять"),
            (6, "шесть"),
            (7, "семь"),
            (8, "восемь"),
            (9, "девять"),
            (10, "десять"),
            (11, "одиннадцать"),
            (12, "двенадцать"),
            (13, "тринадцать"),
            (14, "четырнадцать"),
            (15, "пятнадцать"),
            (16, "шестнадцать"),
            (17, "семнадцать"),
            (18, "восемнадцать"),
            (19, "девятнадцать"),
            (20, "двадцать"),
            (21, "двадцать один"),
            (22, "двадцать два"),
            (23, "двадцать три"),
            (24, "двадцать четыре"),
            (25, "двадцать пять"),
            (30, "тридцать"),
            (40, "сорок"),
            (50, "пятьдесят"),
            (60, "шестьдесят"),
            (70, "семьдесят"),
            (80, "восемьдесят"),
            (90, "девяносто"),
            (99, "девяносто девять"),
            (100, "сто"),
            (101, "сто один"),
            (110, "сто десять"),
            (200, "двести"),
            (999, "девятьсот девяносто девять"),
            (1_000, "одна тысяча"),
            (1_001, "одна тысяча один"),
            (1_100, "одна тысяча сто"),
            (1_956, "одна тысяча девятьсот пятьдесят шесть"),
            (2_000, "две тысячи"),
            (2_026, "две тысячи двадцать шесть"),
            (10_000, "десять тысяч"),
            (12_345, "двенадцать тысяч триста сорок пять"),
            (100_000, "сто тысяч"),
            (1_000_000, "один миллион"),
            (2_500_000, "два миллиона пятьсот тысяч"),
            (
                1_234_567,
                "один миллион двести тридцать четыре тысячи пятьсот шестьдесят семь",
            ),
            (1_000_000_000, "один миллиард"),
        ] {
            assert_eq!(to_words(n, "ru").unwrap(), words, "{n}");
        }
    }
}
