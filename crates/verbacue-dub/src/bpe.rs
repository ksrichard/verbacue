//! The XTTS-v2 tokenizer: byte pair encoding (BPE) over `vocab.json`.
//!
//! `vocab.json` is a Hugging Face `tokenizers` file. This code does the same steps as
//! that library for this one file:
//! 1. Find the special tokens (`[SPACE]`, `[hu]`, ...) in the text.
//! 2. Split the rest into words and punctuation (the `Whitespace` pre-tokenizer).
//! 3. Start each word as single characters, then apply the merges in rank order.

use std::{collections::HashMap, path::Path};

use serde_json::Value;
use verbacue_core::{Context as _, Result};

use crate::text;

const UNK: i64 = 1;

/// The XTTS-v2 tokenizer.
pub struct Tokenizer {
    vocab: HashMap<String, i64>,
    /// (left id, right id) to (rank, merged id).
    merges: HashMap<(i64, i64), (usize, i64)>,
    /// The special tokens, longest first.
    special: Vec<(String, i64)>,
}

impl Tokenizer {
    /// Loads `vocab.json`.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or is not a BPE tokenizer file.
    pub fn load(vocab_json: &Path) -> Result<Tokenizer> {
        let json = std::fs::read_to_string(vocab_json)
            .with_context(|| format!("cannot read {}", vocab_json.display()))?;
        Self::parse(&json)
    }

    fn parse(json: &str) -> Result<Tokenizer> {
        let root: Value = serde_json::from_str(json).context("vocab.json is not JSON")?;
        let model = &root["model"];
        let vocab: HashMap<String, i64> = model["vocab"]
            .as_object()
            .context("vocab.json has no model.vocab")?
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_i64()?)))
            .collect();

        let mut merges = HashMap::new();
        for (rank, m) in model["merges"]
            .as_array()
            .context("vocab.json has no model.merges")?
            .iter()
            .enumerate()
        {
            // Old files write a merge as "a b", new files as ["a", "b"].
            let (a, b) = match m {
                Value::String(s) => s.split_once(' ').context("bad merge")?,
                Value::Array(pair) => (
                    pair.first().and_then(Value::as_str).context("bad merge")?,
                    pair.get(1).and_then(Value::as_str).context("bad merge")?,
                ),
                _ => continue,
            };
            let ids = (vocab.get(a), vocab.get(b), vocab.get(&format!("{a}{b}")));
            if let (Some(&a), Some(&b), Some(&ab)) = ids {
                merges.insert((a, b), (rank, ab));
            }
        }

        let mut special: Vec<(String, i64)> = root["added_tokens"]
            .as_array()
            .context("vocab.json has no added_tokens")?
            .iter()
            .filter_map(|t| Some((t["content"].as_str()?.to_string(), t["id"].as_i64()?)))
            .collect();
        special.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        Ok(Tokenizer {
            vocab,
            merges,
            special,
        })
    }

    /// The token ids for a clean text, as coqui's `VoiceBpeTokenizer.encode`. It puts
    /// `[<lang>]` in front and writes each space as `[SPACE]`.
    pub fn encode(&self, text: &str, lang: &str) -> Vec<i64> {
        let code = text::xtts_code(lang).unwrap_or(lang);
        let text = format!("[{code}]{text}").replace(' ', "[SPACE]");
        let mut ids = Vec::new();
        let mut rest = text.as_str();
        while !rest.is_empty() {
            let next = self
                .special
                .iter()
                .filter_map(|(s, id)| rest.find(s.as_str()).map(|at| (at, s.len(), *id)))
                .min_by_key(|&(at, len, _)| (at, std::cmp::Reverse(len)));
            let (plain, after) = match next {
                Some((at, len, id)) => {
                    self.encode_plain(&rest[..at], &mut ids);
                    ids.push(id);
                    ("", &rest[at + len..])
                }
                None => (rest, ""),
            };
            self.encode_plain(plain, &mut ids);
            rest = after;
        }
        ids
    }

    fn encode_plain(&self, text: &str, ids: &mut Vec<i64>) {
        for word in pre_tokenize(text) {
            self.encode_word(word, ids);
        }
    }

    fn encode_word(&self, word: &str, ids: &mut Vec<i64>) {
        let mut buf = [0u8; 4];
        let mut symbols: Vec<i64> = word
            .chars()
            .map(|c| *self.vocab.get(&*c.encode_utf8(&mut buf)).unwrap_or(&UNK))
            .collect();
        // Merge the pair with the lowest rank, the leftmost first, until none is left.
        loop {
            let best = symbols
                .windows(2)
                .enumerate()
                .filter_map(|(i, w)| self.merges.get(&(w[0], w[1])).map(|&(r, id)| (r, i, id)))
                .min();
            let Some((_, i, id)) = best else { break };
            symbols[i] = id;
            symbols.remove(i + 1);
        }
        ids.extend(symbols);
    }
}

/// The `Whitespace` pre-tokenizer: the regex `\w+|[^\w\s]+`.
fn pre_tokenize(text: &str) -> Vec<&str> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Other,
        Space,
    }
    let class = |c: char| {
        if is_word_char(c) {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut current = Class::Space;
    for (i, c) in text.char_indices() {
        let k = class(c);
        if k != current {
            if current != Class::Space && start < i {
                out.push(&text[start..i]);
            }
            start = i;
            current = k;
        }
    }
    if current != Class::Space && start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `\w` in the regex: letters, marks, digits and `_`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || is_mark(c)
}

/// The combining marks of the scripts in the XTTS-v2 vocabulary that Rust does not
/// count as alphabetic, such as the Devanagari virama.
fn is_mark(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{0483}'..='\u{0489}'
        | '\u{0610}'..='\u{061A}'
        | '\u{064B}'..='\u{065F}'
        | '\u{0670}'
        | '\u{06D6}'..='\u{06DC}'
        | '\u{06DF}'..='\u{06E4}'
        | '\u{06E7}'..='\u{06E8}'
        | '\u{06EA}'..='\u{06ED}'
        | '\u{0900}'..='\u{0903}'
        | '\u{093A}'..='\u{094F}'
        | '\u{0951}'..='\u{0957}'
        | '\u{0962}'..='\u{0963}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{3099}'..='\u{309A}'
        | '\u{FE20}'..='\u{FE2F}')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = r#"{
        "added_tokens": [
            {"id": 0, "content": "[STOP]"}, {"id": 1, "content": "[UNK]"},
            {"id": 2, "content": "[SPACE]"}, {"id": 9, "content": "[hu]"}
        ],
        "model": {
            "vocab": {"[STOP]": 0, "[UNK]": 1, "[SPACE]": 2, "a": 3, "b": 4, "c": 5,
                      "ab": 6, "abc": 7, ",": 8, "[hu]": 9},
            "merges": ["a b", "ab c"]
        }
    }"#;

    #[test]
    fn merges_and_special_tokens() {
        let t = Tokenizer::parse(SMALL).unwrap();
        // [hu] abc [SPACE] ab , [SPACE] c x(unknown)
        assert_eq!(t.encode("abc ab, cx", "hu"), [9, 7, 2, 6, 8, 2, 5, 1]);
    }

    #[test]
    fn pre_tokenizer_splits_words_and_punctuation() {
        assert_eq!(
            pre_tokenize("kívánok, hogy?!"),
            ["kívánok", ",", "hogy", "?!"]
        );
        assert_eq!(pre_tokenize("नमस्ते"), ["नमस्ते"]);
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn tokens_match_python() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out");
        let t = Tokenizer::load(&out.join("vocab.json")).unwrap();
        let cases: Value = serde_json::from_str(
            &std::fs::read_to_string(out.join("fixtures/tokens.json")).unwrap(),
        )
        .unwrap();
        for case in cases.as_array().unwrap() {
            let expected: Vec<i64> = case["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
                .collect();
            let clean = case["clean"].as_str().unwrap();
            let lang = case["lang"].as_str().unwrap();
            assert_eq!(t.encode(clean, lang), expected, "{lang}: {clean}");
        }
    }
}
