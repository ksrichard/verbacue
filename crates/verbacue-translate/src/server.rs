//! A chat model behind an OpenAI-compatible HTTP server, for example llama-server,
//! Ollama, LM Studio, vLLM or Colibri.
//!
//! The server runs as a separate program, so it can host models that verbacue cannot
//! load itself. The batch, JSON and retry logic is the same as for the local model.
//!
//! The request has two differences from the local model:
//! - The instructions go into the user message. Some chat templates (for example
//!   TranslateGemma's) handle a system message badly, and the model repeats it.
//! - The answer shape comes from a JSON schema (`response_format`), which most
//!   servers turn into a grammar. If a server refuses the schema, verbacue asks again
//!   without it and relies on the answer check.

use std::time::Duration;

use reqwest::{
    StatusCode,
    blocking::{Client, RequestBuilder},
};
use serde::Deserialize;
use serde_json::json;

use super::{ChatModel, Prompt, online};
use verbacue_core::error::{Context as _, Result, bail};

const ATTEMPTS: u32 = 4;

pub struct ServerModel {
    client: Client,
    /// The base URL, for example `http://localhost:8080/v1`, without a trailing slash.
    endpoint: String,
    model: String,
    key: Option<String>,
    /// False after the server refused the JSON schema once.
    schema: bool,
}

impl ServerModel {
    /// Connects to the server. Without `model`, it uses the first model that the server
    /// lists.
    ///
    /// # Errors
    /// Returns an error if the server cannot be reached or lists no model.
    pub fn connect(endpoint: &str, model: Option<&str>) -> Result<Self> {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        if !is_local(&endpoint) {
            tracing::info!("the subtitle text is sent to {endpoint}");
        }
        let mut server = Self {
            // A big model on a slow machine can need many minutes for one batch, so
            // there is no limit on the answer time.
            client: verbacue_core::http::client_builder()
                .connect_timeout(Duration::from_secs(10))
                .build()?,
            endpoint,
            model: String::new(),
            key: online::setting("OPENAI_API_KEY", "openai.key")?,
            schema: true,
        };
        server.model = match model {
            Some(m) => m.to_string(),
            None => server.first_model()?,
        };
        tracing::info!(
            "translating with model {} on {}",
            server.model,
            server.endpoint
        );
        Ok(server)
    }

    fn with_key(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    fn first_model(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Model {
            id: String,
        }
        #[derive(Deserialize)]
        struct Models {
            data: Vec<Model>,
        }
        let url = format!("{}/models", self.endpoint);
        let response = self
            .with_key(self.client.get(&url))
            .send()
            .with_context(|| format!("cannot reach the server at {}", self.endpoint))?;
        let status = response.status();
        if !status.is_success() {
            bail!("{url} answered {status}; set the model with --endpoint-model");
        }
        let models: Models = response
            .json()
            .with_context(|| format!("bad answer from {url}"))?;
        models
            .data
            .into_iter()
            .next()
            .map(|m| m.id)
            .with_context(|| format!("the server at {} has no model loaded", self.endpoint))
    }

    fn chat(&mut self, prompt: &Prompt<'_>) -> Result<String> {
        let url = format!("{}/chat/completions", self.endpoint);
        let mut body = json!({
            "model": self.model,
            "messages": [
                { "role": "user", "content": format!("{}\n\n{}", prompt.system, prompt.user) },
            ],
            "temperature": 0,
            "stream": false,
        });
        // An empty grammar asks for free text, so no JSON schema.
        if self.schema && !prompt.grammar.is_empty() {
            body["response_format"] = response_format();
        }
        let mut wait = Duration::from_secs(2);
        for attempt in 1..=ATTEMPTS {
            let last = attempt == ATTEMPTS;
            let response = match self.with_key(self.client.post(&url)).json(&body).send() {
                Ok(r) => r,
                Err(e) if !last => {
                    tracing::warn!("request to {url} failed ({e}); trying again");
                    std::thread::sleep(wait);
                    wait *= 2;
                    continue;
                }
                Err(e) => return Err(e).with_context(|| format!("request to {url} failed")),
            };
            let status = response.status();
            if status.is_success() {
                let text = response.text()?;
                return parse(&text).with_context(|| format!("bad answer from {url}"));
            }
            if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && !last {
                tracing::warn!("{url} answered {status}; trying again");
                std::thread::sleep(wait);
                wait *= 2;
                continue;
            }
            let text = response.text().unwrap_or_default();
            if status == StatusCode::BAD_REQUEST && self.schema {
                tracing::warn!("the server refused the JSON schema; asking without it: {text}");
                self.schema = false;
                body.as_object_mut().map(|b| b.remove("response_format"));
                continue;
            }
            bail!("{url} answered {status}: {text}");
        }
        unreachable!("the loop returns on the last attempt")
    }
}

impl ChatModel for ServerModel {
    fn complete(&mut self, prompt: &Prompt) -> Result<String> {
        self.chat(prompt)
    }
}

/// A JSON schema for the answer: an object with an array of `{id, text}` lines.
/// OpenAI's strict mode needs an object at the top and does not allow item counts,
/// so the answer check enforces the ids and the count.
fn response_format() -> serde_json::Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "subtitles",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "lines": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "integer" },
                                "text": { "type": "string" },
                            },
                            "required": ["id", "text"],
                            "additionalProperties": false,
                        },
                    },
                },
                "required": ["lines"],
                "additionalProperties": false,
            },
        },
    })
}

/// The message text of a chat completion, without the reasoning of thinking models.
fn parse(body: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Message {
        content: Option<String>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
    }
    #[derive(Deserialize)]
    struct Completion {
        choices: Vec<Choice>,
    }
    let completion: Completion = serde_json::from_str(body)?;
    let content = completion
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .context("the answer has no message text")?;
    // Some servers put the reasoning into the text; it can contain brackets that look
    // like the JSON answer.
    Ok(match content.rfind("</think>") {
        Some(end) => content[end + "</think>".len()..].to_string(),
        None => content,
    })
}

/// True if the endpoint is on this computer.
fn is_local(endpoint: &str) -> bool {
    let rest = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| {
        rest.strip_prefix(host)
            .is_some_and(|r| r.is_empty() || r.starts_with([':', '/']))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_chat_completion() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"[{\"id\":1}]"}}]}"#;
        assert_eq!(parse(body).unwrap(), r#"[{"id":1}]"#);
        let thinking =
            r#"{"choices":[{"message":{"content":"<think>use [x]</think>\n[{\"id\":1}]"}}]}"#;
        assert_eq!(parse(thinking).unwrap(), "\n[{\"id\":1}]");
        assert!(parse(r#"{"choices":[]}"#).is_err());
    }

    #[test]
    fn knows_local_endpoints() {
        assert!(is_local("http://localhost:8080/v1"));
        assert!(is_local("http://127.0.0.1:11434/v1"));
        assert!(is_local("http://[::1]:8080/v1"));
        assert!(!is_local("https://api.openai.com/v1"));
        assert!(!is_local("http://192.168.1.5:8080/v1"));
    }
}
