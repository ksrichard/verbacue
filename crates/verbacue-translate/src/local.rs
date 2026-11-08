//! Local translation model: a GGUF instruct model on embedded llama.cpp (Metal on macOS).
//!
//! Each request gets a fresh context, so batches do not share state. The sampler is
//! greedy and follows a GBNF grammar, so the output is always the JSON shape we expect.

use std::{num::NonZeroU32, path::Path, sync::OnceLock};

use llama_cpp_2::{
    LogOptions,
    context::{LlamaContext, params::LlamaContextParams},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{LlamaChatMessage, LlamaModel, params::LlamaModelParams},
    sampling::LlamaSampler,
    token::{LlamaToken, data::LlamaTokenData, data_array::LlamaTokenDataArray},
};

use super::{ChatModel, Prompt};
use verbacue_core::{
    Progress, check_cancel,
    error::{Context as _, Error, ErrorKind, Result, WithKind as _, bail},
};

/// Enough for the system prompt, 30 lines, 5 context lines and the answer.
const CONTEXT_TOKENS: u32 = 8192;
const PROMPT_BATCH: usize = 2048;
const MAX_ANSWER_TOKENS: usize = 4096;

/// A GGUF instruct model on llama.cpp.
pub struct LocalModel {
    model: LlamaModel,
    backend: &'static LlamaBackend,
}

/// llama.cpp can start only once per process, so all models share one backend.
fn backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            llama_cpp_2::send_logs_to_tracing(LogOptions::default());
            LlamaBackend::init().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| Error::new(ErrorKind::Model, format!("cannot start llama.cpp: {e}")))
}

impl LocalModel {
    /// Loads a GGUF model with all layers on the GPU.
    ///
    /// # Errors
    /// Returns an error if llama.cpp cannot load the file.
    pub fn load(path: &Path) -> Result<Self> {
        let backend = backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(u32::MAX);
        tracing::info!("loading translation model {}", path.display());
        let model = LlamaModel::load_from_file(backend, path, &params)
            .with_kind(ErrorKind::Model)
            .with_context(|| format!("cannot load {}", path.display()))?;
        Ok(Self { model, backend })
    }
}

impl ChatModel for LocalModel {
    fn complete(&mut self, prompt: &Prompt) -> Result<String> {
        self.complete_with(prompt, &())
    }

    fn complete_with(&mut self, prompt: &Prompt, progress: &dyn Progress) -> Result<String> {
        let Prompt {
            system,
            user,
            grammar,
            ..
        } = *prompt;
        let template = self
            .model
            .chat_template(None)
            .with_kind(ErrorKind::Model)
            .context("the model has no chat template")?;
        let prompt = if template.to_str()?.contains("<|turn>") {
            gemma4_prompt(system, user)
        } else {
            let messages = [
                LlamaChatMessage::new("system".into(), system.into())
                    .with_kind(ErrorKind::Model)?,
                LlamaChatMessage::new("user".into(), user.into()).with_kind(ErrorKind::Model)?,
            ];
            self.model
                .apply_chat_template(&template, &messages, true)
                .with_kind(ErrorKind::Model)?
        };
        // Some built-in templates (for example Gemma) leave out the BOS token, and the
        // model gets much worse without it. Let the tokenizer add it, and remove the
        // second copy if the template had one already.
        let mut tokens = self.model.vocab().tokenize(prompt.as_bytes(), true, true);
        if tokens.len() > 1 && tokens[0] == tokens[1] {
            tokens.remove(0);
        }
        tracing::trace!("prompt:\n{prompt}");
        if tokens.len() + MAX_ANSWER_TOKENS > CONTEXT_TOKENS as usize {
            bail!("prompt is too long ({} tokens)", tokens.len());
        }

        // A GPU job can fail at random, for example when the system is short of memory.
        // A new context usually works, so one failure does not stop the whole run.
        let answer = match self.generate(&tokens, grammar, progress) {
            Ok(answer) => answer,
            Err(e) if e.kind() == ErrorKind::Cancelled => return Err(e),
            Err(e) => {
                tracing::warn!("translation model failed ({e:#}); trying again");
                self.generate(&tokens, grammar, progress)?
            }
        };
        let answer = String::from_utf8_lossy(&answer).into_owned();
        tracing::trace!("answer:\n{answer}");
        Ok(answer)
    }
}

impl LocalModel {
    /// Runs the prompt in a new context and samples the answer.
    fn generate(
        &self,
        tokens: &[LlamaToken],
        grammar: &str,
        progress: &dyn Progress,
    ) -> Result<Vec<u8>> {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS))
            .with_n_batch(PROMPT_BATCH as u32)
            // A full-size sliding-window cache needs 3.3 GB more GPU memory for Gemma 27B.
            // Each batch uses a new context, so the small cache loses nothing.
            .with_swa_full(false)
            .with_n_threads(i32::try_from(threads).unwrap_or(4));
        let mut ctx = self
            .model
            .new_context(self.backend, params)
            .with_kind(ErrorKind::Model)?;

        let started = std::time::Instant::now();
        let mut batch = LlamaBatch::new(PROMPT_BATCH, 1);
        let mut pos = 0i32;
        for chunk in tokens.chunks(PROMPT_BATCH) {
            batch.clear();
            for (i, token) in chunk.iter().enumerate() {
                let last = pos as usize + i + 1 == tokens.len();
                batch
                    .add(*token, pos + i as i32, &[0], last)
                    .with_kind(ErrorKind::Model)?;
            }
            ctx.decode(&mut batch)
                .with_kind(ErrorKind::Model)
                .context("prompt decoding failed")?;
            pos += chunk.len() as i32;
        }

        let prompt_time = started.elapsed();
        let mut grammar = LlamaSampler::grammar(&self.model, grammar, "root")
            .with_kind(ErrorKind::Model)
            .context("invalid response grammar")?;
        let vocab = self.model.vocab();
        let mut answer = Vec::new();
        for _ in 0..MAX_ANSWER_TOKENS {
            check_cancel(progress)?;
            let token = sample(&ctx, batch.n_tokens() - 1, &mut grammar);
            if vocab.is_eog(token) {
                break;
            }
            answer.extend(vocab.token_to_piece(token, false, None));
            batch.clear();
            batch
                .add(token, pos, &[0], true)
                .with_kind(ErrorKind::Model)?;
            pos += 1;
            ctx.decode(&mut batch)
                .with_kind(ErrorKind::Model)
                .context("decoding failed")?;
        }
        let generated = pos as usize - tokens.len();
        let answer_time = started.elapsed() - prompt_time;
        tracing::debug!(
            "prompt: {} tokens in {:.1?}, answer: {generated} tokens in {:.1?}",
            tokens.len(),
            prompt_time,
            answer_time
        );
        Ok(answer)
    }
}

/// Picks the most likely token that the grammar allows.
///
/// Gemma has a vocabulary of 262k tokens, and the grammar check over all of them takes
/// longer than the model step. The best token is almost always allowed, so it is checked
/// alone first. Only when the grammar rejects it are all tokens checked.
fn sample(ctx: &LlamaContext<'_>, idx: i32, grammar: &mut LlamaSampler) -> LlamaToken {
    let logits = ctx.get_logits_ith(idx);
    let (best, logit) = logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, l)| (LlamaToken(i32::try_from(i).unwrap_or(0)), *l))
        .unwrap_or((LlamaToken(0), 0.0));
    let mut one = LlamaTokenDataArray::new(vec![LlamaTokenData::new(best, logit, 0.0)], false);
    grammar.apply(&mut one);
    let token = if one.data[0].logit().is_finite() {
        best
    } else {
        let mut all = ctx.token_data_array_ith(idx);
        grammar.apply(&mut all);
        all.sample_token_greedy()
    };
    grammar.accept(token);
    token
}

/// The built-in llama.cpp templates do not know the Gemma 4 turn format, so it is
/// written here. The empty thought channel turns off thinking, so the answer starts
/// with the JSON at once.
fn gemma4_prompt(system: &str, user: &str) -> String {
    format!(
        "<|turn>system\n{}<turn|>\n<|turn>user\n{}<turn|>\n<|turn>model\n<|channel>thought\n<channel|>",
        system.trim(),
        user.trim()
    )
}
