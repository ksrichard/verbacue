//! Subtitle translation with a language model.
//!
//! The cues go to the model in batches of [`BATCH_SIZE`] numbered lines as JSON. The
//! last [`CONTEXT_LINES`] translated lines go with each batch as read-only context, so
//! names, gender and the form of address stay the same across batches. Timestamps never
//! go to the model; the translated text goes back onto the original cues by id.
//!
//! The answer must be a JSON array with the same ids. If it is not, the batch is sent
//! once more with a stricter prompt. If that also fails, each line is sent alone.

mod script;

#[cfg(feature = "local")]
pub mod local;
#[cfg(feature = "online")]
pub mod online;
#[cfg(feature = "online")]
pub mod server;

use std::{collections::HashMap, fmt::Write as _, path::Path};

use serde::{Deserialize, Serialize};

use verbacue_core::{
    Progress,
    error::{Context as _, Result, bail},
    subtitle::Cue,
};

pub use verbacue_core::languages::{language_name, languages};

pub const BATCH_SIZE: usize = 30;
pub const CONTEXT_LINES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Backend {
    /// Local GGUF model with embedded llama.cpp.
    Local,
    /// DeepL API (online). Needs DEEPL_API_KEY or deepl.key in the config folder.
    Deepl,
    /// Google Cloud Translation (online). Needs GOOGLE_TRANSLATE_API_KEY or
    /// google.key in the config folder.
    Google,
    /// Azure Translator (online). Needs AZURE_TRANSLATOR_KEY and AZURE_TRANSLATOR_REGION,
    /// or azure.key and azure.region in the config folder.
    Azure,
    /// A model on an OpenAI-compatible server (llama-server, Ollama, LM Studio, vLLM,
    /// Colibri). Set the address with --endpoint. An optional key comes from
    /// OPENAI_API_KEY or openai.key in the config folder.
    OpenaiCompat,
    /// No translation; write only the original-language subtitles.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Register {
    Informal,
    Formal,
}

pub struct Config {
    pub backend: Backend,
    pub target: String,
    pub register: Register,
    pub glossary: Vec<(String, String)>,
    pub llm_model: String,
    /// Only for [`Backend::OpenaiCompat`]: the base URL and the model name.
    pub endpoint: String,
    pub endpoint_model: Option<String>,
}

/// One subtitle line as the model sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub id: usize,
    pub text: String,
}

/// An earlier line and its translation, sent as read-only context.
#[derive(Debug, Clone, Serialize)]
pub struct ContextLine {
    pub source: String,
    pub translation: String,
}

pub struct Batch<'a> {
    pub source_lang: &'a str,
    pub lines: &'a [Line],
    pub context: &'a [ContextLine],
}

/// A translation backend. It returns one translated text per input line, in order.
pub trait Translator {
    /// # Errors
    /// Returns an error if the backend fails in a way that a retry cannot fix.
    fn translate(&mut self, batch: &Batch) -> Result<Vec<String>>;
}

/// One request to a chat model: translate the lines with these ids.
pub struct Prompt<'a> {
    /// The instructions.
    pub system: &'a str,
    /// The lines to translate, with context, as the model sees them.
    pub user: &'a str,
    /// A GBNF grammar for models with constrained decoding. It allows only a JSON
    /// array with exactly the ids of the batch.
    pub grammar: &'a str,
}

/// A chat model that answers one prompt. A model can use the grammar to force the
/// shape of the answer; [`LlmTranslator`] checks the answer in any case.
pub trait ChatModel {
    /// # Errors
    /// Returns an error if inference fails.
    fn complete(&mut self, prompt: &Prompt) -> Result<String>;
}

/// Reasons why a model answer is not a usable translation of the batch.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResponseError {
    #[error("the answer contains no JSON array")]
    NoJson,
    #[error("the answer is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("expected {expected} lines, got {got}")]
    CountMismatch { expected: usize, got: usize },
    #[error("the ids do not match; missing ids: {0:?}")]
    IdMismatch(Vec<usize>),
    #[error("line {0} is empty")]
    EmptyLine(usize),
    #[error("line {id} has words in the wrong language: {words}")]
    ForeignWords { id: usize, words: String },
}

/// Creates the translator for a config. For [`Backend::Local`] this loads the model,
/// and downloads it first if it is a known model that is not in the cache yet.
///
/// # Errors
/// Returns an error if the model cannot be found, downloaded or loaded, if an online
/// service has no key, or if this build has no support for the backend.
pub fn create(config: &Config) -> Result<Box<dyn Translator>> {
    match config.backend {
        Backend::Deepl | Backend::Google | Backend::Azure => {
            #[cfg(feature = "online")]
            {
                if !config.glossary.is_empty() {
                    tracing::warn!("the glossary is used only by the language models");
                }
                Ok(Box::new(online::OnlineTranslator::new(
                    config.backend,
                    &config.target,
                    config.register,
                )?))
            }
            #[cfg(not(feature = "online"))]
            bail!("this build has no online translators (feature `online`)")
        }
        Backend::OpenaiCompat => {
            #[cfg(feature = "online")]
            {
                let model = server::ServerModel::connect(
                    &config.endpoint,
                    config.endpoint_model.as_deref(),
                )?;
                Ok(Box::new(LlmTranslator::new(model, config)))
            }
            #[cfg(not(feature = "online"))]
            bail!("this build has no server translator (feature `online`)")
        }
        Backend::Local => {
            #[cfg(feature = "local")]
            {
                let path = verbacue_core::models::resolve(
                    &config.llm_model,
                    verbacue_core::models::Kind::Llm,
                    &(),
                )?;
                let model = local::LocalModel::load(&path)?;
                Ok(Box::new(LlmTranslator::new(model, config)))
            }
            #[cfg(not(feature = "local"))]
            bail!("this build has no local translator (feature `local`)")
        }
        Backend::None => bail!(InvalidInput; "the backend `none` does not translate"),
    }
}

/// Translates all cues and returns them with the same timing and new text.
///
/// # Errors
/// Returns an error if the backend fails.
pub fn translate_cues(
    translator: &mut dyn Translator,
    cues: &[Cue],
    source_lang: &str,
    progress: &dyn Progress,
) -> Result<Vec<Cue>> {
    let lines: Vec<Line> = cues
        .iter()
        .enumerate()
        .map(|(i, c)| Line {
            id: i + 1,
            text: one_line(&c.text),
        })
        .collect();
    let mut translated: Vec<String> = Vec::with_capacity(lines.len());
    for chunk in lines.chunks(BATCH_SIZE) {
        let start = translated.len().saturating_sub(CONTEXT_LINES);
        let context: Vec<ContextLine> = lines[start..translated.len()]
            .iter()
            .zip(&translated[start..])
            .map(|(l, t)| ContextLine {
                source: l.text.clone(),
                translation: t.clone(),
            })
            .collect();
        let batch = Batch {
            source_lang,
            lines: chunk,
            context: &context,
        };
        translated.extend(translator.translate(&batch)?);
        progress.update(translated.len() as u64, lines.len() as u64);
    }
    Ok(cues
        .iter()
        .zip(translated)
        .map(|(c, text)| Cue { text, ..c.clone() })
        .collect())
}

/// The batch, JSON, retry and fallback logic on top of any [`ChatModel`].
pub struct LlmTranslator<M> {
    model: M,
    system: String,
    target: String,
    /// The characters that the target language may use, for the response grammar.
    chars: Option<String>,
}

/// A failed answer. `texts` holds the translations if only the script check failed,
/// so the caller can still use them.
struct Invalid {
    reason: ResponseError,
    texts: Option<Vec<String>>,
}

impl<M: ChatModel> LlmTranslator<M> {
    pub fn new(model: M, config: &Config) -> Self {
        Self {
            model,
            system: system_prompt(config),
            target: config.target.clone(),
            chars: script::grammar_class(&config.target),
        }
    }

    /// Parses an answer, then checks that each line uses the target's script.
    fn check(&self, raw: &str, batch: &Batch) -> std::result::Result<Vec<String>, Invalid> {
        let ids: Vec<usize> = batch.lines.iter().map(|l| l.id).collect();
        let texts = parse_response(raw, &ids).map_err(|reason| Invalid {
            reason,
            texts: None,
        })?;
        for (line, text) in batch.lines.iter().zip(&texts) {
            let words = script::foreign_words(text, &line.text, &self.target);
            if !words.is_empty() {
                return Err(Invalid {
                    reason: ResponseError::ForeignWords {
                        id: line.id,
                        words: words.join(", "),
                    },
                    texts: Some(texts),
                });
            }
        }
        Ok(texts)
    }

    /// Asks the model, checks the answer, and asks once more with a stricter prompt if
    /// the answer is not valid.
    fn ask(&mut self, batch: &Batch) -> Result<std::result::Result<Vec<String>, Invalid>> {
        let ids: Vec<usize> = batch.lines.iter().map(|l| l.id).collect();
        let grammar = response_grammar(&ids, self.chars.as_deref());
        let user = user_prompt(batch, None);
        let raw = self.model.complete(&Prompt {
            system: &self.system,
            user: &user,
            grammar: &grammar,
        })?;
        let first = self.check(&raw, batch);
        let Err(invalid) = first else {
            return Ok(first);
        };
        tracing::debug!("invalid answer ({}), retrying: {raw}", invalid.reason);
        let strict = user_prompt(batch, Some(&invalid.reason));
        let raw = self.model.complete(&Prompt {
            system: &self.system,
            user: &strict,
            grammar: &grammar,
        })?;
        Ok(self.check(&raw, batch))
    }
}

impl<M: ChatModel> Translator for LlmTranslator<M> {
    fn translate(&mut self, batch: &Batch) -> Result<Vec<String>> {
        let answer = self.ask(batch)?;
        if let [line] = batch.lines {
            return Ok(vec![line_or_fallback(answer, line)]);
        }
        match answer {
            Ok(texts) => return Ok(texts),
            Err(invalid) => tracing::warn!(
                "batch at line {} failed ({}); translating line by line",
                batch.lines[0].id,
                invalid.reason
            ),
        }
        let mut out = Vec::with_capacity(batch.lines.len());
        for line in batch.lines {
            let single = Batch {
                lines: std::slice::from_ref(line),
                ..*batch
            };
            let answer = self.ask(&single)?;
            out.push(line_or_fallback(answer, line));
        }
        Ok(out)
    }
}

/// The translation of one line from an answer for that line alone.
fn line_or_fallback(answer: std::result::Result<Vec<String>, Invalid>, line: &Line) -> String {
    match answer {
        Ok(mut texts) => texts.remove(0),
        // Only the script check failed: a slightly wrong translation is still better
        // than the source text.
        Err(Invalid {
            reason,
            texts: Some(mut texts),
        }) => {
            tracing::warn!("line {}: {reason}", line.id);
            texts.remove(0)
        }
        Err(Invalid { reason, .. }) => {
            tracing::warn!("line {} kept untranslated: {reason}", line.id);
            line.text.clone()
        }
    }
}

/// Checks an answer against the expected ids and returns the texts in id order.
///
/// It accepts text around the array (for example a Markdown code fence) and any order
/// of the objects, but the set of ids must match exactly.
pub fn parse_response(raw: &str, ids: &[usize]) -> std::result::Result<Vec<String>, ResponseError> {
    let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) else {
        return Err(ResponseError::NoJson);
    };
    if end < start {
        return Err(ResponseError::NoJson);
    }
    let lines: Vec<Line> = serde_json::from_str(&raw[start..=end])
        .map_err(|e| ResponseError::InvalidJson(e.to_string()))?;
    if lines.len() != ids.len() {
        return Err(ResponseError::CountMismatch {
            expected: ids.len(),
            got: lines.len(),
        });
    }
    let by_id: HashMap<usize, String> = lines.into_iter().map(|l| (l.id, l.text)).collect();
    let missing: Vec<usize> = ids
        .iter()
        .copied()
        .filter(|id| !by_id.contains_key(id))
        .collect();
    if !missing.is_empty() {
        return Err(ResponseError::IdMismatch(missing));
    }
    ids.iter()
        .map(|id| {
            let text = one_line(&by_id[id]);
            if text.is_empty() {
                Err(ResponseError::EmptyLine(*id))
            } else {
                Ok(text)
            }
        })
        .collect()
}

/// A GBNF grammar that allows only a JSON array with exactly these ids, in this order.
/// With it, a local model cannot return a wrong count or a wrong id.
/// `chars` is a character class that limits the text, or `None` for any character.
pub fn response_grammar(ids: &[usize], chars: Option<&str>) -> String {
    let items: Vec<String> = ids
        .iter()
        .map(|id| format!(r#"ws "{{" ws "\"id\":" ws "{id}," ws "\"text\":" ws string ws "}}""#))
        .collect();
    format!(
        "root ::= \"[\" {} ws \"]\"\n\
         string ::= \"\\\"\" char+ \"\\\"\"\n\
         char ::= {} | \"\\\\\" [\"\\\\/nt]\n\
         ws ::= [ \\n]?\n",
        items.join(" \",\" "),
        chars.unwrap_or("[^\"\\\\\\x00-\\x1F]")
    )
}

fn system_prompt(config: &Config) -> String {
    let target = language_name(&config.target);
    let mut prompt = format!(
        "You are a professional subtitle translator. Translate film and TV subtitles into \
         {target}.\n\
         Rules:\n\
         - Write natural, spoken {target}, as people really talk.\n\
         - Keep lines short. A subtitle must be quick to read.\n\
         - Keep the meaning and tone. Do not translate word for word.\n\
         - Keep proper names as they are.\n\
         - Translate every line on its own id. Do not merge or split lines.\n\
         - Output only JSON, with no comments.\n"
    );
    prompt.push_str(&register_rule(&config.target, config.register));
    if !config.glossary.is_empty() {
        prompt.push_str("- Always use these translations:\n");
        for (term, translation) in &config.glossary {
            let _ = writeln!(prompt, "  {term} = {translation}");
        }
    }
    prompt
}

fn register_rule(target: &str, register: Register) -> String {
    match (target, register) {
        ("hu", Register::Informal) => {
            "- Use the informal form of address (tegeződés, \"te\") unless the scene clearly \
             needs the formal one.\n"
                .to_string()
        }
        ("hu", Register::Formal) => {
            "- Use the formal form of address (magázódás, \"ön\" / \"maga\").\n".to_string()
        }
        (_, Register::Informal) => "- Use the informal form of address.\n".to_string(),
        (_, Register::Formal) => "- Use the formal form of address.\n".to_string(),
    }
}

fn user_prompt(batch: &Batch, previous_error: Option<&ResponseError>) -> String {
    let mut prompt = String::new();
    let source = if batch.source_lang == "auto" {
        "the source language".to_string()
    } else {
        language_name(batch.source_lang)
    };
    let _ = writeln!(prompt, "Translate these subtitle lines from {source}.");
    if !batch.context.is_empty() {
        let context = serde_json::to_string(batch.context).unwrap_or_default();
        let _ = writeln!(
            prompt,
            "\nEarlier lines and their translations, for context only. Do not output them:\n\
             {context}"
        );
    }
    let lines = serde_json::to_string(batch.lines).unwrap_or_default();
    let _ = writeln!(prompt, "\nLines to translate:\n{lines}");
    let _ = writeln!(
        prompt,
        "\nAnswer with a JSON array of {} objects with the same ids: \
         [{{\"id\": <id>, \"text\": \"<translation>\"}}]",
        batch.lines.len()
    );
    if let Some(error) = previous_error {
        let _ = writeln!(
            prompt,
            "\nYour previous answer was not valid: {error}. Output ONLY the JSON array, with \
             exactly one object for each id above and nothing else."
        );
    }
    prompt
}

/// Reads a glossary file with one `term = translation` pair per line. Empty lines and
/// lines that start with `#` are ignored.
///
/// # Errors
/// Returns an error if the file cannot be read or a line has no `=`.
pub fn load_glossary(path: &Path) -> Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read glossary {}", path.display()))?;
    parse_glossary(&text).with_context(|| format!("bad glossary {}", path.display()))
}

fn parse_glossary(text: &str) -> Result<Vec<(String, String)>> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .map(|(n, l)| {
            let (term, translation) = l
                .split_once('=')
                .with_context(|| format!("line {n} has no '='"))?;
            Ok((term.trim().to_string(), translation.trim().to_string()))
        })
        .collect()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that returns prepared answers in order and records the prompts.
    struct ScriptedModel {
        answers: Vec<String>,
        prompts: Vec<String>,
    }

    impl ScriptedModel {
        fn new(answers: &[&str]) -> Self {
            Self {
                answers: answers.iter().rev().map(|a| a.to_string()).collect(),
                prompts: vec![],
            }
        }
    }

    impl ChatModel for ScriptedModel {
        fn complete(&mut self, prompt: &Prompt) -> Result<String> {
            self.prompts.push(prompt.user.to_string());
            Ok(self.answers.pop().expect("no more scripted answers"))
        }
    }

    fn config() -> Config {
        Config {
            backend: Backend::Local,
            target: "hu".into(),
            register: Register::Informal,
            glossary: vec![("Winterfell".into(), "Deres".into())],
            llm_model: String::new(),
            endpoint: String::new(),
            endpoint_model: None,
        }
    }

    fn lines(texts: &[&str], first_id: usize) -> Vec<Line> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| Line {
                id: first_id + i,
                text: t.to_string(),
            })
            .collect()
    }

    #[test]
    fn parse_accepts_fenced_and_reordered_answer() {
        let raw =
            "```json\n[{\"id\": 13, \"text\": \"Szia\"}, {\"id\": 12, \"text\": \"Helló\"}]\n```";
        assert_eq!(parse_response(raw, &[12, 13]).unwrap(), ["Helló", "Szia"]);
    }

    #[test]
    fn parse_rejects_bad_answers() {
        assert_eq!(parse_response("Sure!", &[1]), Err(ResponseError::NoJson));
        assert!(matches!(
            parse_response("[{\"id\": 1}]", &[1]),
            Err(ResponseError::InvalidJson(_))
        ));
        assert_eq!(
            parse_response(r#"[{"id":1,"text":"a"}]"#, &[1, 2]),
            Err(ResponseError::CountMismatch {
                expected: 2,
                got: 1
            })
        );
        assert_eq!(
            parse_response(r#"[{"id":1,"text":"a"},{"id":3,"text":"b"}]"#, &[1, 2]),
            Err(ResponseError::IdMismatch(vec![2]))
        );
        assert_eq!(
            parse_response(r#"[{"id":1,"text":"  "}]"#, &[1]),
            Err(ResponseError::EmptyLine(1))
        );
    }

    #[test]
    fn valid_first_answer_is_used() {
        let model = ScriptedModel::new(&[
            r#"[{"id":1,"text":"Szia, John."},{"id":2,"text":"Hol voltál?"}]"#,
        ]);
        let mut t = LlmTranslator::new(model, &config());
        let l = lines(&["Hello John.", "Where were you?"], 1);
        let out = t
            .translate(&Batch {
                source_lang: "en",
                lines: &l,
                context: &[],
            })
            .unwrap();
        assert_eq!(out, ["Szia, John.", "Hol voltál?"]);
        assert_eq!(t.model.prompts.len(), 1);
    }

    #[test]
    fn invalid_answer_is_retried_with_a_stricter_prompt() {
        let model = ScriptedModel::new(&[
            r#"[{"id":1,"text":"Szia"}]"#,
            r#"[{"id":1,"text":"Szia"},{"id":2,"text":"Hé"}]"#,
        ]);
        let mut t = LlmTranslator::new(model, &config());
        let l = lines(&["Hi", "Hey"], 1);
        let out = t
            .translate(&Batch {
                source_lang: "en",
                lines: &l,
                context: &[],
            })
            .unwrap();
        assert_eq!(out, ["Szia", "Hé"]);
        assert_eq!(t.model.prompts.len(), 2);
        assert!(t.model.prompts[1].contains("expected 2 lines, got 1"));
    }

    #[test]
    fn foreign_words_are_retried() {
        let model = ScriptedModel::new(&[
            r#"[{"id":1,"text":"Vártalak az állomáson, где ты"}]"#,
            r#"[{"id":1,"text":"Vártalak az állomáson."}]"#,
        ]);
        let mut t = LlmTranslator::new(model, &config());
        let l = lines(&["I waited for you at the station."], 1);
        let batch = Batch {
            source_lang: "en",
            lines: &l,
            context: &[],
        };
        assert_eq!(t.translate(&batch).unwrap(), ["Vártalak az állomáson."]);
        assert!(t.model.prompts[1].contains("wrong language: где, ты"));
    }

    #[test]
    fn foreign_words_are_kept_if_every_retry_has_them() {
        let bad = r#"[{"id":1,"text":"Vártalak, где"}]"#;
        let model = ScriptedModel::new(&[bad, bad]);
        let mut t = LlmTranslator::new(model, &config());
        let l = lines(&["I waited for you."], 1);
        let batch = Batch {
            source_lang: "en",
            lines: &l,
            context: &[],
        };
        assert_eq!(t.translate(&batch).unwrap(), ["Vártalak, где"]);
    }

    #[test]
    fn two_failures_fall_back_to_line_by_line() {
        let model = ScriptedModel::new(&[
            "no json",
            "still no json",
            r#"[{"id":7,"text":"Szia"}]"#,
            "bad",
            "bad again",
        ]);
        let mut t = LlmTranslator::new(model, &config());
        let l = lines(&["Hi", "Hey"], 7);
        let out = t
            .translate(&Batch {
                source_lang: "en",
                lines: &l,
                context: &[],
            })
            .unwrap();
        // Line 8 failed twice on its own too, so it keeps the source text.
        assert_eq!(out, ["Szia", "Hey"]);
        assert_eq!(t.model.prompts.len(), 5);
    }

    #[test]
    fn translate_cues_batches_and_sends_previous_lines_as_context() {
        struct Echo {
            batches: Vec<(Vec<usize>, Vec<String>)>,
        }
        impl Translator for Echo {
            fn translate(&mut self, batch: &Batch) -> Result<Vec<String>> {
                let ids = batch.lines.iter().map(|l| l.id).collect();
                let ctx = batch
                    .context
                    .iter()
                    .map(|c| c.translation.clone())
                    .collect();
                self.batches.push((ids, ctx));
                Ok(batch.lines.iter().map(|l| format!("T{}", l.id)).collect())
            }
        }
        let cues: Vec<Cue> = (0..65)
            .map(|i| Cue::new(i * 1000, i * 1000 + 900, format!("line\n{i}")))
            .collect();
        let mut echo = Echo { batches: vec![] };
        let out = translate_cues(&mut echo, &cues, "en", &()).unwrap();

        assert_eq!(echo.batches.len(), 3);
        assert_eq!(echo.batches[0].0.len(), 30);
        assert!(echo.batches[0].1.is_empty());
        assert_eq!(echo.batches[1].0[0], 31);
        assert_eq!(echo.batches[1].1, ["T26", "T27", "T28", "T29", "T30"]);
        assert_eq!(echo.batches[2].0.len(), 5);
        // Timing comes from the original cues.
        assert_eq!(out[64], Cue::new(64_000, 64_900, "T65"));
    }

    #[test]
    fn prompts_contain_register_glossary_and_context() {
        let system = system_prompt(&config());
        assert!(system.contains("Hungarian"));
        assert!(system.contains("tegeződés"));
        assert!(system.contains("Winterfell = Deres"));
        let l = lines(&["Hi"], 3);
        let ctx = [ContextLine {
            source: "Hello".into(),
            translation: "Helló".into(),
        }];
        let user = user_prompt(
            &Batch {
                source_lang: "en",
                lines: &l,
                context: &ctx,
            },
            None,
        );
        assert!(user.contains("from English"));
        assert!(user.contains(r#"{"source":"Hello","translation":"Helló"}"#));
        assert!(user.contains(r#"[{"id":3,"text":"Hi"}]"#));
    }

    #[test]
    fn grammar_lists_every_id_in_order() {
        let g = response_grammar(&[4, 5], None);
        let first = g.find("\"4,\"").unwrap();
        let second = g.find("\"5,\"").unwrap();
        assert!(first < second);
        assert!(g.starts_with("root ::= \"[\""));
        assert!(g.contains("char ::= [^"));
        let limited = response_grammar(&[1], Some("[a-z]"));
        assert!(limited.contains("char ::= [a-z] |"));
    }

    #[test]
    fn glossary_parsing() {
        let parsed = parse_glossary("# names\nWinterfell = Deres\n\n Ned = Ned \n").unwrap();
        assert_eq!(
            parsed,
            [
                ("Winterfell".into(), "Deres".into()),
                ("Ned".into(), "Ned".into())
            ]
        );
        assert!(parse_glossary("no separator").is_err());
    }
}
