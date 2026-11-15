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
    /// Write the subtitle files (`.srt` or `.vtt`). The video outputs have the
    /// subtitles in any case.
    pub subtitles: bool,
    /// Also write an MKV with soft subtitle tracks.
    pub embed: bool,
    /// Also write an MP4 with the subtitles burned into the picture.
    pub burn: bool,
    /// Run again even if the output exists.
    pub force: bool,
    /// Also write an MKV with a new audio track that speaks the translation.
    #[cfg(feature = "dub")]
    pub dub: Option<crate::dub::Options>,
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
            subtitles: true,
            embed: false,
            burn: false,
            force: false,
            #[cfg(feature = "dub")]
            dub: None,
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
    /// What the user knows about the video, for example the characters.
    pub notes: String,
    /// Read the whole transcript first and write notes for the translation.
    pub film_notes: bool,
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
            notes: String::new(),
            film_notes: false,
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
    /// Reading the whole transcript to write notes for the translation.
    Notes,
    Translate,
    Embed,
    Burn,
    /// Writing the subtitle files of a job without video output.
    Save,
    /// Dubbing: decoding the audio, and taking the original voices out.
    Separate,
    /// Dubbing: finding the speakers and their voices.
    Speakers,
    /// Dubbing: speaking the lines.
    Speak,
    /// Dubbing: mixing and writing the video.
    Mix,
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

    /// Shows the film notes before the translation starts. A sink that returns text
    /// replaces the notes with it; `None` keeps them. The pipeline waits for the answer.
    fn review_notes(&self, notes: &str) -> Option<String> {
        let _ = notes;
        None
    }

    /// Asks the user to accept the licence of `model` before its first download. The
    /// default is no.
    fn accept_licence(&self, model: &str, licence: &str) -> bool {
        let _ = (model, licence);
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

/// The dubbed video: `movie.<lang>.dub.mkv`.
#[cfg(feature = "dub")]
pub fn dub_output(job: &Job) -> PathBuf {
    video_output(job, "dub.mkv")
}

/// The cues of a subtitle file from an earlier run, if it exists, reads well and the
/// job does not force a new run.
fn earlier(path: &Path, job: &Job) -> Option<Vec<Cue>> {
    if job.force || !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    // A WebVTT file starts with a header block; the cues after it read like SRT.
    let text = match job.format {
        Format::Vtt => text
            .replace("\r\n", "\n")
            .split_once("\n\n")
            .map_or_else(String::new, |(_, cues)| cues.to_string()),
        Format::Srt => text,
    };
    let cues = subtitle::parse_srt(&text).ok()?;
    (!cues.is_empty()).then_some(cues)
}

/// The transcript of an earlier run: its language, path and cues. With an unknown
/// spoken language, exactly one transcript file must exist, so that the language is
/// clear.
fn earlier_transcript(job: &Job) -> Option<(String, PathBuf, Vec<Cue>)> {
    let candidates: Vec<String> = match &job.language {
        Some(lang) => vec![lang.clone()],
        None => verbacue_core::languages::languages()
            .iter()
            .map(|(code, _)| (*code).to_string())
            .filter(|code| *code != job.target && output_path(job, code).is_file())
            .collect(),
    };
    let [lang] = candidates.as_slice() else {
        return None;
    };
    // Without a translation, `output` names the transcript.
    let path = match (&job.output, &job.translation) {
        (Some(out), None) => out.clone(),
        _ => output_path(job, lang),
    };
    Some((lang.clone(), path.clone(), earlier(&path, job)?))
}

/// Tells the user that a file of an earlier run is used again.
fn reused(sink: &dyn Sink, what: &str, path: &Path) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    sink.event(Event::Note(&format!("reused the {what} {name}")));
}

/// True when `job` writes a dubbed video.
fn dubbing(job: &Job) -> bool {
    #[cfg(feature = "dub")]
    return job.dub.is_some();
    #[cfg(not(feature = "dub"))]
    {
        let _ = job;
        false
    }
}

/// The last file that `job` writes. If it exists, the job is already done.
pub fn final_output(job: &Job) -> Option<PathBuf> {
    #[cfg(feature = "dub")]
    if job.dub.is_some() {
        return Some(dub_output(job));
    }
    if job.burn {
        Some(video_output(job, "mp4"))
    } else if job.embed {
        Some(video_output(job, "mkv"))
    } else if job.translation.is_some() && job.subtitles {
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
        #[cfg(feature = "dub")]
        let dub_model = dub::check(job, sink)?;

        #[cfg(feature = "dub")]
        let dub_dirs = dub_model.map(|m| m.download(sink)).transpose()?;
        #[cfg(feature = "dub")]
        let dub_dirs = dub_dirs.as_ref();
        #[cfg(not(feature = "dub"))]
        let dub_dirs: Option<&DubDirs> = None;

        let mut files = Vec::new();
        let (source_lang, source_cues) = if let Some((lang, path, cues)) = earlier_transcript(job) {
            reused(sink, "transcript", &path);
            if job.subtitles {
                files.push(path);
            }
            (lang, cues)
        } else {
            let (lang, cues) = self.transcribe(job, sink)?;
            // Without a translation, `output` names the original transcript, since
            // nothing else is written.
            let source_out = match (&job.output, &job.translation) {
                (Some(out), None) => out.clone(),
                _ => output_path(job, &lang),
            };
            if job.subtitles {
                write(&source_out, &cues, job.format, sink, &mut files)?;
            }
            (lang, cues)
        };

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
                let path = output_path(job, &job.target);
                if let Some(cues) = earlier(&path, job) {
                    reused(sink, "translation", &path);
                    if job.subtitles {
                        files.push(path);
                    }
                    Some(cues)
                } else {
                    let cues = self.translate(job, settings, &source_cues, &source_lang, sink)?;
                    if job.subtitles {
                        if !job.embed && !job.burn && !dubbing(job) {
                            sink.event(Event::Step(Step::Save));
                        }
                        write(&path, &cues, job.format, sink, &mut files)?;
                    }
                    Some(cues)
                }
            }
        };

        self.video_outputs(
            job,
            &source_lang,
            &source_cues,
            target_cues.as_deref(),
            dub_dirs,
            sink,
            &mut files,
        )?;
        Ok(Outcome {
            source_language: source_lang,
            files,
            skipped: false,
        })
    }

    /// Loads the speech models, decodes the audio and transcribes it. Returns the spoken
    /// language and the cues.
    fn transcribe(&mut self, job: &Job, sink: &dyn Sink) -> Result<(String, Vec<Cue>)> {
        let report = Report(sink);
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
        Ok((
            transcript.language,
            subtitle::layout_transcript(&transcript.cues),
        ))
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
        let same = matches!(&self.translator, Some((target, s, _)) if *target == job.target && same_translator(s, settings));
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
        let texts: Vec<String> = cues.iter().map(|c| c.text.clone()).collect();
        let mut hints = translate::Hints {
            title: job
                .video
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            notes: settings.notes.trim().to_string(),
            film_notes: String::new(),
            names: translate::proper_names(&texts),
        };
        if settings.film_notes {
            sink.event(Event::Step(Step::Notes));
            if let Some(notes) = translator.film_notes(&texts, source_lang, &Report(sink))? {
                tracing::info!("film notes: {notes}");
                hints.film_notes = sink.review_notes(&notes).unwrap_or(notes);
                check_cancel(&Report(sink))?;
            }
        }
        sink.event(Event::Step(Step::Translate));
        let translated = translate::translate_cues_with(
            translator.as_mut(),
            cues,
            source_lang,
            &hints,
            &Report(sink),
        )?;
        Ok(subtitle::layout_translation(&translated))
    }
}

/// True when two settings use the same translator. The notes change per video and
/// need no new translator.
fn same_translator(a: &Translation, b: &Translation) -> bool {
    a.backend == b.backend
        && a.register == b.register
        && a.glossary == b.glossary
        && a.llm_model == b.llm_model
        && a.endpoint == b.endpoint
        && a.endpoint_model == b.endpoint_model
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
    #[expect(
        clippy::too_many_arguments,
        reason = "the results of the earlier steps"
    )]
    fn video_outputs(
        &mut self,
        job: &Job,
        source_lang: &str,
        source_cues: &[Cue],
        target_cues: Option<&[Cue]>,
        dub_dirs: Option<&DubDirs>,
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
        #[cfg(feature = "dub")]
        if let (Some(options), Some(dirs)) = (&job.dub, dub_dirs) {
            let cues = match target_cues {
                Some(cues) => cues,
                None if source_lang == job.target => source_cues,
                None => bail!("dubbing needs a translation to {}", job.target),
            };
            let out = dub_output(job);
            let temp = models::cache_dir()?.join("dub-tmp");
            crate::dub::dub(
                crate::dub::DubJob {
                    video: &job.video,
                    track: job.audio_track,
                    cues,
                    lang: &job.target,
                    models_dir: &dirs.voice,
                    separation_model: dirs.separation.as_deref(),
                    options: options.clone(),
                    temp_dir: &temp,
                    output: &out,
                    subtitle_tracks: &tracks,
                },
                &dub::Events(sink),
            )?;
            sink.event(Event::Wrote(&out));
            files.push(out);
        }
        #[cfg(not(feature = "dub"))]
        let _ = dub_dirs;
        Ok(())
    }

    #[cfg(not(feature = "video"))]
    fn video_outputs(
        &mut self,
        _job: &Job,
        _source_lang: &str,
        _source_cues: &[Cue],
        _target_cues: Option<&[Cue]>,
        _dub_dirs: Option<&DubDirs>,
        _sink: &dyn Sink,
        _files: &mut Vec<PathBuf>,
    ) -> Result<()> {
        Ok(())
    }
}

/// The model folders of a dub job.
#[cfg(feature = "dub")]
type DubDirs = dub::Dirs;
#[cfg(not(feature = "dub"))]
type DubDirs = ();

/// The checks and the model of a dub job.
#[cfg(feature = "dub")]
mod dub {
    use std::path::{Path, PathBuf};

    use verbacue_core::{Error, ErrorKind, Result, error::Context as _, models};

    use super::{Event, Job, Sink, Step, ensure};
    use crate::{audio, dub};

    /// A model of a dub job: a local folder or a known bundle.
    pub(super) enum Model {
        Dir(PathBuf),
        Bundle(&'static models::Bundle),
    }

    impl Model {
        /// The folder with the model files. It downloads a missing bundle, one step
        /// for each file.
        fn dir(self, sink: &dyn Sink) -> Result<PathBuf> {
            match self {
                Model::Dir(dir) => Ok(dir),
                Model::Bundle(bundle) => {
                    for file in bundle.files {
                        ensure(
                            models::find(file).context("a bundle file is unknown")?,
                            sink,
                        )?;
                    }
                    bundle.cached_dir()
                }
            }
        }
    }

    /// The models of a dub job.
    pub(super) struct Models {
        voice: Model,
        /// For `Original::Remove` only.
        separation: Option<Model>,
    }

    /// The model folders of a dub job, after the downloads.
    pub(super) struct Dirs {
        pub(super) voice: PathBuf,
        pub(super) separation: Option<PathBuf>,
    }

    impl Models {
        pub(super) fn download(self, sink: &dyn Sink) -> Result<Dirs> {
            let voice = self.voice.dir(sink)?;
            let separation = match self.separation {
                Some(m) => Some(m.dir(sink)?.join(SEPARATION_FILE)),
                None => None,
            };
            Ok(Dirs { voice, separation })
        }
    }

    const SEPARATION_BUNDLE: &str = "htdemucs-vocals";
    const SEPARATION_FILE: &str = "htdemucs_vocals.onnx";

    /// Checks a dub job before the slow steps: the language, the audio track and the
    /// licence. It downloads nothing.
    pub(super) fn check(job: &Job, sink: &dyn Sink) -> Result<Option<Models>> {
        let Some(options) = &job.dub else {
            return Ok(None);
        };
        if !dub::supports(&job.target) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "the voice model cannot speak {:?}; it knows {}",
                    job.target,
                    dub::LANGUAGES.join(", ")
                ),
            ));
        }
        let tracks = audio::list_tracks(&job.video)?;
        if job.audio_track >= tracks.len() {
            let message = if tracks.is_empty() {
                format!("{} has no audio to dub", job.video.display())
            } else {
                format!(
                    "{} has no audio track {}; it has {} (see verbacue tracks)",
                    job.video.display(),
                    job.audio_track,
                    tracks.len()
                )
            };
            return Err(Error::new(ErrorKind::InvalidInput, message));
        }
        let separation = if options.original == dub::Original::Remove {
            Some(separation_model(&options.model)?)
        } else {
            None
        };
        if Path::new(&options.model).is_dir() {
            return Ok(Some(Models {
                voice: Model::Dir(PathBuf::from(&options.model)),
                separation,
            }));
        }
        let bundle = models::bundle(&options.model)
            .filter(|b| b.kind == models::Kind::Voice)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("unknown voice model {:?}", options.model),
                )
            })?;
        if !bundle.is_cached()
            && let Some(licence) = bundle.licence
            && !sink.accept_licence(bundle.name, licence)
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("the licence of {} was not accepted", bundle.name),
            ));
        }
        Ok(Some(Models {
            voice: Model::Bundle(bundle),
            separation,
        }))
    }

    /// The separation model: next to a local voice model folder, else the bundle.
    fn separation_model(voice_model: &str) -> Result<Model> {
        let dir = Path::new(voice_model);
        if dir.join(SEPARATION_FILE).is_file() {
            return Ok(Model::Dir(dir.to_path_buf()));
        }
        models::bundle(SEPARATION_BUNDLE)
            .map(Model::Bundle)
            .context("the separation model is unknown")
    }

    /// Shows the dub events as pipeline events.
    pub(super) struct Events<'a>(pub(super) &'a dyn Sink);

    impl dub::Events for Events<'_> {
        fn step(&self, step: dub::DubStep) {
            self.0.event(Event::Step(match step {
                dub::DubStep::Separate => Step::Separate,
                dub::DubStep::Speakers => Step::Speakers,
                dub::DubStep::Speak => Step::Speak,
                dub::DubStep::Mix => Step::Mix,
            }));
        }

        fn progress(&self, done: u64, total: u64) {
            self.0.event(Event::Progress { done, total });
        }

        fn note(&self, text: &str) {
            self.0.event(Event::Note(text));
        }

        fn cancelled(&self) -> bool {
            self.0.cancelled()
        }
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
        j.subtitles = false;
        assert_eq!(final_output(&j), None);
        j.subtitles = true;
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

    const SRT: &str = "1\n00:00:01,000 --> 00:00:02,000\nMerhaba\n\n";

    fn reuse_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("verbacue-pipeline-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn earlier_files_skip_transcription_and_translation() {
        let dir = reuse_dir("reuse");
        std::fs::write(dir.join("movie.tr.srt"), SRT).unwrap();
        std::fs::write(dir.join("movie.hu.srt"), SRT.replace("Merhaba", "Szia")).unwrap();
        let mut j = Job::new(dir.join("movie.mkv"), "hu");
        j.translation = Some(Translation::default());
        // Without subtitle files and videos, the job is not "done", so it runs. It
        // needs no model, because both files exist.
        j.subtitles = false;
        let sink = Record::default();
        let outcome = Pipeline::new().run(&j, &sink).unwrap();
        assert_eq!(outcome.source_language, "tr");
        let events = sink.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.contains("reused the transcript movie.tr.srt")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| e.contains("reused the translation movie.hu.srt"))
        );
        assert!(
            !events
                .iter()
                .any(|e| e.contains("Decode") || e.contains("Translate"))
        );
    }

    #[test]
    fn the_transcript_must_be_clear_and_force_ignores_it() {
        let dir = reuse_dir("reuse-unclear");
        let mut j = Job::new(dir.join("movie.mkv"), "hu");
        j.translation = Some(Translation::default());
        std::fs::write(dir.join("movie.tr.srt"), SRT).unwrap();
        assert_eq!(earlier_transcript(&j).map(|t| t.0).as_deref(), Some("tr"));
        // Two transcripts: the spoken language is not clear.
        std::fs::write(dir.join("movie.de.srt"), SRT).unwrap();
        assert!(earlier_transcript(&j).is_none());
        // A given spoken language picks its file.
        j.language = Some("de".into());
        assert_eq!(earlier_transcript(&j).map(|t| t.0).as_deref(), Some("de"));
        j.force = true;
        assert!(earlier_transcript(&j).is_none());
    }

    #[test]
    fn an_earlier_webvtt_file_reads_too() {
        let dir = reuse_dir("reuse-vtt");
        let path = dir.join("movie.hu.vtt");
        std::fs::write(
            &path,
            "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500\nSzia\n\n",
        )
        .unwrap();
        let mut j = Job::new(dir.join("movie.mkv"), "hu");
        j.format = subtitle::Format::Vtt;
        let cues = earlier(&path, &j).unwrap();
        assert_eq!(cues, [Cue::new(1000, 2500, "Szia")]);
        // An empty or broken file is not used.
        std::fs::write(&path, "").unwrap();
        assert!(earlier(&path, &j).is_none());
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

    #[cfg(feature = "dub")]
    #[test]
    fn dub_output_path() {
        let mut j = job("/x/movie.mkv", "hu");
        j.dub = Some(crate::dub::Options::default());
        assert_eq!(dub_output(&j), PathBuf::from("/x/movie.hu.dub.mkv"));
        assert_eq!(final_output(&j), Some(PathBuf::from("/x/movie.hu.dub.mkv")));
        j.out_dir = Some(PathBuf::from("/out"));
        assert_eq!(dub_output(&j), PathBuf::from("/out/movie.hu.dub.mkv"));
    }

    #[cfg(feature = "dub")]
    #[test]
    fn dub_refuses_unknown_language() {
        let mut j = job("/x/missing.mkv", "th");
        j.dub = Some(crate::dub::Options::default());
        let sink = Record::default();
        let err = Pipeline::new().run(&j, &sink).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().contains("hu"), "{err}");
        assert!(sink.events.lock().unwrap().is_empty());
    }

    /// A test video with one audio track, made with the ffmpeg command.
    #[cfg(feature = "dub")]
    fn one_track_video(name: &str) -> Option<PathBuf> {
        let path = std::env::temp_dir().join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=size=160x120:rate=25:duration=1")
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=duration=1",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipped: the ffmpeg command is needed to make the test video");
        }
        ok.then_some(path)
    }

    #[cfg(feature = "dub")]
    #[test]
    fn dub_refuses_missing_audio_track() {
        let Some(video) = one_track_video("verbacue-pipeline-dub-track.mkv") else {
            return;
        };
        let mut j = Job::new(video, "hu");
        j.audio_track = 5;
        j.dub = Some(crate::dub::Options::default());
        let sink = Record::default();
        let err = Pipeline::new().run(&j, &sink).unwrap_err();
        assert!(err.to_string().contains("no audio track 5"), "{err}");
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[cfg(feature = "dub")]
    #[test]
    fn dub_needs_licence() {
        let Some(video) = one_track_video("verbacue-pipeline-dub-licence.mkv") else {
            return;
        };
        let options = crate::dub::Options::default();
        if models::bundle(&options.model).is_some_and(models::Bundle::is_cached) {
            eprintln!("skipped: the voice model is already downloaded");
            return;
        }
        let mut j = Job::new(video, "hu");
        j.dub = Some(options);
        let sink = Record::default();
        let err = Pipeline::new().run(&j, &sink).unwrap_err();
        assert!(err.to_string().contains("licence"), "{err}");
        assert!(
            sink.events.lock().unwrap().is_empty(),
            "nothing is downloaded"
        );
    }

    #[test]
    fn notes_do_not_reload_the_translator() {
        let base = Translation::default();
        let notes = Translation {
            notes: "Two sisters.".into(),
            film_notes: true,
            ..Translation::default()
        };
        assert!(same_translator(&base, &notes));
        let other = Translation {
            llm_model: "gemma-3-12b".into(),
            ..Translation::default()
        };
        assert!(!same_translator(&base, &other));
    }
}
