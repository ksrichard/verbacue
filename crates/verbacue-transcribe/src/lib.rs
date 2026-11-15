//! Speech to text with whisper.cpp (Metal on Apple Silicon).
//!
//! Whisper often invents text during silence or music. Two steps reduce that: the
//! Silero VAD model skips parts without speech, and [`filter_hallucinations`] removes
//! typical invented lines from the result. Whisper also misses lines sometimes, so a
//! second pass transcribes the speech that no cue covers.

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters, WhisperState, WhisperVadContext, WhisperVadContextParams,
    WhisperVadParams,
};

use verbacue_core::{
    Progress, check_cancel,
    error::{Context as _, Error, ErrorKind, Result, WithKind as _},
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
/// With a VAD model, a second pass transcribes the speech that the first pass missed:
/// large-v3 sometimes jumps over a few lines or repeats one line instead of the next.
///
/// The Whisper model stays loaded only while this function runs, so the memory is free
/// again before the translation model loads.
///
/// # Errors
/// Returns an error if the model cannot be loaded or inference fails.
pub fn transcribe(pcm: &[f32], opts: &Options, progress: &dyn Progress) -> Result<Transcript> {
    check_cancel(progress)?;
    whisper_rs::install_logging_hooks();
    let mut context = WhisperContextParameters::default();
    let preset = dtw_preset(opts.model);
    if let Some(model_preset) = preset.clone() {
        context.dtw_parameters(DtwParameters {
            mode: DtwMode::ModelPreset { model_preset },
            ..DtwParameters::default()
        });
    }
    let ctx = WhisperContext::new_with_params(opts.model, context)
        .with_kind(ErrorKind::Model)
        .with_context(|| format!("cannot load Whisper model {}", opts.model.display()))?;
    let mut state = ctx
        .create_state()
        .with_kind(ErrorKind::Model)
        .context("cannot create Whisper state")?;
    if preset.is_none() {
        tracing::debug!("no word timing preset for {}", opts.model.display());
    }

    let main_share = if opts.vad_model.is_some() { 90 } else { 100 };
    let pass = Pass {
        language: opts.language.unwrap_or("auto"),
        vad_model: opts.vad_model,
        offset_ms: 0,
        percent: (0, main_share),
        live: true,
    };
    let mut cues = clean(run(&mut state, pcm, &pass, progress)?);
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
        .unwrap_or("und")
        .to_string();

    if let Some(vad) = opts.vad_model {
        let regions = missed(&speech(vad, pcm)?, &cues);
        tracing::info!(
            "second pass over {} parts with missed speech",
            regions.len()
        );
        let count = u64::try_from(regions.len()).unwrap_or(1).max(1);
        for (i, region) in (0_u64..).zip(&regions) {
            check_cancel(progress)?;
            let done = main_share + (100 - main_share) * i / count;
            progress.update(done, 100);
            let from = sample_index(region.start_ms).min(pcm.len());
            let to = sample_index(region.end_ms).min(pcm.len());
            let pass = Pass {
                language: &language,
                vad_model: None,
                offset_ms: region.start_ms,
                percent: (done, done),
                live: false,
            };
            let found = clean(run(&mut state, &pcm[from..to], &pass, progress)?);
            cues.extend(region.keep(found));
        }
        cues.sort_by_key(|c| c.start_ms);
        progress.update(100, 100);
    }

    Ok(Transcript { language, cues })
}

/// The settings of one Whisper run.
struct Pass<'a> {
    language: &'a str,
    vad_model: Option<&'a Path>,
    /// The time of the first sample in the film.
    offset_ms: u64,
    /// The progress range of this run, in percent.
    percent: (u64, u64),
    /// Shows each new segment with [`Progress::cue`].
    live: bool,
}

const SAMPLES_PER_MS: u64 = 16;

fn sample_index(ms: u64) -> usize {
    usize::try_from(ms * SAMPLES_PER_MS).unwrap_or(usize::MAX)
}

/// One Whisper segment as Whisper timed it, and the times of its words (DTW).
type Segment = (Cue, Vec<u64>);

/// Runs Whisper on `pcm`. The times in the result are film times.
fn run(
    state: &mut WhisperState,
    pcm: &[f32],
    pass: &Pass,
    progress: &dyn Progress,
) -> Result<Vec<Segment>> {
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    params.set_n_threads(i32::try_from(threads).unwrap_or(4));
    params.set_language(Some(pass.language));
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
    if let Some(vad) = pass.vad_model {
        params.set_vad_model_path(vad.to_str());
        params.enable_vad(true);
    }
    let percent = Arc::new(AtomicU64::new(pass.percent.0));
    let reported = Arc::clone(&percent);
    let (low, high) = pass.percent;
    params.set_progress_callback_safe(move |p: i32| {
        let p = u64::try_from(p).unwrap_or(0).min(100);
        reported.store(low + (high - low) * p / 100, Ordering::Relaxed);
    });

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    // whisper-rs 0.16 reads the closure through a pointer of type `F` but stores it as a
    // `Box<dyn FnMut>`. With `F` already that box type, both sides agree.
    let abort: Box<dyn FnMut() -> bool> = Box::new(move || stop_flag.load(Ordering::Relaxed));
    params.set_abort_callback_safe(abort);

    let new_cues = Arc::new(Mutex::new(Vec::new()));
    if pass.live {
        let segments = Arc::clone(&new_cues);
        params.set_segment_callback_safe_lossy(move |seg: whisper_rs::SegmentCallbackData| {
            // Whisper timestamps are in centiseconds.
            let start = u64::try_from(seg.start_timestamp).unwrap_or(0) * 10;
            let end = u64::try_from(seg.end_timestamp).unwrap_or(0) * 10;
            if let Ok(mut cues) = segments.lock() {
                cues.push(Cue::new(start, end, seg.text.trim()));
            }
        });
    }

    // Whisper's callbacks must be 'static, so inference runs on its own thread, and this
    // thread passes progress, new cues and the cancel flag between the two.
    let pass_on = || {
        let cues = new_cues.lock().map(|mut c| std::mem::take(&mut *c));
        for cue in cues.unwrap_or_default() {
            progress.cue(&cue);
        }
    };
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| state.full(params, pcm));
        while !job.is_finished() {
            progress.update(percent.load(Ordering::Relaxed), 100);
            pass_on();
            if progress.cancelled() {
                stop.store(true, Ordering::Relaxed);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        job.join().expect("the Whisper thread panicked")
    });
    pass_on();
    if stop.load(Ordering::Relaxed) {
        return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
    }
    result
        .with_kind(ErrorKind::Model)
        .context("Whisper inference failed")?;
    progress.update(high, 100);

    let offset = pass.offset_ms;
    let segments: Vec<Segment> = state
        .as_iter()
        .map(|seg| {
            // Whisper timestamps are in centiseconds.
            let start = u64::try_from(seg.start_timestamp()).unwrap_or(0) * 10;
            let end = u64::try_from(seg.end_timestamp()).unwrap_or(0) * 10;
            let text = seg
                .to_str_lossy()
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            let words = (0..seg.n_tokens())
                .filter_map(|i| seg.get_token(i))
                .filter(|token| token.to_str_lossy().is_ok_and(|t| is_word(&t)))
                .filter_map(|token| u64::try_from(token.token_data().t_dtw).ok())
                .map(|t| t * 10 + offset)
                .collect();
            (Cue::new(start + offset, end + offset, text), words)
        })
        .collect();
    for (cue, words) in &segments {
        tracing::debug!(
            "segment {}-{} ms, words at {words:?} ms: {:?}",
            cue.start_ms,
            cue.end_ms,
            cue.text
        );
    }
    Ok(segments)
}

/// Gives each segment the times of its words, then removes invented lines. The filter
/// judges the word times, because a segment often includes seconds of silence.
fn clean(segments: Vec<Segment>) -> Vec<Cue> {
    let tight = segments
        .into_iter()
        .map(|(segment, words)| {
            let (start, end) = tighten(segment.start_ms, segment.end_ms, &words);
            Cue::new(start, end, segment.text)
        })
        .collect();
    filter_hallucinations(tight)
}

/// The speech parts of `pcm` that the Silero VAD model finds, as (start, end) in ms.
fn speech(vad_model: &Path, pcm: &[f32]) -> Result<Vec<(u64, u64)>> {
    let path = vad_model
        .to_str()
        .context("the VAD model path is not UTF-8")?;
    let mut vad = WhisperVadContext::new(path, WhisperVadContextParams::new())
        .with_kind(ErrorKind::Model)
        .context("cannot load the VAD model")?;
    let segments = vad
        .segments_from_samples(WhisperVadParams::new(), pcm)
        .with_kind(ErrorKind::Model)
        .context("speech detection failed")?;
    // VAD times are in centiseconds.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "film times in ms fit in u64"
    )]
    Ok(segments
        .map(|s| ((s.start * 10.0) as u64, (s.end * 10.0) as u64))
        .collect())
}

/// Speech shorter than this outside every cue is noise or a breath.
const MIN_MISSED_MS: u64 = 1_000;
/// Speech this close to a cue belongs to that cue.
const CUE_MARGIN_MS: u64 = 300;
/// Extra audio around a missed part, so that Whisper hears the first and last word.
const REGION_PAD_MS: u64 = 300;

/// A part of the film between two cues with speech that no cue covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Region {
    start_ms: u64,
    end_ms: u64,
    /// The end of the cue before and the start of the cue after.
    gap: (u64, u64),
}

impl Region {
    /// The new cues of this region. A cue keeps its place only if its middle is in the
    /// region, and it never overlaps the cues around the gap.
    fn keep(&self, found: Vec<Cue>) -> Vec<Cue> {
        found
            .into_iter()
            .filter(|c| (self.start_ms..self.end_ms).contains(&(c.start_ms / 2 + c.end_ms / 2)))
            .map(|c| {
                let start = c.start_ms.max(self.gap.0);
                let end = c.end_ms.min(self.gap.1).max(start);
                Cue::new(start, end, c.text)
            })
            .collect()
    }
}

/// The parts with speech (`speech`, sorted) that no cue covers. Missed speech in the
/// same gap between two cues forms one region.
fn missed(speech: &[(u64, u64)], cues: &[Cue]) -> Vec<Region> {
    let mut regions: Vec<Region> = Vec::new();
    for &(start, end) in speech {
        // The cues that touch this speech part, with the margin.
        let mut cursor = start;
        let mut parts = Vec::new();
        for cue in cues {
            let (c0, c1) = (
                cue.start_ms.saturating_sub(CUE_MARGIN_MS),
                cue.end_ms + CUE_MARGIN_MS,
            );
            if c1 <= cursor || c0 >= end {
                continue;
            }
            if c0 > cursor {
                parts.push((cursor, c0));
            }
            cursor = cursor.max(c1);
        }
        if cursor < end {
            parts.push((cursor, end));
        }
        for (p0, p1) in parts {
            if p1 - p0 < MIN_MISSED_MS {
                continue;
            }
            let gap = gap_around(p0, p1, cues);
            match regions.last_mut() {
                Some(last) if last.gap == gap => last.end_ms = p1,
                _ => regions.push(Region {
                    start_ms: p0,
                    end_ms: p1,
                    gap,
                }),
            }
        }
    }
    for region in &mut regions {
        region.start_ms = region
            .start_ms
            .saturating_sub(REGION_PAD_MS)
            .max(region.gap.0);
        region.end_ms = (region.end_ms + REGION_PAD_MS).min(region.gap.1);
    }
    regions
}

/// The end of the last cue before `start` and the start of the first cue after `end`.
fn gap_around(start: u64, end: u64, cues: &[Cue]) -> (u64, u64) {
    let before = cues
        .iter()
        .filter(|c| c.end_ms <= start + CUE_MARGIN_MS)
        .map(|c| c.end_ms)
        .max()
        .unwrap_or(0);
    let after = cues
        .iter()
        .filter(|c| c.start_ms + CUE_MARGIN_MS >= end)
        .map(|c| c.start_ms)
        .min()
        .unwrap_or(u64::MAX);
    (before, after)
}

/// True for a token with a letter or digit. Punctuation tokens often get the time of
/// the next segment, and special tokens such as "[_TT_516]" have no word time.
fn is_word(token: &str) -> bool {
    !token.trim_start().starts_with("[_") && token.chars().any(char::is_alphanumeric)
}

/// A subtitle shows this long before its first word …
const LEAD_MS: u64 = 100;
/// … and stays this long after its last word.
const TAIL_MS: u64 = 500;

/// Whisper's segment times often cover the silence before or after the speech, so a
/// subtitle would show seconds early or late. The word times (DTW) mark the speech;
/// the result stays inside the segment. Word times outside the segment are wrong
/// (Whisper sometimes gives a value far past the end of the film), so they do not count.
/// Without word times the segment stays as it is.
fn tighten(start: u64, end: u64, words: &[u64]) -> (u64, u64) {
    let inside = || words.iter().filter(|&&w| (start..=end).contains(&w));
    let (Some(first), Some(last)) = (inside().min(), inside().max()) else {
        return (start, end);
    };
    (
        first.saturating_sub(LEAD_MS).max(start),
        (last + TAIL_MS).min(end),
    )
}

/// The word timing preset for a standard Whisper model, from its file name
/// (`ggml-large-v3.bin`, also quantized ones such as `ggml-small.en-q8_0.bin`).
fn dtw_preset(model: &Path) -> Option<DtwModelPreset> {
    let stem = model.file_name()?.to_str()?.to_lowercase();
    let name = stem.strip_prefix("ggml-").unwrap_or(&stem);
    // Longer names first, so "large-v3-turbo" does not match "large-v3".
    let presets = [
        ("large-v3-turbo", DtwModelPreset::LargeV3Turbo),
        ("large-v3", DtwModelPreset::LargeV3),
        ("large-v2", DtwModelPreset::LargeV2),
        ("large-v1", DtwModelPreset::LargeV1),
        ("medium.en", DtwModelPreset::MediumEn),
        ("medium", DtwModelPreset::Medium),
        ("small.en", DtwModelPreset::SmallEn),
        ("small", DtwModelPreset::Small),
        ("base.en", DtwModelPreset::BaseEn),
        ("base", DtwModelPreset::Base),
        ("tiny.en", DtwModelPreset::TinyEn),
        ("tiny", DtwModelPreset::Tiny),
    ];
    presets
        .into_iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, preset)| preset)
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
    "altyazı mk",
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

    #[test]
    fn dtw_preset_comes_from_the_model_file_name() {
        use whisper_rs::DtwModelPreset as P;
        let preset = |name: &str| dtw_preset(Path::new(name));
        assert!(matches!(preset("/c/ggml-large-v3.bin"), Some(P::LargeV3)));
        assert!(matches!(
            preset("ggml-large-v3-turbo.bin"),
            Some(P::LargeV3Turbo)
        ));
        assert!(matches!(
            preset("ggml-large-v3-turbo-q5_0.bin"),
            Some(P::LargeV3Turbo)
        ));
        assert!(matches!(preset("ggml-tiny.bin"), Some(P::Tiny)));
        assert!(matches!(preset("ggml-tiny.en.bin"), Some(P::TinyEn)));
        assert!(matches!(preset("ggml-medium.bin"), Some(P::Medium)));
        assert!(matches!(preset("ggml-small.en-q8_0.bin"), Some(P::SmallEn)));
        assert!(preset("my-finetune.bin").is_none());
    }

    #[test]
    fn only_tokens_with_letters_or_digits_are_words() {
        assert!(is_word(" show"));
        assert!(is_word("évé"));
        assert!(is_word("42"));
        assert!(!is_word("."));
        assert!(!is_word(" ,"));
        assert!(!is_word("[_TT_516]"));
    }

    #[test]
    fn word_times_outside_the_segment_do_not_count() {
        // Whisper gave one word a time far past the end of the film.
        assert_eq!(
            tighten(1_000, 5_000, &[2_000, 440_000_000_000]),
            (1_900, 2_500)
        );
        assert_eq!(tighten(1_000, 5_000, &[440_000_000_000]), (1_000, 5_000));
    }

    #[test]
    fn a_short_line_in_a_long_segment_stays() {
        let long_silence = (Cue::new(0, 16_000, "Hadi Turgut."), vec![7_000, 7_400]);
        let music = (Cue::new(20_000, 38_000, "Dicle"), vec![]);
        assert_eq!(texts(&clean(vec![long_silence, music])), ["Hadi Turgut."]);
    }

    #[test]
    fn speech_without_a_cue_is_missed() {
        let cues = [Cue::new(1_000, 3_000, "a"), Cue::new(20_000, 22_000, "b")];
        // Covered, too short, then two missed parts in the same gap.
        let speech = [
            (1_000, 3_100),
            (5_000, 5_500),
            (8_000, 10_000),
            (12_000, 15_000),
        ];
        assert_eq!(
            missed(&speech, &cues),
            [Region {
                start_ms: 7_700,
                end_ms: 15_300,
                gap: (3_000, 20_000)
            }]
        );
    }

    #[test]
    fn missed_speech_in_two_gaps_gives_two_regions() {
        let cues = [Cue::new(10_000, 12_000, "a")];
        let speech = [(2_000, 4_000), (11_000, 16_000)];
        assert_eq!(
            missed(&speech, &cues),
            [
                Region {
                    start_ms: 1_700,
                    end_ms: 4_300,
                    gap: (0, 10_000)
                },
                Region {
                    start_ms: 12_000,
                    end_ms: 16_300,
                    gap: (12_000, u64::MAX)
                },
            ]
        );
    }

    #[test]
    fn a_region_keeps_only_its_own_lines_inside_the_gap() {
        let region = Region {
            start_ms: 5_000,
            end_ms: 9_000,
            gap: (4_000, 10_000),
        };
        let found = vec![
            Cue::new(3_800, 6_400, "starts before the gap"),
            Cue::new(6_000, 7_000, "inside"),
            Cue::new(9_500, 11_000, "after the region"),
        ];
        let kept = region.keep(found);
        assert_eq!(texts(&kept), ["starts before the gap", "inside"]);
        assert_eq!((kept[0].start_ms, kept[0].end_ms), (4_000, 6_400));
    }

    #[test]
    fn word_times_tighten_a_segment() {
        // Whisper gave 0-10.32 s; the words were spoken from 2.14 to 4.40 s.
        let words = [2140, 2440, 2660, 2980, 3260, 3560, 3640, 3920, 4400];
        assert_eq!(tighten(0, 10_320, &words), (2040, 4900));
    }

    #[test]
    fn tight_times_stay_inside_the_segment() {
        assert_eq!(tighten(2100, 4500, &[2140, 4400]), (2100, 4500));
    }

    #[test]
    fn a_segment_without_word_times_keeps_its_times() {
        assert_eq!(tighten(1000, 3000, &[]), (1000, 3000));
    }

    struct Stopped;

    impl Progress for Stopped {
        fn update(&self, _done: u64, _total: u64) {}

        fn cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn transcribe_checks_cancel_before_loading_the_model() {
        let opts = Options {
            model: Path::new("missing-model.bin"),
            vad_model: None,
            language: None,
        };
        let Err(err) = transcribe(&[0.0; 16_000], &opts, &Stopped) else {
            panic!("transcribe did not stop");
        };
        assert_eq!(err.kind(), ErrorKind::Cancelled);
    }

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
            "Altyazı M.K.",
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
