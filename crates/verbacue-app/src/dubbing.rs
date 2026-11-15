//! The dub options and commands: `--dub`, `verbacue dub`, `verbacue voices` and the
//! hidden `verbacue speak`.

use std::{
    io::{BufRead as _, IsTerminal as _, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use clap::{Args, ValueEnum};
use verbacue::{
    dub::{self, Original, VoiceChoice, Xtts},
    models, subtitle,
    video::SubtitleTrack,
};

use crate::{BarSink, ensure_bundle, split_lang_suffix};

/// What happens to the original speech.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum DubOriginal {
    /// Take the original voices out (slower).
    Remove,
    /// Keep the original audio, lower while a new line plays (faster).
    Lower,
}

#[derive(Args, Clone, Debug)]
pub struct DubArgs {
    /// The voice: "clone" for the voice of each speaker, or a studio voice from
    /// `verbacue voices`.
    #[arg(long, default_value = "clone")]
    pub dub_voice: String,

    /// What happens to the original speech.
    #[arg(long, value_enum, default_value_t = DubOriginal::Remove)]
    pub dub_original: DubOriginal,

    /// The highest speed-up of a line without a warning, from 1.0 to 1.5.
    #[arg(long, default_value_t = 1.3, value_parser = speed_limit)]
    pub dub_speed_limit: f32,

    /// The voice model: xtts-v2, xtts-v2-int8, or a folder with the model files. The
    /// default is xtts-v2 in a CUDA build, else xtts-v2-int8.
    #[arg(long)]
    pub dub_model: Option<String>,

    /// Accept the licence of the voice model (non-commercial use only) without a
    /// question. Needed without a terminal.
    #[arg(long)]
    pub accept_model_licence: bool,
}

fn speed_limit(value: &str) -> std::result::Result<f32, String> {
    let v: f32 = value
        .parse()
        .map_err(|_| format!("{value:?} is not a number"))?;
    if (1.0..=1.5).contains(&v) {
        Ok(v)
    } else {
        Err("the speed limit must be from 1.0 to 1.5".into())
    }
}

impl DubArgs {
    pub fn options(&self) -> dub::Options {
        dub::Options {
            voice: if self.dub_voice == "clone" {
                VoiceChoice::Clone
            } else {
                VoiceChoice::Studio(self.dub_voice.clone())
            },
            original: match self.dub_original {
                DubOriginal::Remove => Original::Remove,
                DubOriginal::Lower => Original::Lower,
            },
            speed_limit: self.dub_speed_limit,
            model: self
                .dub_model
                .clone()
                .unwrap_or_else(|| dub::default_model().to_string()),
        }
    }
}

/// Asks the user to accept a model licence. Without a terminal, the answer is no.
pub fn ask_licence(model: &str, licence: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        eprintln!("{licence}\nUse --accept-model-licence to accept it.");
        return false;
    }
    eprintln!("{licence}");
    eprint!("Accept the {model} licence (non-commercial use only)? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).is_ok()
        && matches!(answer.trim(), "y" | "Y" | "yes")
}

/// The folder of a voice model: a folder as it is, or a known model, downloaded after
/// the user accepts its licence.
fn model_dir(model: &str, accept: bool) -> Result<PathBuf> {
    if Path::new(model).is_dir() {
        return Ok(PathBuf::from(model));
    }
    let bundle = models::bundle(model)
        .filter(|b| b.kind == models::Kind::Voice)
        .with_context(|| format!("unknown voice model {model:?}"))?;
    if !bundle.is_cached()
        && let Some(licence) = bundle.licence
        && !accept
        && !ask_licence(bundle.name, licence)
    {
        bail!("the licence of {} was not accepted", bundle.name);
    }
    ensure_bundle(bundle)
}

/// The separation model: next to a local voice model, else the downloaded bundle.
fn separation_model(voice_dir: &Path) -> Result<PathBuf> {
    local_or_bundle(voice_dir, "htdemucs_vocals.onnx", "htdemucs-vocals")
}

/// `file` next to a local voice model, else in the downloaded bundle `name`.
fn local_or_bundle(voice_dir: &Path, file: &str, name: &str) -> Result<PathBuf> {
    let local = voice_dir.join(file);
    if local.is_file() {
        return Ok(local);
    }
    let bundle = models::bundle(name).with_context(|| format!("the model {name} is unknown"))?;
    Ok(ensure_bundle(bundle)?.join(file))
}

/// `verbacue dub`: dubs a video with an existing subtitle file.
pub fn dub_file(
    video: &Path,
    subtitles: &Path,
    to: Option<String>,
    output: Option<PathBuf>,
    force: bool,
    audio_track: usize,
    args: &DubArgs,
) -> Result<()> {
    let lang = to
        .or_else(|| split_lang_suffix(subtitles).1)
        .context("the language is not in the SRT name (movie.hu.srt); use --to")?;
    if !dub::supports(&lang) {
        bail!(
            "the voice model cannot speak {lang:?}; it knows {}",
            dub::LANGUAGES.join(", ")
        );
    }
    let tracks = verbacue::audio::list_tracks(video)?;
    if audio_track >= tracks.len() {
        bail!(
            "{} has no audio track {audio_track}; see verbacue tracks",
            video.display()
        );
    }
    let out = output.unwrap_or_else(|| video.with_extension(format!("{lang}.dub.mkv")));
    if out.exists() && !force {
        tracing::info!("skipped: {} exists (use --force)", out.display());
        return Ok(());
    }
    let text = std::fs::read_to_string(subtitles)
        .with_context(|| format!("cannot read {}", subtitles.display()))?;
    let cues = subtitle::parse_srt(&text)
        .with_context(|| format!("cannot parse {}", subtitles.display()))?;
    let options = args.options();
    let dir = model_dir(&options.model, args.accept_model_licence)?;
    let separation = if options.original == Original::Remove {
        Some(separation_model(&dir)?)
    } else {
        None
    };
    let speakers = if options.voice == dub::VoiceChoice::Clone {
        Some(local_or_bundle(
            &dir,
            dub::speakers::MODEL_FILE,
            "campplus",
        )?)
    } else {
        None
    };
    let sink = BarSink::new(video);
    let result = dub::dub(
        dub::DubJob {
            video,
            track: audio_track,
            cues: &cues,
            lang: &lang,
            models_dir: &dir,
            separation_model: separation.as_deref(),
            speaker_model: speakers.as_deref(),
            options,
            temp_dir: &models::cache_dir()?.join("dub-tmp"),
            output: &out,
            subtitle_tracks: &[SubtitleTrack {
                language: &lang,
                cues: &cues,
            }],
        },
        &sink,
    );
    sink.finish();
    result?;
    tracing::info!("wrote {}", out.display());
    Ok(())
}

/// `verbacue voices`: prints the studio voices, one per line.
pub fn print_voices(model: Option<&str>, accept: bool) -> Result<()> {
    let dir = model_dir(model.unwrap_or(dub::default_model()), accept)?;
    for name in dub::xtts::studio_voices_in(&dir)? {
        out!("{name}");
    }
    Ok(())
}

/// `verbacue speak`: speaks one line into a WAV file.
pub fn speak(
    text: &str,
    lang: &str,
    voice: &str,
    model: &str,
    seed: u64,
    output: &Path,
) -> Result<()> {
    if !dub::supports(lang) {
        bail!(
            "XTTS-v2 cannot speak {lang:?}; it knows {}",
            dub::LANGUAGES.join(", ")
        );
    }
    let dir = model_dir(model, false)?;
    let int8 = model.ends_with("int8") || !dir.join("gpt.onnx").is_file();
    let start = std::time::Instant::now();
    let mut xtts = Xtts::load(&dir, int8)?;
    let voice = if Path::new(voice).is_file() {
        let (audio, rate) = dub::wav::read(Path::new(voice))?;
        xtts.clone_voice(&dub::resample::resample(&audio, rate, dub::SAMPLE_RATE))?
    } else {
        xtts.studio_voice(voice).with_context(|| {
            format!(
                "unknown voice {voice:?}; the studio voices are: {}",
                xtts.studio_voices().join(", ")
            )
        })?
    };
    let latents = xtts.latents(text, lang, &voice, seed)?;
    let audio = xtts.vocode(&latents, &voice)?;
    dub::wav::write(output, &audio, dub::SAMPLE_RATE)?;
    #[expect(clippy::cast_precision_loss, reason = "a duration")]
    let seconds = audio.len() as f32 / dub::SAMPLE_RATE as f32;
    eprintln!("{seconds:.1} s of speech in {:.1?}", start.elapsed());
    out!("{}", output.display());
    Ok(())
}
