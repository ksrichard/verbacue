//! The whole job for one video: decode, transcribe, translate, and write subtitle files
//! and videos. The `verbacue` command and the desktop app both run it.

use std::{
    fs,
    path::{Path, PathBuf},
};

use verbacue_core::{
    Progress, Result, check_cancel,
    error::{Context as _, bail},
    models,
    subtitle::{self, Cue, Format},
};

use crate::{audio, transcribe, translate};

/// One video and what to make from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub video: PathBuf,
    /// The position among the audio streams.
    pub audio_track: usize,
    /// A known Whisper model name, or a model file.
    pub whisper_model: String,
    /// Skip silence and music with the Silero VAD model.
    pub vad: bool,
    /// The spoken language, or `None` to detect it.
    pub language: Option<String>,
    /// The target language code. Output files use it in their names, also without a
    /// translation.
    pub target: String,
    /// `None` writes only the original-language subtitles.
    pub translation: Option<Translation>,
    pub format: Format,
    /// The file for the target-language subtitles (the CLI's `-o`).
    pub output: Option<PathBuf>,
    /// The folder for all output files. `None` writes them next to the video.
    pub out_dir: Option<PathBuf>,
    /// Also write an MKV with soft subtitle tracks.
    pub embed: bool,
    /// Also write an MP4 with the subtitles burned into the picture.
    pub burn: bool,
    /// Run again even if the output exists.
    pub force: bool,
}

impl Job {
    /// A job with the defaults of the `verbacue` command and no translation.
    pub fn new(video: impl Into<PathBuf>, target: impl Into<String>) -> Self {
        Self {
            video: video.into(),
            audio_track: 0,
            whisper_model: models::DEFAULT_WHISPER.to_string(),
            vad: true,
            language: None,
            target: target.into(),
            translation: None,
            format: Format::Srt,
            output: None,
            out_dir: None,
            embed: false,
            burn: false,
            force: false,
        }
    }
}

/// How to translate.
#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    pub backend: translate::Backend,
    pub register: translate::Register,
    pub glossary: Option<PathBuf>,
    /// A known model name or a GGUF file for the local backend; the model name for
    /// an OpenAI-compatible server.
    pub llm_model: String,
    pub endpoint: String,
    pub endpoint_model: Option<String>,
}

impl Default for Translation {
    fn default() -> Self {
        Self {
            backend: translate::Backend::Local,
            register: translate::Register::Informal,
            glossary: None,
            llm_model: models::DEFAULT_LLM.to_string(),
            endpoint: "http://localhost:8080/v1".to_string(),
            endpoint_model: None,
        }
    }
}

/// A part of the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A model download, with the model name.
    Download(&'static str),
    Decode,
    Transcribe,
    Translate,
    Embed,
    Burn,
    /// Writing the subtitle files of a job without video output.
    Save,
}

/// What happens during a job.
#[derive(Debug)]
pub enum Event<'a> {
    /// A new step starts. Progress events after it belong to it.
    Step(Step),
    /// `done` of `total` units of the current step are finished.
    Progress {
        done: u64,
        total: u64,
    },
    /// A new transcribed cue, before layout.
    Cue(&'a Cue),
    Wrote(&'a Path),
    /// The job is already done; this file exists.
    Skipped(&'a Path),
    /// Something to tell the user.
    Note(&'a str),
}

/// Receives the events of a job, and can stop it.
pub trait Sink: Sync {
    fn event(&self, event: Event<'_>);

    /// True when the job must stop.
    fn cancelled(&self) -> bool {
        false
    }
}

/// The result of a job.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The detected (or given) spoken language. Empty for a skipped job.
    pub source_language: String,
    pub files: Vec<PathBuf>,
    pub skipped: bool,
}

/// Runs jobs. It keeps the translator loaded between jobs with the same settings.
#[derive(Default)]
pub struct Pipeline {
    translator: Option<(String, Translation, Box<dyn translate::Translator>)>,
}

/// Turns a [`Sink`] into a [`Progress`].
struct Report<'a>(&'a dyn Sink);

impl Progress for Report<'_> {
    fn update(&self, done: u64, total: u64) {
        self.0.event(Event::Progress { done, total });
    }

    fn cancelled(&self) -> bool {
        self.0.cancelled()
    }

    fn cue(&self, cue: &Cue) {
        self.0.event(Event::Cue(cue));
    }
}

/// The path of an output file of `job`, moved into `out_dir` if one is set.
fn place(job: &Job, path: PathBuf) -> PathBuf {
    match (&job.out_dir, path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path,
    }
}

/// The subtitle file for language `lang`: `movie.<lang>.srt` next to the video (or in
/// `out_dir`). `job.output` replaces the path for the target language.
pub fn output_path(job: &Job, lang: &str) -> PathBuf {
    if lang == job.target
        && let Some(out) = &job.output
    {
        return out.clone();
    }
    place(
        job,
        job.video
            .with_extension(format!("{lang}.{}", job.format.extension())),
    )
}

fn video_output(job: &Job, extension: &str) -> PathBuf {
    place(
        job,
        job.video
            .with_extension(format!("{}.{extension}", job.target)),
    )
}

/// The last file that `job` writes. If it exists, the job is already done.
pub fn final_output(job: &Job) -> Option<PathBuf> {
    if job.burn {
        Some(video_output(job, "mp4"))
    } else if job.embed {
        Some(video_output(job, "mkv"))
    } else if job.translation.is_some() {
        Some(output_path(job, &job.target))
    } else {
        None
    }
}

impl Pipeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `job` and reports to `sink`.
    ///
    /// # Errors
    /// Returns an error if a step fails, and a `Cancelled` error if `sink` stops the job.
    pub fn run(&mut self, job: &Job, sink: &dyn Sink) -> Result<Outcome> {
        let report = Report(sink);
        check_cancel(&report)?;
        if let Some(done) = final_output(job)
            && done.exists()
            && !job.force
        {
            sink.event(Event::Skipped(&done));
            return Ok(Outcome {
                skipped: true,
                ..Outcome::default()
            });
        }
        if job.embed || job.burn {
            check_video_output(job)?;
        }

        let whisper = resolve(&job.whisper_model, models::Kind::Whisper, sink)?;
        let vad = if job.vad {
            let spec = models::find(models::VAD).context("VAD model missing")?;
            Some(ensure(spec, sink)?)
        } else {
            None
        };

        sink.event(Event::Step(Step::Decode));
        let pcm = audio::decode_pcm(&job.video, job.audio_track, &report)?;
        tracing::info!("decoded {}", audio_length(pcm.len()));
        sink.event(Event::Step(Step::Transcribe));
        let opts = transcribe::Options {
            model: &whisper,
            vad_model: vad.as_deref(),
            language: job.language.as_deref(),
        };
        let transcript = transcribe::transcribe(&pcm, &opts, &report)?;
        drop(pcm);
        tracing::info!(
            "transcribed {} cues, language {}",
            transcript.cues.len(),
            transcript.language
        );

        let mut files = Vec::new();
        let source_lang = transcript.language.clone();
        let source_cues = subtitle::layout_transcript(&transcript.cues);
        // Without a translation, `output` names the original transcript, since nothing
        // else is written.
        let source_out = match (&job.output, &job.translation) {
            (Some(out), None) => out.clone(),
            _ => output_path(job, &source_lang),
        };
        write(&source_out, &source_cues, job.format, sink, &mut files)?;

        let target_cues = match &job.translation {
            None => None,
            Some(_) if source_lang == job.target => {
                let note = format!(
                    "source language is already {}; no translation needed",
                    job.target
                );
                sink.event(Event::Note(&note));
                None
            }
            Some(settings) => {
                let cues = self.translate(job, settings, &source_cues, &source_lang, sink)?;
                if !job.embed && !job.burn {
                    sink.event(Event::Step(Step::Save));
                }
                write(
                    &output_path(job, &job.target),
                    &cues,
                    job.format,
                    sink,
                    &mut files,
                )?;
                Some(cues)
            }
        };

        self.video_outputs(
            job,
            &source_lang,
            &source_cues,
            target_cues.as_deref(),
            sink,
            &mut files,
        )?;
        Ok(Outcome {
            source_language: source_lang,
            files,
            skipped: false,
        })
    }

    /// Translates `cues` and lays out the result. The translator loads on first use and
    /// stays loaded while the settings stay the same.
    fn translate(
        &mut self,
        job: &Job,
        settings: &Translation,
        cues: &[Cue],
        source_lang: &str,
        sink: &dyn Sink,
    ) -> Result<Vec<Cue>> {
        let same = matches!(&self.translator, Some((target, s, _)) if *target == job.target && s == settings);
        if !same {
            self.translator = None;
            // Download a missing local model here, so the download is its own step.
            let llm_model = if settings.backend == translate::Backend::Local {
                resolve(&settings.llm_model, models::Kind::Llm, sink)?
                    .display()
                    .to_string()
            } else {
                settings.llm_model.clone()
            };
            let translator = translate::create(&translate::Config {
                backend: settings.backend,
                target: job.target.clone(),
                register: settings.register,
                glossary: match &settings.glossary {
                    Some(path) => translate::load_glossary(path)?,
                    None => Vec::new(),
                },
                llm_model,
                endpoint: settings.endpoint.clone(),
                endpoint_model: settings.endpoint_model.clone(),
            })?;
            self.translator = Some((job.target.clone(), settings.clone(), translator));
        }
        let Some((_, _, translator)) = self.translator.as_mut() else {
            bail!("the translator is not loaded");
        };
        sink.event(Event::Step(Step::Translate));
        let translated =
            translate::translate_cues(translator.as_mut(), cues, source_lang, &Report(sink))?;
        Ok(subtitle::layout_translation(&translated))
    }
}

fn write(
    path: &Path,
    cues: &[Cue],
    format: Format,
    sink: &dyn Sink,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    fs::write(path, subtitle::write(cues, format))
        .with_context(|| format!("cannot write {}", path.display()))?;
    sink.event(Event::Wrote(path));
    files.push(path.to_path_buf());
    Ok(())
}

/// "1:42:08 of audio" for a number of 16 kHz samples.
fn audio_length(samples: usize) -> String {
    let s = samples as u64 / u64::from(audio::SAMPLE_RATE);
    format!("{}:{:02}:{:02} of audio", s / 3600, s / 60 % 60, s % 60)
}

/// Resolves a model name or path. A file with that name wins over a known model. A
/// missing known model downloads as its own step.
fn resolve(value: &str, kind: models::Kind, sink: &dyn Sink) -> Result<PathBuf> {
    match models::find(value) {
        Some(spec) if spec.kind == kind && !Path::new(value).is_file() => ensure(spec, sink),
        _ => models::resolve(value, kind, &Report(sink)),
    }
}

fn ensure(spec: &'static models::ModelSpec, sink: &dyn Sink) -> Result<PathBuf> {
    if !models::cached_path(spec)?.is_file() {
        sink.event(Event::Step(Step::Download(spec.name)));
    }
    models::ensure(spec, &Report(sink))
}

#[cfg(feature = "video")]
fn check_video_output(job: &Job) -> Result<()> {
    if job.burn {
        crate::video::check_burn(&job.video)?;
    }
    Ok(())
}

#[cfg(not(feature = "video"))]
fn check_video_output(_job: &Job) -> Result<()> {
    use verbacue_core::{Error, ErrorKind};
    Err(Error::new(
        ErrorKind::InvalidInput,
        "this build cannot write videos",
    ))
}

impl Pipeline {
    #[cfg(feature = "video")]
    fn video_outputs(
        &mut self,
        job: &Job,
        source_lang: &str,
        source_cues: &[Cue],
        target_cues: Option<&[Cue]>,
        sink: &dyn Sink,
        files: &mut Vec<PathBuf>,
    ) -> Result<()> {
        use crate::video;
        // The translated track comes first, so it becomes the default one.
        let mut tracks = Vec::new();
        if let Some(cues) = target_cues {
            tracks.push(video::SubtitleTrack {
                language: &job.target,
                cues,
            });
        }
        tracks.push(video::SubtitleTrack {
            language: source_lang,
            cues: source_cues,
        });
        if job.embed {
            sink.event(Event::Step(Step::Embed));
            let out = video_output(job, "mkv");
            video::embed(&job.video, &out, &tracks, &Report(sink))?;
            sink.event(Event::Wrote(&out));
            files.push(out);
        }
        if job.burn {
            sink.event(Event::Step(Step::Burn));
            let out = video_output(job, "mp4");
            video::burn(&job.video, &out, tracks[0].cues, &Report(sink))?;
            sink.event(Event::Wrote(&out));
            files.push(out);
        }
        Ok(())
    }

    #[cfg(not(feature = "video"))]
    fn video_outputs(
        &mut self,
        _job: &Job,
        _source_lang: &str,
        _source_cues: &[Cue],
        _target_cues: Option<&[Cue]>,
        _sink: &dyn Sink,
        _files: &mut Vec<PathBuf>,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbacue_core::ErrorKind;

    fn job(video: &str, target: &str) -> Job {
        Job::new(PathBuf::from(video), target)
    }

    #[test]
    fn output_paths_follow_the_cli_rules() {
        let j = job("/x/movie.mkv", "hu");
        assert_eq!(output_path(&j, "hu"), PathBuf::from("/x/movie.hu.srt"));
        assert_eq!(output_path(&j, "tr"), PathBuf::from("/x/movie.tr.srt"));
        let mut o = job("/x/movie.mkv", "hu");
        o.output = Some(PathBuf::from("/y/out.srt"));
        o.format = subtitle::Format::Vtt;
        assert_eq!(output_path(&o, "hu"), PathBuf::from("/y/out.srt"));
        assert_eq!(output_path(&o, "tr"), PathBuf::from("/x/movie.tr.vtt"));
        let s = job("/x/Szép film.mkv", "hu");
        assert_eq!(output_path(&s, "hu"), PathBuf::from("/x/Szép film.hu.srt"));
    }

    #[test]
    fn out_dir_moves_every_output() {
        let mut j = job("/x/movie.mkv", "hu");
        j.out_dir = Some(PathBuf::from("/out"));
        j.burn = true;
        assert_eq!(output_path(&j, "tr"), PathBuf::from("/out/movie.tr.srt"));
        assert_eq!(final_output(&j), Some(PathBuf::from("/out/movie.hu.mp4")));
    }

    #[test]
    fn final_output_prefers_burn_then_embed_then_translation() {
        let mut j = job("/x/m.mkv", "hu");
        assert_eq!(final_output(&j), None);
        j.translation = Some(Translation::default());
        assert_eq!(final_output(&j), Some(PathBuf::from("/x/m.hu.srt")));
        j.embed = true;
        assert_eq!(final_output(&j), Some(PathBuf::from("/x/m.hu.mkv")));
        j.burn = true;
        assert_eq!(final_output(&j), Some(PathBuf::from("/x/m.hu.mp4")));
    }

    #[derive(Default)]
    struct Record {
        events: std::sync::Mutex<Vec<String>>,
        cancel: bool,
    }

    impl Sink for Record {
        fn event(&self, event: Event<'_>) {
            let text = match event {
                Event::Step(step) => format!("step {step:?}"),
                Event::Skipped(path) => format!("skipped {}", path.display()),
                other => format!("{other:?}"),
            };
            self.events.lock().unwrap().push(text);
        }

        fn cancelled(&self) -> bool {
            self.cancel
        }
    }

    #[test]
    fn existing_final_output_skips_the_job() {
        let dir = std::env::temp_dir().join("verbacue-pipeline-skip");
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("movie.mkv");
        std::fs::write(dir.join("movie.hu.srt"), "").unwrap();
        let mut j = Job::new(video, "hu");
        j.translation = Some(Translation::default());
        let sink = Record::default();
        let outcome = Pipeline::new().run(&j, &sink).unwrap();
        assert!(outcome.skipped);
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].starts_with("skipped"));
    }

    #[test]
    fn cancelled_sink_stops_before_decoding() {
        let j = job("/x/missing.mkv", "hu");
        let sink = Record {
            cancel: true,
            ..Record::default()
        };
        let err = Pipeline::new().run(&j, &sink).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert!(
            !sink
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e == "step Decode")
        );
    }
}
