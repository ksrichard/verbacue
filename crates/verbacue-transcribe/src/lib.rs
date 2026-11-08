//! Speech to text with whisper.cpp (Metal on Apple Silicon).
//!
//! Whisper often invents text during silence or music. Two steps reduce that: the
//! Silero VAD model skips parts without speech, and [`filter_hallucinations`] removes
//! typical invented lines from the result.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use verbacue_core::{
    Progress,
    error::{Context as _, ErrorKind, Result, WithKind as _},
    subtitle::Cue,
};

pub struct Transcript {
    /// ISO 639-1 code, for example "en".
    pub language: String,
    pub cues: Vec<Cue>,
}

pub struct Options<'a> {
    pub model: &'a Path,
    pub vad_model: Option<&'a Path>,
    /// `None` means auto-detect.
    pub language: Option<&'a str>,
}

/// Transcribes 16 kHz mono samples. `progress` receives percent values (0-100).
///
/// The Whisper model stays loaded only while this function runs, so the memory is free
/// again before the translation model loads.
///
/// # Errors
/// Returns an error if the model cannot be loaded or inference fails.
pub fn transcribe(pcm: &[f32], opts: &Options, progress: &dyn Progress) -> Result<Transcript> {
    whisper_rs::install_logging_hooks();
    let ctx = WhisperContext::new_with_params(opts.model, WhisperContextParameters::default())
        .with_kind(ErrorKind::Model)
        .with_context(|| format!("cannot load Whisper model {}", opts.model.display()))?;
    let mut state = ctx
        .create_state()
        .with_kind(ErrorKind::Model)
        .context("cannot create Whisper state")?;

    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    params.set_n_threads(i32::try_from(threads).unwrap_or(4));
    params.set_language(Some(opts.language.unwrap_or("auto")));
    // Each 30 s window would otherwise get the text of the earlier windows as a prompt.
    // After one invented line in a quiet part, large-v3 then repeats it for the rest of
    // the file (on a 2.5 h film it lost 56 minutes). `no_context` does not stop this:
    // it clears the history only at the start of the run.
    params.set_n_max_text_ctx(0);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    // Suppresses non-speech tokens such as "[Music]" or "(applause)".
    params.set_suppress_nst(true);
    if let Some(vad) = opts.vad_model {
        params.set_vad_model_path(vad.to_str());
        params.enable_vad(true);
    }
    let percent = Arc::new(AtomicU64::new(0));
    let reported = Arc::clone(&percent);
    params.set_progress_callback_safe(move |p: i32| {
        reported.store(u64::try_from(p).unwrap_or(0), Ordering::Relaxed);
    });

    // Whisper's progress callback must be 'static, so inference runs on its own thread
    // and this thread passes the percentage on to `progress`.
    std::thread::scope(|scope| {
        let job = scope.spawn(|| state.full(params, pcm));
        while !job.is_finished() {
            progress.update(percent.load(Ordering::Relaxed), 100);
            std::thread::sleep(Duration::from_millis(200));
        }
        job.join().expect("the Whisper thread panicked")
    })
    .with_kind(ErrorKind::Model)
    .context("Whisper inference failed")?;
    progress.update(100, 100);

    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
        .unwrap_or("und")
        .to_string();
    let cues = state
        .as_iter()
        .map(|seg| {
            // Whisper timestamps are in centiseconds.
            let start = u64::try_from(seg.start_timestamp()).unwrap_or(0) * 10;
            let end = u64::try_from(seg.end_timestamp()).unwrap_or(0) * 10;
            let text = seg
                .to_str_lossy()
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            Cue::new(start, end, text)
        })
        .collect::<Vec<Cue>>();
    for cue in &cues {
        tracing::debug!("segment {}-{} ms: {:?}", cue.start_ms, cue.end_ms, cue.text);
    }

    Ok(Transcript {
        language,
        cues: filter_hallucinations(cues),
    })
}

/// Phrases that Whisper adds from its training data (mostly YouTube) when nobody speaks.
/// A cue is removed when its whole text, without punctuation and case, is one of these.
const HALLUCINATIONS: &[&str] = &[
    "thanks for watching",
    "thank you for watching",
    "thank you so much for watching",
    "thanks for watching and see you next time",
    "please subscribe",
    "please like and subscribe",
    "dont forget to like and subscribe",
    "subscribe to my channel",
    "see you in the next video",
    "subtitles by the amaraorg community",
    "transcribed by",
    "izlediğiniz için teşekkür ederim",
    "izlediğiniz için teşekkürler",
    "vielen dank fürs zuschauen",
    "gracias por ver el video",
    "gracias por ver",
    "merci davoir regardé",
    "grazie per la visione",
    "obrigado por assistir",
    "bedankt voor het kijken",
    "dziękuję za obejrzenie",
    "спасибо за просмотр",
    "ご視聴ありがとうございました",
    "시청해주셔서 감사합니다",
    "谢谢观看",
    "謝謝觀看",
];

/// A cue is also removed when its text contains one of these.
const HALLUCINATION_MARKERS: &[&str] = &[
    "amara.org",
    "www.mooji.org",
    "subtitles by",
    "untertitel im auftrag des zdf",
    "субтитры сделал",
];

/// Whisper reads at most 30 s at a time. A cue that fills most of that time with a few
/// words is music or noise that Whisper turned into text. Real speech has 12-15
/// characters per second.
const SLOW_CUE_MS: u64 = 15_000;
const MIN_CHARS_PER_SECOND: f64 = 1.5;

/// The longest run of identical consecutive cues that is kept. Real dialogue repeats
/// ("No." "No."), but a long run of the same line is a Whisper decoding loop.
const MAX_REPEATS: usize = 2;

/// Removes empty and punctuation-only cues, known invented phrases, and long runs of
/// the same text.
pub fn filter_hallucinations(cues: Vec<Cue>) -> Vec<Cue> {
    let mut out: Vec<Cue> = Vec::with_capacity(cues.len());
    let mut last_key = String::new();
    let mut run = 0;
    for cue in cues {
        let key = normalize(&cue.text);
        let lower = cue.text.to_lowercase();
        if key.is_empty()
            || HALLUCINATIONS.contains(&key.as_str())
            || HALLUCINATION_MARKERS.iter().any(|m| lower.contains(m))
            || is_too_slow(&cue, &key)
        {
            tracing::debug!("dropped hallucinated cue {:?}", cue.text);
            continue;
        }
        if key == last_key {
            run += 1;
            if run > MAX_REPEATS {
                tracing::debug!("dropped repeated cue {:?}", cue.text);
                continue;
            }
        } else {
            last_key = key;
            run = 1;
        }
        out.push(cue);
    }
    out
}

fn is_too_slow(cue: &Cue, key: &str) -> bool {
    let ms = cue.end_ms.saturating_sub(cue.start_ms);
    ms >= SLOW_CUE_MS && (key.chars().count() as f64) * 1000.0 / (ms as f64) < MIN_CHARS_PER_SECOND
}

/// Lowercase letters and digits only, with single spaces between words.
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cues(texts: &[&str]) -> Vec<Cue> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| Cue::new(i as u64 * 1000, i as u64 * 1000 + 900, *t))
            .collect()
    }

    fn texts(cues: &[Cue]) -> Vec<&str> {
        cues.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn removes_empty_and_punctuation_only() {
        let out = filter_hallucinations(cues(&["", "  ", "...", "-", "♪", "Hello."]));
        assert_eq!(texts(&out), ["Hello."]);
    }

    #[test]
    fn removes_known_phrases_but_keeps_real_dialogue() {
        let out = filter_hallucinations(cues(&[
            "Thanks for watching!",
            "Thank you.",
            "Subtitles by the Amara.org community",
            "I'm watching you.",
        ]));
        assert_eq!(texts(&out), ["Thank you.", "I'm watching you."]);
    }

    #[test]
    fn keeps_two_repeats_and_drops_the_rest_of_a_run() {
        let out = filter_hallucinations(cues(&["No.", "no!", "No.", "No.", "Yes.", "No."]));
        assert_eq!(texts(&out), ["No.", "no!", "Yes.", "No."]);
    }

    #[test]
    fn removes_phrases_in_other_languages() {
        let out = filter_hallucinations(cues(&[
            "İzlediğiniz için teşekkür ederim.",
            "Vielen Dank fürs Zuschauen!",
            "ご視聴ありがとうございました",
            "Teşekkür ederim.",
        ]));
        assert_eq!(texts(&out), ["Teşekkür ederim."]);
    }

    #[test]
    fn removes_a_few_words_that_fill_a_long_window() {
        let out = filter_hallucinations(vec![
            Cue::new(30_000, 59_980, "Bir şey söyle."),
            Cue::new(
                60_000,
                75_000,
                "This is a long line of real speech, said very slowly.",
            ),
            Cue::new(80_000, 86_000, "Every week."),
        ]);
        assert_eq!(
            texts(&out),
            [
                "This is a long line of real speech, said very slowly.",
                "Every week."
            ]
        );
    }

    #[test]
    fn keeps_non_latin_text() {
        let out = filter_hallucinations(cues(&["こんにちは", "Привет"]));
        assert_eq!(out.len(), 2);
    }
}
