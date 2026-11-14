//! The dub run for one video: decode, speak each cue, mix, and write the MKV.

use std::path::{Path, PathBuf};

use verbacue_core::{
    Error, ErrorKind, Progress, Result, check_cancel, languages::language_name, subtitle::Cue,
};
use verbacue_media::{
    audio::{self, RawAudio},
    video::{self, SubtitleTrack},
};

use crate::{
    fit,
    mix::{self, Mixer},
    xtts::{DEFAULT_STUDIO_VOICE, Latents, SAMPLE_RATE, Voice, Xtts},
};

/// The sample rate of the mix. HTDemucs works at 44.1 kHz.
const MIX_RATE: u32 = 44_100;
/// The expected speech time for one character, for the length check.
const MS_PER_CHAR: u64 = 70;

/// What happens to the original speech.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Original {
    /// Separation takes the original voices out.
    Remove,
    /// The original audio stays, lower while a new line plays. Faster.
    Lower,
}

/// The voice of the new lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceChoice {
    /// The voice of each speaker in the video.
    Clone,
    /// One studio voice for all lines.
    Studio(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub voice: VoiceChoice,
    pub original: Original,
    /// The highest speed-up without a warning, from 1.0 to 1.5.
    pub speed_limit: f32,
    /// `xtts-v2` or `xtts-v2-int8`.
    pub model: String,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            voice: VoiceChoice::Clone,
            original: Original::Remove,
            speed_limit: 1.3,
            model: default_model().to_string(),
        }
    }
}

/// The full model in CUDA builds, else the INT8 model. On a CPU, also on Apple
/// Silicon, the INT8 model is about twice as fast.
pub fn default_model() -> &'static str {
    if cfg!(feature = "cuda") {
        "xtts-v2"
    } else {
        "xtts-v2-int8"
    }
}

/// The steps of a dub run, for the progress display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DubStep {
    Separate,
    Speakers,
    Speak,
    Mix,
}

/// Receives the progress of a dub run.
pub trait Events: Sync {
    fn step(&self, step: DubStep);
    /// `done` of `total` units of the current step.
    fn progress(&self, done: u64, total: u64);
    /// A warning for the user, for example a line that is much faster than normal.
    fn note(&self, text: &str);
    fn cancelled(&self) -> bool;
}

impl Events for () {
    fn step(&self, _: DubStep) {}
    fn progress(&self, _: u64, _: u64) {}
    fn note(&self, _: &str) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// One video to dub.
pub struct DubJob<'a> {
    pub video: &'a Path,
    /// The audio track, as for `--audio-track`.
    pub track: usize,
    /// The translated cues.
    pub cues: &'a [Cue],
    /// The language of the cues.
    pub lang: &'a str,
    /// The folder with the files of `options.model`.
    pub models_dir: &'a Path,
    pub options: Options,
    /// The folder for the temporary audio files.
    pub temp_dir: &'a Path,
    pub output: &'a Path,
    pub subtitle_tracks: &'a [SubtitleTrack<'a>],
}

/// Dubs one video. The temporary files are deleted at the end, also on an error or a
/// cancel. On an error or a cancel, no file with the output name stays.
///
/// # Errors
/// Returns an error if a step fails, or [`ErrorKind::Cancelled`] if the user stops.
pub fn dub(job: DubJob<'_>, sink: &dyn Events) -> Result<()> {
    let progress = EventProgress(sink);
    let mut temp = TempFiles::default();
    std::fs::create_dir_all(job.temp_dir).map_err(|e| {
        Error::new(
            ErrorKind::Other,
            format!("cannot create {}: {e}", job.temp_dir.display()),
        )
    })?;
    let stem = job
        .output
        .file_stem()
        .map_or_else(|| "verbacue".into(), |s| s.to_string_lossy().into_owned());

    sink.step(DubStep::Separate);
    let original_path = temp.add(job.temp_dir.join(format!("{stem}.original.f32")));
    audio::decode_to_file(job.video, job.track, MIX_RATE, &original_path, &progress)?;
    let original = RawAudio {
        path: original_path,
        rate: MIX_RATE,
        channels: 2,
    };
    if job.options.original == Original::Remove {
        sink.note(
            "removing the original speech is not available yet; the original audio is lowered",
        );
    }

    // A folder without `gpt.onnx` holds only the INT8 model.
    let int8 = job.options.model.ends_with("int8") || !job.models_dir.join("gpt.onnx").is_file();
    let mut xtts = Xtts::load(job.models_dir, int8)?;
    let default_voice = xtts
        .studio_voice(DEFAULT_STUDIO_VOICE)
        .ok_or_else(|| Error::new(ErrorKind::Model, "the model has no default studio voice"))?;
    let voice = match &job.options.voice {
        VoiceChoice::Studio(name) => xtts.studio_voice(name).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("unknown studio voice {name:?}"),
            )
        })?,
        VoiceChoice::Clone => {
            sink.note("voice cloning is not available yet; all lines use the studio voice");
            default_voice.clone()
        }
    };

    sink.step(DubStep::Speak);
    let slots = fit::slots(job.cues);
    let mut mixer = Mixer::new(original.clone());
    for (n, slot) in slots.iter().enumerate() {
        check_cancel(&progress)?;
        sink.progress(n as u64, slots.len() as u64);
        let cue = &job.cues[slot.cue];
        let line = slot.cue + 1;
        let Some(latents) = speak_line(
            &mut xtts,
            cue,
            job.lang,
            &voice,
            &default_voice,
            n as u64,
            line,
            sink,
        )?
        else {
            continue;
        };
        let natural_ms = frames_ms(&latents.latents);
        let f = fit::fit(natural_ms, slot, job.options.speed_limit);
        if f.warn {
            sink.note(&format!(
                "line {line}: the speech is {:.2} times faster than normal",
                f.factor
            ));
        }
        let speech_voice = if latents.voice_is_default {
            &default_voice
        } else {
            &voice
        };
        let audio = xtts.vocode(&latents.latents.stretch(1.0 / f.factor), speech_voice)?;
        let reference = mix::read_mono(&original, slot.start_ms, slot.end_ms)?;
        let gain = mix::match_gain(&reference, &audio);
        let audio_ms = audio.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
        mixer.duck(slot.start_ms, slot.end_ms.max(slot.start_ms + audio_ms));
        mixer.add(
            slot.start_ms,
            &audio,
            gain,
            if f.warn { slot.next_start_ms } else { None },
        );
    }
    sink.progress(slots.len() as u64, slots.len() as u64);

    sink.step(DubStep::Mix);
    check_cancel(&progress)?;
    let mixed_path = temp.add(job.temp_dir.join(format!("{stem}.mix.f32")));
    let mixed = mixer.finish(&mixed_path)?;
    let title = format!("{} (AI voice)", language_name(job.lang));
    video::dub(
        job.video,
        job.output,
        &mixed,
        job.lang,
        &title,
        job.subtitle_tracks,
        &progress,
    )
}

/// The latents of a line, and whether they use the default voice.
struct Spoken {
    latents: Latents,
    voice_is_default: bool,
}

/// Speaks one line with the retry rule: a second seed, then the default studio voice.
/// Returns `None` for a line with nothing to speak, or a line that fails every try.
#[expect(clippy::too_many_arguments, reason = "the parts of one line")]
fn speak_line(
    xtts: &mut Xtts,
    cue: &Cue,
    lang: &str,
    voice: &Voice,
    default_voice: &Voice,
    seed: u64,
    line: usize,
    sink: &dyn Events,
) -> Result<Option<Spoken>> {
    let max_ms = 2 * MS_PER_CHAR * cue.text.chars().count() as u64;
    let tries = [
        (voice, seed, false),
        (voice, seed + 1, false),
        (default_voice, seed, true),
    ];
    for (v, s, is_default) in tries {
        match xtts.latents(&cue.text, lang, v, s) {
            Ok(l) if l.frames.is_empty() => return Ok(None),
            Ok(l) if frames_ms(&l) <= max_ms => {
                if is_default {
                    sink.note(&format!(
                        "line {line}: the voice failed; the studio voice speaks it"
                    ));
                }
                return Ok(Some(Spoken {
                    latents: l,
                    voice_is_default: is_default,
                }));
            }
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Other => {}
            Err(e) => return Err(e),
        }
    }
    sink.note(&format!(
        "line {line}: no usable speech; the line stays silent"
    ));
    Ok(None)
}

/// The speech time of latents: one frame is 1024 samples at 24 kHz.
fn frames_ms(latents: &Latents) -> u64 {
    latents.frames.len() as u64 * 1024 * 1000 / u64::from(SAMPLE_RATE)
}

/// Shows the events as a [`Progress`] for the media functions.
struct EventProgress<'a>(&'a dyn Events);

impl Progress for EventProgress<'_> {
    fn update(&self, done: u64, total: u64) {
        self.0.progress(done, total);
    }

    fn cancelled(&self) -> bool {
        self.0.cancelled()
    }
}

/// Deletes the temporary files when the run ends, in every case.
#[derive(Default)]
struct TempFiles(Vec<PathBuf>);

impl TempFiles {
    fn add(&mut self, path: PathBuf) -> PathBuf {
        self.0.push(path.clone());
        path
    }
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cancel;

    impl Events for Cancel {
        fn step(&self, _: DubStep) {}
        fn progress(&self, _: u64, _: u64) {}
        fn note(&self, _: &str) {}
        fn cancelled(&self) -> bool {
            true
        }
    }

    /// Makes a test video with the ffmpeg command, or returns `None` when it is missing.
    fn test_video(name: &str) -> Option<PathBuf> {
        let path = std::env::temp_dir().join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=size=160x120:rate=25:duration=4")
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(path).or_else(|| {
            eprintln!("skipped: the ffmpeg command is needed to make the test video");
            None
        })
    }

    fn job<'a>(
        video: &'a Path,
        cues: &'a [Cue],
        temp: &'a Path,
        output: &'a Path,
        models: &'a Path,
    ) -> DubJob<'a> {
        DubJob {
            video,
            track: 0,
            cues,
            lang: "hu",
            models_dir: models,
            options: Options {
                voice: VoiceChoice::Studio(DEFAULT_STUDIO_VOICE.into()),
                original: Original::Lower,
                speed_limit: 1.3,
                model: "xtts-v2-int8".into(),
            },
            temp_dir: temp,
            output,
            subtitle_tracks: &[],
        }
    }

    #[test]
    fn cancel_leaves_no_files() {
        let Some(video) = test_video("verbacue-run-cancel.mkv") else {
            return;
        };
        let temp = std::env::temp_dir().join("verbacue-run-cancel-tmp");
        let output = std::env::temp_dir().join("verbacue-run-cancel.hu.dub.mkv");
        let cues = [Cue::new(0, 1000, "Szia!")];
        let err = dub(
            job(&video, &cues, &temp, &output, Path::new("/nonexistent")),
            &Cancel,
        )
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
    }

    #[test]
    fn default_model_is_a_voice_bundle() {
        assert!(["xtts-v2", "xtts-v2-int8"].contains(&default_model()));
        assert_eq!(Options::default().original, Original::Remove);
    }

    /// Cancels at the second progress event of the speak step.
    #[derive(Default)]
    struct CancelInSpeak {
        speaking: std::sync::atomic::AtomicBool,
        events: std::sync::atomic::AtomicU32,
    }

    impl Events for CancelInSpeak {
        fn step(&self, step: DubStep) {
            use std::sync::atomic::Ordering;
            self.speaking
                .store(step == DubStep::Speak, Ordering::SeqCst);
        }
        fn progress(&self, _: u64, _: u64) {
            use std::sync::atomic::Ordering;
            if self.speaking.load(Ordering::SeqCst) {
                self.events.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn note(&self, _: &str) {}
        fn cancelled(&self) -> bool {
            self.events.load(std::sync::atomic::Ordering::SeqCst) >= 2
        }
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn cancel_during_speak_leaves_no_output() {
        let Some(video) = test_video("verbacue-run-speak-cancel.mkv") else {
            return;
        };
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out");
        let temp = std::env::temp_dir().join("verbacue-run-speak-cancel-tmp");
        let output = std::env::temp_dir().join("verbacue-run-speak-cancel.hu.dub.mkv");
        let cues = [
            Cue::new(0, 1500, "Jó napot!"),
            Cue::new(2000, 3500, "Hogy van?"),
            Cue::new(3600, 3900, "Jól."),
        ];
        let sink = CancelInSpeak::default();
        let err = dub(job(&video, &cues, &temp, &output, &models), &sink).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn dubs_a_video() {
        let Some(video) = test_video("verbacue-run-dub.mkv") else {
            return;
        };
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out");
        let temp = std::env::temp_dir().join("verbacue-run-dub-tmp");
        let output = std::env::temp_dir().join("verbacue-run-dub.hu.dub.mkv");
        let cues = [
            Cue::new(200, 1800, "Jó napot!"),
            Cue::new(2000, 3500, "[zene]"),
            Cue::new(2500, 3900, "Hogy van?"),
        ];
        dub(job(&video, &cues, &temp, &output, &models), &()).unwrap();
        assert!(output.is_file());
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
        let pcm = audio::decode_pcm(&output, 0, &()).unwrap();
        // The new track is 4 s long, as the input.
        assert!((63_000..=65_500).contains(&pcm.len()), "{}", pcm.len());
    }
}
