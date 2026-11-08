//! Online translators: DeepL, Google Cloud Translation and Azure Translator.
//!
//! These send the subtitle text over the internet. Each service reads its key from an
//! environment variable, or else from a file in the verbacue config folder, so the key
//! never has to be on the command line. The folder is `~/.config/verbacue` on macOS and
//! Linux, and `%APPDATA%\verbacue` on Windows.

use std::{path::PathBuf, time::Duration};

use reqwest::{
    StatusCode,
    blocking::{Client, RequestBuilder},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Backend, Batch, Register, Translator};
use verbacue_core::error::{Context as _, Error, ErrorKind, Result, bail};

const ATTEMPTS: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    Deepl,
    Google,
    Azure,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::Deepl => "DeepL",
            Self::Google => "Google Cloud Translation",
            Self::Azure => "Azure Translator",
        }
    }

    /// The environment variable and the file name in the config folder for the key.
    fn key_source(self) -> (&'static str, &'static str) {
        match self {
            Self::Deepl => ("DEEPL_API_KEY", "deepl.key"),
            Self::Google => ("GOOGLE_TRANSLATE_API_KEY", "google.key"),
            Self::Azure => ("AZURE_TRANSLATOR_KEY", "azure.key"),
        }
    }
}

pub struct OnlineTranslator {
    service: Service,
    client: Client,
    key: String,
    /// Only for Azure: the region of the Translator resource, for example `westeurope`.
    region: Option<String>,
    target: String,
    formality: &'static str,
}

impl OnlineTranslator {
    /// # Errors
    /// Returns an error if the key (or the Azure region) is not set.
    pub fn new(backend: Backend, target: &str, register: Register) -> Result<Self> {
        let service = match backend {
            Backend::Deepl => Service::Deepl,
            Backend::Google => Service::Google,
            Backend::Azure => Service::Azure,
            Backend::Local | Backend::OpenaiCompat | Backend::None => {
                bail!("not an online translator")
            }
        };
        let (var, file) = service.key_source();
        let key = setting(var, file)?.ok_or_else(|| {
            Error::new(
                ErrorKind::MissingKey,
                format!(
                    "{} needs a key: set {var} or write it to {}",
                    service.name(),
                    shown(file)
                ),
            )
        })?;
        let region = if service == Service::Azure {
            Some(
                setting("AZURE_TRANSLATOR_REGION", "azure.region")?.ok_or_else(|| {
                    Error::new(
                        ErrorKind::MissingKey,
                        format!(
                            "Azure Translator needs the resource region: set \
                             AZURE_TRANSLATOR_REGION or write it to {}",
                            shown("azure.region")
                        ),
                    )
                })?,
            )
        } else {
            None
        };
        tracing::info!(
            "translating with {}; the subtitle text is sent over the internet",
            service.name()
        );
        Ok(Self {
            service,
            client: verbacue_core::http::client_builder()
                .timeout(Duration::from_secs(60))
                .build()?,
            key,
            region,
            target: target.to_string(),
            // DeepL uses the default for languages without formality.
            formality: match register {
                Register::Informal => "prefer_less",
                Register::Formal => "prefer_more",
            },
        })
    }

    fn request(&self, batch: &Batch) -> RequestBuilder {
        let texts: Vec<&str> = batch.lines.iter().map(|l| l.text.as_str()).collect();
        match self.service {
            Service::Deepl => {
                let host = if self.key.ends_with(":fx") {
                    "https://api-free.deepl.com"
                } else {
                    "https://api.deepl.com"
                };
                let mut body = json!({
                    "text": texts,
                    "source_lang": batch.source_lang.to_uppercase(),
                    "target_lang": deepl_target(&self.target),
                    "formality": self.formality,
                });
                // DeepL reads the context but does not translate or bill it.
                let context = deepl_context(batch);
                if !context.is_empty() {
                    body["context"] = json!(context);
                }
                self.client
                    .post(format!("{host}/v2/translate"))
                    .header("Authorization", format!("DeepL-Auth-Key {}", self.key))
                    .json(&body)
            }
            Service::Google => self
                .client
                .post("https://translation.googleapis.com/language/translate/v2")
                .header("X-goog-api-key", &self.key)
                .json(&json!({
                    "q": texts,
                    "source": batch.source_lang,
                    "target": self.target,
                    "format": "text",
                })),
            Service::Azure => {
                let body: Vec<Value> = texts.iter().map(|t| json!({ "Text": t })).collect();
                // The language codes are plain ASCII, so they need no escaping.
                let url = format!(
                    "https://api.cognitive.microsofttranslator.com/translate\
                     ?api-version=3.0&from={}&to={}",
                    batch.source_lang,
                    azure_code(&self.target)
                );
                self.client
                    .post(url)
                    .header("Ocp-Apim-Subscription-Key", &self.key)
                    .header(
                        "Ocp-Apim-Subscription-Region",
                        self.region.as_deref().unwrap_or_default(),
                    )
                    .json(&body)
            }
        }
    }

    fn send(&self, batch: &Batch<'_>) -> Result<Vec<String>> {
        let name = self.service.name();
        let mut wait = Duration::from_secs(2);
        for attempt in 1..=ATTEMPTS {
            let last = attempt == ATTEMPTS;
            let response = match self.request(batch).send() {
                Ok(r) => r,
                Err(e) if !last => {
                    tracing::warn!("{name} request failed ({e}); trying again");
                    std::thread::sleep(wait);
                    wait *= 2;
                    continue;
                }
                Err(e) => return Err(e).with_context(|| format!("{name} request failed")),
            };
            let status = response.status();
            if status.is_success() {
                let body = response.text()?;
                return parse(self.service, &body).with_context(|| format!("bad {name} response"));
            }
            if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && !last {
                tracing::warn!("{name} answered {status}; trying again");
                std::thread::sleep(wait);
                wait *= 2;
                continue;
            }
            let text = response.text().unwrap_or_default();
            match (self.service, status.as_u16()) {
                (_, 401 | 403) => bail!(KeyRefused; "{name} refused the key ({status}): {text}"),
                (Service::Deepl, 456) => {
                    bail!(QuotaExceeded; "the DeepL character quota is used up (456)")
                }
                _ => bail!("{name} answered {status}: {text}"),
            }
        }
        unreachable!("the loop returns on the last attempt")
    }
}

impl Translator for OnlineTranslator {
    fn translate(&mut self, batch: &Batch) -> Result<Vec<String>> {
        let texts = self.send(batch)?;
        if texts.len() != batch.lines.len() {
            bail!(
                "{} returned {} lines for {}",
                self.service.name(),
                texts.len(),
                batch.lines.len()
            );
        }
        Ok(texts)
    }
}

/// Reads a setting from the environment, or else from `<config folder>/<file>`.
pub(super) fn setting(var: &str, file: &str) -> Result<Option<String>> {
    if let Some(value) = std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
        return Ok(Some(value.trim().to_string()));
    }
    let Some(path) = config_file(file) else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text.trim().to_string()).filter(|v| !v.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn config_file(file: &str) -> Option<PathBuf> {
    verbacue_core::config_dir().map(|d| d.join(file))
}

/// The path of a config file, for error messages.
fn shown(file: &str) -> String {
    config_file(file).map_or_else(|| file.to_string(), |p| p.display().to_string())
}

fn parse(service: Service, body: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Text {
        text: String,
    }
    #[derive(Deserialize)]
    struct Deepl {
        translations: Vec<Text>,
    }
    #[derive(Deserialize)]
    struct GoogleText {
        #[serde(rename = "translatedText")]
        translated_text: String,
    }
    #[derive(Deserialize)]
    struct GoogleData {
        translations: Vec<GoogleText>,
    }
    #[derive(Deserialize)]
    struct Google {
        data: GoogleData,
    }
    #[derive(Deserialize)]
    struct Azure {
        translations: Vec<Text>,
    }

    Ok(match service {
        Service::Deepl => serde_json::from_str::<Deepl>(body)?
            .translations
            .into_iter()
            .map(|t| t.text)
            .collect(),
        Service::Google => serde_json::from_str::<Google>(body)?
            .data
            .translations
            .into_iter()
            .map(|t| t.translated_text)
            .collect(),
        Service::Azure => serde_json::from_str::<Vec<Azure>>(body)?
            .into_iter()
            .map(|a| a.translations.into_iter().next().map(|t| t.text))
            .collect::<Option<_>>()
            .context("an Azure item has no translation")?,
    })
}

/// DeepL needs a regional variant for some target languages.
/// The context text for DeepL: the viewer's notes, the film notes, the earlier lines
/// and the next lines.
fn deepl_context(batch: &Batch) -> String {
    let hints = batch.hints;
    let mut parts: Vec<&str> = Vec::new();
    for notes in [&hints.notes, &hints.film_notes] {
        if !notes.is_empty() {
            parts.push(notes);
        }
    }
    parts.extend(batch.context.iter().map(|c| c.source.as_str()));
    parts.extend(batch.upcoming.iter().map(|l| l.text.as_str()));
    parts.join("\n")
}

fn deepl_target(lang: &str) -> String {
    match lang {
        "en" => "EN-US".into(),
        "pt" => "PT-BR".into(),
        "zh" => "ZH-HANS".into(),
        other => other.to_uppercase(),
    }
}

fn azure_code(lang: &str) -> &str {
    match lang {
        "zh" => "zh-Hans",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContextLine, Hints, Line};

    #[test]
    fn deepl_context_has_notes_earlier_and_next_lines() {
        let hints = Hints {
            notes: "Two sisters talk.".into(),
            film_notes: "Ayşe is the older sister.".into(),
            names: vec![],
        };
        let earlier = [ContextLine {
            source: "Before".into(),
            translation: "Előtte".into(),
        }];
        let now = [Line {
            id: 2,
            text: "Now".into(),
        }];
        let next = [Line {
            id: 3,
            text: "After".into(),
        }];
        let batch = Batch {
            source_lang: "en",
            lines: &now,
            context: &earlier,
            upcoming: &next,
            hints: &hints,
        };
        assert_eq!(
            deepl_context(&batch),
            "Two sisters talk.\nAyşe is the older sister.\nBefore\nAfter"
        );
    }

    #[test]
    fn parses_each_service_answer() {
        let deepl = r#"{"translations":[{"detected_source_language":"TR","text":"Szia"}]}"#;
        assert_eq!(parse(Service::Deepl, deepl).unwrap(), ["Szia"]);
        let google = r#"{"data":{"translations":[{"translatedText":"Szia"}]}}"#;
        assert_eq!(parse(Service::Google, google).unwrap(), ["Szia"]);
        let azure = r#"[{"translations":[{"text":"Szia","to":"hu"}]}]"#;
        assert_eq!(parse(Service::Azure, azure).unwrap(), ["Szia"]);
        assert!(parse(Service::Azure, r#"[{"translations":[]}]"#).is_err());
    }

    #[test]
    fn language_codes() {
        assert_eq!(deepl_target("hu"), "HU");
        assert_eq!(deepl_target("en"), "EN-US");
        assert_eq!(azure_code("zh"), "zh-Hans");
        assert_eq!(azure_code("hu"), "hu");
    }
}
