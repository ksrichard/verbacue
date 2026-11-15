//! Chinese, Japanese and Korean to Latin letters.
//!
//! The XTTS-v2 vocabulary has almost no Hangul and few Chinese or Japanese
//! characters. Coqui converts these languages before the tokenizer, and XTTS-v2 was
//! trained on that output. The rules here follow coqui:
//! - Korean: hangul_romanize (academic transliteration), ported exactly.
//! - Chinese: pypinyin with the tone number at the end and `5` for the neutral tone.
//!   All punctuation becomes `,`, as coqui's `TextNorm` does.
//! - Japanese: cutlet (Hepburn). Cutlet uses the UniDic dictionary and we use IPADIC,
//!   so a few words get a different reading.

use std::{borrow::Cow, sync::OnceLock};

/// The text in Latin letters for `zh-cn`, `ja` and `ko`. Other languages are not
/// changed.
pub fn to_latin<'a>(text: &'a str, lang: &str) -> Cow<'a, str> {
    match lang {
        "ko" => Cow::Owned(korean(text)),
        "zh-cn" | "zh" => Cow::Owned(chinese(text)),
        "ja" => Cow::Owned(japanese(text)),
        _ => Cow::Borrowed(text),
    }
}

// Korean ---------------------------------------------------------------------------

const KO_INITIALS: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "l", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];
const KO_VOWELS: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];
const KO_FINALS: [&str; 28] = [
    "", "g", "kk", "gs", "n", "nj", "nh", "d", "l", "lg", "lm", "lb", "ls", "lt", "lp", "lh", "m",
    "b", "bs", "s", "ss", "ng", "j", "ch", "k", "t", "p", "h",
];

/// The (final, initial) letter groups that a reader can split in more than one way.
/// An example is "kk": "k" + "k", or "kk" + "". A hyphen goes between such syllables.
fn ko_ambiguous(joined: &str) -> bool {
    let splits = (0..joined.len())
        .filter(|&i| joined.is_char_boundary(i))
        .filter(|&i| KO_FINALS.contains(&&joined[..i]) && KO_INITIALS.contains(&&joined[i..]))
        .count();
    splits > 1
}

fn korean(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut prev_final: Option<usize> = None;
    for c in text.chars() {
        let code = c as u32;
        if !(0xAC00..=0xD7A3).contains(&code) {
            out.push(c);
            prev_final = None;
            continue;
        }
        let index = (code - 0xAC00) as usize;
        let (initial, vowel, last) = (index / 588, (index / 28) % 21, index % 28);
        if let Some(pf) = prev_final {
            let joined = format!("{}{}", KO_FINALS[pf], KO_INITIALS[initial]);
            if KO_INITIALS[initial].is_empty() || ko_ambiguous(&joined) {
                out.push('-');
            }
        }
        out.push_str(KO_INITIALS[initial]);
        out.push_str(KO_VOWELS[vowel]);
        out.push_str(KO_FINALS[last]);
        prev_final = Some(last);
    }
    out
}

// Chinese --------------------------------------------------------------------------

/// The punctuation that coqui's `TextNorm` changes to `,`.
fn zh_is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || "！？｡。＂＃＄％＆＇（）＊＋，－／：；＜＝＞＠［＼］＾＿｀｛｜｝～｟｠｢｣､、〃《》「」『』【】\
            〔〕〖〗〘〙〚〛〜〝〞〟〰〾〿–—‘’‛“”„‟…‧﹏·〈〉"
            .contains(c)
}

fn chinese(text: &str) -> String {
    use pinyin::ToPinyin as _;
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if zh_is_punct(c) {
            out.push(',');
            continue;
        }
        match c.to_pinyin() {
            Some(p) => {
                let syllable = p.with_tone_num_end();
                out.push_str(syllable);
                if !syllable.ends_with(|d: char| d.is_ascii_digit()) {
                    out.push('5');
                }
            }
            None => out.push(c),
        }
    }
    out
}

// Japanese -------------------------------------------------------------------------

fn segmenter() -> Option<&'static lindera::segmenter::Segmenter> {
    static SEGMENTER: OnceLock<Option<lindera::segmenter::Segmenter>> = OnceLock::new();
    SEGMENTER
        .get_or_init(|| {
            let dictionary = lindera::dictionary::load_dictionary("embedded://ipadic").ok()?;
            Some(lindera::segmenter::Segmenter::new(
                lindera::mode::Mode::Normal,
                dictionary,
                None,
            ))
        })
        .as_ref()
}

/// One word from the dictionary, with the IPADIC fields that the rules need.
struct Word {
    surface: String,
    pos1: String,
    pos2: String,
    /// The kana spelling, such as トウキョウ. Empty for an unknown word.
    reading: String,
    /// The spoken form, such as ワ for the particle は.
    pron: String,
}

/// Cutlet's list of words with a fixed spelling.
const JA_EXCEPTIONS: &[(&str, &str)] = &[
    ("ホッピー", "hoppy"),
    ("東京", "tokyo"),
    ("大阪", "osaka"),
    ("京都", "kyoto"),
    ("柔道", "judo"),
    ("弁当", "bento"),
    ("ヒルズ", "hills"),
    ("カービィ", "kirby"),
];

fn japanese(text: &str) -> String {
    let Some(segmenter) = segmenter() else {
        tracing::warn!("cannot load the Japanese dictionary; the text stays in Japanese");
        return text.to_string();
    };
    let Ok(tokens) = segmenter.segment(Cow::Borrowed(text)) else {
        return text.to_string();
    };
    let words: Vec<Word> = tokens
        .into_iter()
        .map(|mut t| {
            let d: Vec<String> = t
                .details()
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            let field = |i: usize| d.get(i).filter(|s| *s != "*").cloned().unwrap_or_default();
            Word {
                surface: t.surface.to_string(),
                pos1: field(0),
                pos2: field(1),
                reading: field(7),
                pron: field(8),
            }
        })
        .collect();
    ja_join(&words).to_lowercase()
}

/// Cutlet's `romaji_tokens`: the romaji of each word, and a space after a word unless
/// the next word belongs to it.
fn ja_join(words: &[Word]) -> String {
    let mut out: Vec<(String, bool)> = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        let next = words.get(i + 1);
        let roma = ja_word(w);
        // A word that ends in っ doubles the first letter of the next word.
        if let Some((prev, _)) = out.last_mut()
            && prev.ends_with('っ')
            && let Some(first) = roma.chars().next()
        {
            prev.pop();
            prev.push(first);
        }
        if matches!(w.surface.as_str(), "「" | "『") || matches!(roma.as_str(), "(" | "[") {
            if let Some(prev) = out.last_mut() {
                prev.1 = true;
            }
            out.push((roma, false));
            continue;
        }
        if w.surface.is_ascii() && next.is_some_and(|n| n.surface.is_ascii()) {
            out.push((w.surface.clone(), false));
            continue;
        }
        let space = !(w.pos1 == "接頭詞"
            || next.is_some_and(|n| {
                n.pos1 == "記号"
                    || n.pos2 == "接尾"
                    || n.pos2 == "接続助詞"
                    || n.surface == ","
                    || (matches!(w.pos1.as_str(), "動詞" | "助動詞" | "形容詞")
                        && n.pos1 == "助動詞"
                        && n.surface != "です")
            }));
        out.push((roma, space));
    }
    let mut s = String::new();
    for (roma, space) in out {
        s.push_str(&roma.replace('っ', ""));
        if space {
            s.push(' ');
        }
    }
    s.trim().to_string()
}

fn ja_word(w: &Word) -> String {
    if let Some((_, roma)) = JA_EXCEPTIONS.iter().find(|(k, _)| *k == w.surface) {
        return (*roma).to_string();
    }
    if w.surface.is_ascii() {
        return w.surface.clone();
    }
    if w.pos1 == "記号" {
        return w
            .surface
            .chars()
            .map(|c| ja_symbol(c).unwrap_or(""))
            .collect();
    }
    if w.pos1 == "助詞" {
        match w.pron.as_str() {
            "ワ" => return "wa".into(),
            "エ" => return "e".into(),
            _ => {}
        }
    }
    let kana = if w.reading.is_empty() {
        &w.surface
    } else {
        &w.reading
    };
    let hira: Vec<char> = kana.chars().map(to_hiragana).collect();
    if hira.iter().all(|c| is_kana(*c)) {
        ja_kana(&hira)
    } else {
        // An unknown word that is not kana, such as a rare kanji. Cutlet writes "?".
        String::new()
    }
}

fn to_hiragana(c: char) -> char {
    match c {
        'ァ'..='ヶ' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn is_kana(c: char) -> bool {
    matches!(c, 'ぁ'..='ゖ' | 'ー' | 'ゝ' | 'ゞ')
}

const SUTEGANA: &str = "ゃゅょぁぃぅぇぉ";

/// Cutlet's `map_kana` for Hepburn.
fn ja_kana(kana: &[char]) -> String {
    let mut out = String::new();
    for (i, &k) in kana.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| kana[p]);
        let next = kana.get(i + 1).copied();
        if let Some(p) = prev
            && let Some(r) = digraph(p, k)
        {
            out.push_str(r);
            continue;
        }
        if next.is_some_and(|n| digraph(k, n).is_some()) {
            continue;
        }
        if let Some(n) = next.filter(|n| SUTEGANA.contains(*n)) {
            if k != 'っ' {
                let base = hepburn(k);
                out.push_str(&base[..base.len().saturating_sub(1)]);
                out.push_str(hepburn(n));
            }
            continue;
        }
        if SUTEGANA.contains(k) {
            continue;
        }
        match k {
            'ー' => match prev.map(hepburn).and_then(|r| r.chars().last()) {
                Some(v) => out.push(v),
                None => out.push('-'),
            },
            'っ' => match next {
                Some('ち') => out.push('t'),
                Some(n) if "あいうえおっ".contains(n) => out.push('-'),
                Some(n) => out.extend(hepburn(n).chars().next()),
                None => out.push('っ'),
            },
            'ん' => {
                out.push('n');
                if next.is_some_and(|n| "あいうえおやゆよ".contains(n)) {
                    out.push('\'');
                }
            }
            'ゝ' => out.extend(prev.map(hepburn)),
            _ => out.push_str(hepburn(k)),
        }
    }
    out
}

fn digraph(a: char, b: char) -> Option<&'static str> {
    Some(match (a, b) {
        ('し', 'ゃ') => "sha",
        ('し', 'ゅ') => "shu",
        ('し', 'ょ') => "sho",
        ('じ', 'ゃ') => "ja",
        ('じ', 'ゅ') => "ju",
        ('じ', 'ょ') => "jo",
        ('ち', 'ゃ') => "cha",
        ('ち', 'ゅ') => "chu",
        ('ち', 'ょ') => "cho",
        _ => return None,
    })
}

fn hepburn(c: char) -> &'static str {
    const ROWS: [(&str, [&str; 5]); 15] = [
        ("あいうえお", ["a", "i", "u", "e", "o"]),
        ("かきくけこ", ["ka", "ki", "ku", "ke", "ko"]),
        ("がぎぐげご", ["ga", "gi", "gu", "ge", "go"]),
        ("さしすせそ", ["sa", "shi", "su", "se", "so"]),
        ("ざじずぜぞ", ["za", "ji", "zu", "ze", "zo"]),
        ("たちつてと", ["ta", "chi", "tsu", "te", "to"]),
        ("だぢづでど", ["da", "ji", "zu", "de", "do"]),
        ("なにぬねの", ["na", "ni", "nu", "ne", "no"]),
        ("はひふへほ", ["ha", "hi", "fu", "he", "ho"]),
        ("ばびぶべぼ", ["ba", "bi", "bu", "be", "bo"]),
        ("ぱぴぷぺぽ", ["pa", "pi", "pu", "pe", "po"]),
        ("まみむめも", ["ma", "mi", "mu", "me", "mo"]),
        ("らりるれろ", ["ra", "ri", "ru", "re", "ro"]),
        ("ぁぃぅぇぉ", ["a", "i", "u", "e", "o"]),
        ("ゐゑゔゎ　", ["wi", "we", "vu", "wa", ""]),
    ];
    for (kana, roma) in ROWS {
        if let Some(i) = kana.chars().position(|k| k == c) {
            return roma[i];
        }
    }
    match c {
        'や' | 'ゃ' => "ya",
        'ゆ' | 'ゅ' => "yu",
        'よ' | 'ょ' => "yo",
        'わ' => "wa",
        'を' => "wo",
        'ん' => "n",
        'ゕ' => "ka",
        'ゖ' => "ke",
        _ => "",
    }
}

fn ja_symbol(c: char) -> Option<&'static str> {
    Some(match c {
        '。' => ".",
        '、' | '，' => ",",
        '？' => "?",
        '！' => "!",
        '「' | '」' | '『' | '』' => "\"",
        '：' => ":",
        '（' | '《' => "(",
        '）' | '》' => ")",
        '【' => "[",
        '】' => "]",
        '・' => "/",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn korean_matches_hangul_romanize() {
        // From coqui's tools/xtts-export/romanize_fixtures.py output.
        for (text, latin) in [
            (
                "안녕하세요, 잘 지내세요?",
                "annyeonghase-yo, jal jinaese-yo?",
            ),
            (
                "저는 서울에 살고 있어요.",
                "jeoneun seo-ul-e salgo iss-eo-yo.",
            ),
            ("잠깐만 기다려 주세요!", "jamkkanman gidalyeo juse-yo!"),
            ("물엿을 샀어요.", "mul-yeos-eul sass-eo-yo."),
            ("감사합니다, 친구.", "gamsahabnida, chingu."),
        ] {
            assert_eq!(to_latin(text, "ko"), latin);
        }
    }

    #[test]
    fn chinese_uses_tone_numbers() {
        assert_eq!(to_latin("你好，朋友。", "zh-cn"), "ni3hao3,peng2you3,");
    }

    #[test]
    fn japanese_kana_rules() {
        let k = |s: &str| ja_kana(&s.chars().collect::<Vec<_>>());
        assert_eq!(k("ちょっと"), "chotto");
        assert_eq!(k("きっぷ"), "kippu");
        assert_eq!(k("まっちゃ"), "matcha");
        assert_eq!(k("しんよう"), "shin'you");
        assert_eq!(k("らーめん"), "raamen");
    }

    #[test]
    fn romanize_keeps_other_languages() {
        assert!(matches!(to_latin("jó napot", "hu"), Cow::Borrowed(_)));
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out/fixtures/romanize.json"]
    fn conversions_match_python() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/xtts-export/out/fixtures/romanize.json");
        let cases: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut misses = Vec::new();
        for lang in ["zh", "ja", "ko"] {
            let mut exact = 0;
            for case in cases
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["lang"] == lang)
            {
                let ours = to_latin(case["text"].as_str().unwrap(), lang);
                if ours == case["latin"].as_str().unwrap() {
                    exact += 1;
                } else {
                    misses.push(format!("{lang}: {ours:?} != {}", case["latin"]));
                }
            }
            eprintln!("{lang}: {exact} of 5 exact");
            // IPADIC reads some words in another way than UniDic, for example 私 as
            // "watashi" and 明日 as "ashita". pypinyin knows the neutral tone of 个 in 这个.
            let need = match lang {
                "ko" => 5,
                "zh" => 4,
                _ => 3,
            };
            assert!(exact >= need, "{lang}: {exact} of 5\n{}", misses.join("\n"));
        }
        eprintln!("{}", misses.join("\n"));
    }
}
