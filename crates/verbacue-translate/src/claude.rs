//! A chat model that runs the Claude Code CLI (`claude -p`).
//!
//! Claude reads a long context well, so one request can hold a whole subtitle file.
//! The prompt goes in on stdin, because a film is too long for the command line.
//! The CLI runs with no tools and without a saved session, so it answers once.

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::Duration,
};

use serde::Deserialize;

use super::{ChatModel, Prompt};
use verbacue_core::{
    Progress, check_cancel,
    error::{Error, ErrorKind, Result, bail},
};

/// Lines per request. A normal film has fewer, so it goes in one request.
pub const CLAUDE_BATCH_SIZE: usize = 800;

/// Names another program than `claude`, for tests and unusual installs.
const BIN_VAR: &str = "VERBACUE_CLAUDE_BIN";

const NOT_INSTALLED: &str =
    "the claude CLI is not installed; see https://docs.claude.com/claude-code";

pub struct ClaudeModel {
    bin: String,
    model: Option<String>,
}

impl ClaudeModel {
    /// Without `model`, the CLI uses its default model.
    pub fn new(model: Option<&str>) -> Self {
        let bin = std::env::var(BIN_VAR).unwrap_or_else(|_| "claude".to_string());
        Self::with_bin(bin, model)
    }

    pub fn with_bin(bin: impl Into<String>, model: Option<&str>) -> Self {
        Self {
            bin: bin.into(),
            model: model.map(str::to_string),
        }
    }

    fn command(&self, system: &str) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["-p", "--output-format", "json"])
            // An empty list turns off all tools, so the model cannot touch files.
            .args(["--tools", ""])
            .arg("--no-session-persistence")
            .args(["--system-prompt", system]);
        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }
        cmd
    }

    fn run(&self, prompt: &Prompt, progress: &dyn Progress) -> Result<String> {
        check_cancel(progress)?;
        let mut child = self
            .command(prompt.system)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::new(ErrorKind::Other, NOT_INSTALLED),
                _ => Error::new(
                    ErrorKind::Other,
                    format!("cannot start `{}`: {e}", self.bin),
                ),
            })?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        // Threads keep the three pipes moving, so a big prompt cannot block the CLI.
        let (status, out, err) = std::thread::scope(|scope| {
            scope.spawn(move || {
                if let Some(mut stdin) = stdin.take() {
                    // A write error means the CLI exited early; its exit code tells why.
                    let _ = stdin.write_all(prompt.user.as_bytes());
                }
            });
            let out = scope.spawn(move || read_all(&mut stdout));
            let err = scope.spawn(move || read_all(&mut stderr));
            let status = loop {
                if progress.cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Some(Ok(status)),
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(e) => break Some(Err(e)),
                }
            };
            (
                status,
                out.join().unwrap_or_default(),
                err.join().unwrap_or_default(),
            )
        });

        let Some(status) = status else {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        };
        let status =
            status.map_err(|e| Error::new(ErrorKind::Other, format!("claude failed: {e}")))?;
        if !status.success() {
            // The CLI can report an API error as a JSON envelope on stdout.
            let reason = match (err.trim(), parse_envelope(&out)) {
                ("", Err(_)) => String::new(),
                ("", Ok(text)) => text,
                (stderr, _) => stderr.to_string(),
            };
            bail!("claude exited with {status}: {reason}");
        }
        parse_envelope(&out)
    }
}

fn read_all(pipe: &mut Option<impl Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The answer text from the JSON that `--output-format json` prints.
fn parse_envelope(stdout: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        is_error: bool,
        result: Option<String>,
    }
    let envelope: Envelope = serde_json::from_str(stdout.trim())
        .map_err(|e| Error::new(ErrorKind::Other, format!("bad answer from claude: {e}")))?;
    match envelope.result {
        Some(text) if !envelope.is_error => Ok(text),
        Some(text) => bail!("claude reported an error: {text}"),
        None => bail!("the answer from claude has no result"),
    }
}

impl ChatModel for ClaudeModel {
    fn complete(&mut self, prompt: &Prompt) -> Result<String> {
        self.run(prompt, &())
    }

    fn complete_with(&mut self, prompt: &Prompt, progress: &dyn Progress) -> Result<String> {
        self.run(prompt, progress)
    }

    fn batch_size(&self) -> usize {
        CLAUDE_BATCH_SIZE
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use super::*;
    use crate::{Backend, Config, Hints, LlmTranslator, Register, translate_cues_with};
    use verbacue_core::subtitle::Cue;

    /// A fake `claude`. It saves its arguments and stdin, then prints the next canned
    /// reply file (`reply1`, `reply2`, ...), or fails if the file `exit` exists.
    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("verbacue-claude-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let script = dir.join("claude");
            fs::write(
                &script,
                "#!/bin/sh\n\
                 d=$(dirname \"$0\")\n\
                 n=$(cat \"$d/count\" 2>/dev/null || echo 0)\n\
                 n=$((n+1))\n\
                 echo $n > \"$d/count\"\n\
                 printf '%s\\n' \"$@\" > \"$d/args$n\"\n\
                 cat > \"$d/stdin$n\"\n\
                 if [ -f \"$d/exit\" ]; then echo 'login needed' >&2; exit 3; fi\n\
                 cat \"$d/reply$n\"\n",
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn bin(&self) -> String {
            self.dir.join("claude").display().to_string()
        }

        /// Queues the answer of the model, wrapped as the CLI wraps it.
        fn reply(&self, n: usize, answer: &str) {
            let envelope = serde_json::json!({
                "type": "result", "subtype": "success", "is_error": false, "result": answer
            });
            fs::write(self.dir.join(format!("reply{n}")), envelope.to_string()).unwrap();
        }

        fn requests(&self) -> usize {
            fs::read_to_string(self.dir.join("count"))
                .map(|c| c.trim().parse().unwrap())
                .unwrap_or(0)
        }

        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.dir.join(file)).unwrap()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn config() -> Config {
        Config {
            backend: Backend::Claude,
            target: "hu".into(),
            register: Register::Informal,
            glossary: vec![("Winterfell".into(), "Deres".into())],
            llm_model: String::new(),
            endpoint: String::new(),
            endpoint_model: None,
        }
    }

    fn cues(n: usize) -> Vec<Cue> {
        (0..n)
            .map(|i| {
                let start = i as u64 * 1000;
                Cue::new(start, start + 900, format!("line {}", i + 1))
            })
            .collect()
    }

    fn answer(ids: impl Iterator<Item = usize>) -> String {
        let items: Vec<_> = ids
            .map(|id| serde_json::json!({ "id": id, "text": format!("sor {id}") }))
            .collect();
        serde_json::to_string(&items).unwrap()
    }

    fn translator(fake: &Fake, model: Option<&str>) -> LlmTranslator<ClaudeModel> {
        LlmTranslator::new(ClaudeModel::with_bin(fake.bin(), model), &config())
    }

    #[test]
    fn a_whole_file_goes_in_one_request_with_the_video_context() {
        let fake = Fake::new("whole");
        fake.reply(1, &answer(1..=100));
        let hints = Hints {
            title: "movie.mkv".into(),
            notes: "Two sisters talk.".into(),
            ..Hints::default()
        };
        let mut t = translator(&fake, Some("sonnet"));
        let out = translate_cues_with(&mut t, &cues(100), "en", &hints, &()).unwrap();

        assert_eq!(fake.requests(), 1);
        assert_eq!(out[99].text, "sor 100");
        assert_eq!(out[99].start_ms, 99_000);
        let stdin = fake.read("stdin1");
        assert!(stdin.contains(r#"{"id":1,"text":"line 1"}"#));
        assert!(stdin.contains(r#"{"id":100,"text":"line 100"}"#));
        assert!(stdin.contains("movie.mkv") && stdin.contains("Two sisters talk."));
        let args = fake.read("args1");
        assert!(args.contains("--output-format\njson") && args.contains("--tools\n\n"));
        assert!(args.contains("--model\nsonnet"));
        assert!(args.contains("Winterfell = Deres") && args.contains("Hungarian"));
    }

    #[test]
    fn a_long_file_is_split_and_gets_earlier_lines_as_context() {
        let fake = Fake::new("long");
        fake.reply(1, &answer(1..=CLAUDE_BATCH_SIZE));
        fake.reply(2, &answer(CLAUDE_BATCH_SIZE + 1..=CLAUDE_BATCH_SIZE + 5));
        let mut t = translator(&fake, None);
        let all = cues(CLAUDE_BATCH_SIZE + 5);
        translate_cues_with(&mut t, &all, "en", &Hints::default(), &()).unwrap();
        assert_eq!(fake.requests(), 2);
        assert!(fake.read("stdin2").contains(r#""translation":"sor 800""#));
    }

    #[test]
    fn missing_ids_are_retried_with_a_stricter_prompt() {
        let fake = Fake::new("retry");
        fake.reply(1, &answer(1..=2));
        fake.reply(2, &answer(1..=3));
        let mut t = translator(&fake, None);
        let out = translate_cues_with(&mut t, &cues(3), "en", &Hints::default(), &()).unwrap();
        assert_eq!(out[2].text, "sor 3");
        assert_eq!(fake.requests(), 2);
        assert!(fake.read("stdin2").contains("expected 3 lines, got 2"));
    }

    #[test]
    fn a_batch_that_fails_twice_is_retried_in_smaller_batches() {
        let fake = Fake::new("smaller");
        let total = crate::BATCH_SIZE + 10;
        fake.reply(1, "[]");
        fake.reply(2, "[]");
        fake.reply(3, &answer(1..=crate::BATCH_SIZE));
        fake.reply(4, &answer(crate::BATCH_SIZE + 1..=total));
        let mut t = translator(&fake, None);
        let out = translate_cues_with(&mut t, &cues(total), "en", &Hints::default(), &()).unwrap();
        assert_eq!(out[total - 1].text, format!("sor {total}"));
        assert_eq!(fake.requests(), 4);
    }

    #[test]
    fn a_non_zero_exit_reports_stderr() {
        let fake = Fake::new("exit");
        fs::write(fake.dir.join("exit"), "").unwrap();
        let mut t = translator(&fake, None);
        let err = translate_cues_with(&mut t, &cues(1), "en", &Hints::default(), &()).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("login needed") && text.contains("exited"),
            "{text}"
        );
    }

    #[test]
    fn a_missing_binary_has_a_clear_error() {
        let model = ClaudeModel::with_bin("/nonexistent/claude", None);
        let mut t = LlmTranslator::new(model, &config());
        let err = translate_cues_with(&mut t, &cues(1), "en", &Hints::default(), &()).unwrap_err();
        assert!(err.to_string().contains("claude CLI is not installed"));
    }

    #[test]
    fn a_cancel_stops_before_the_request() {
        struct Stop;
        impl Progress for Stop {
            fn update(&self, _: u64, _: u64) {}
            fn cancelled(&self) -> bool {
                true
            }
        }
        let fake = Fake::new("cancel");
        let mut t = translator(&fake, None);
        let err =
            translate_cues_with(&mut t, &cues(1), "en", &Hints::default(), &Stop).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert_eq!(fake.requests(), 0);
    }

    /// Calls the real CLI. Run by hand:
    /// `cargo test -p verbacue-translate real_claude -- --ignored`.
    /// It needs `claude` on PATH and a login, and it costs money.
    #[test]
    #[ignore = "calls the real claude CLI"]
    fn real_claude_translates_three_lines() {
        let mut t = LlmTranslator::new(ClaudeModel::new(None), &config());
        let lines = ["Hello, John.", "Where were you?", "I waited for you."];
        let cues: Vec<Cue> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| Cue::new(i as u64 * 1000, i as u64 * 1000 + 900, *l))
            .collect();
        let out = translate_cues_with(&mut t, &cues, "en", &Hints::default(), &()).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().zip(lines).all(|(c, l)| c.text != l));
    }
}
