/// Prints a line to stdout like `println!`. If the reader has closed the pipe (for
/// example `verbacue languages | head`), the program stops quietly instead of panicking.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            panic!("cannot write to stdout: {e}");
        }
    }};
}

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tracing_subscriber::EnvFilter;
use verbacue::{
    audio, models,
    subtitle::{self, Cue, Format},
    transcribe,
    translate::{self, Backend, Register, Translator},
    video,
};

#[derive(Parser)]
#[command(
    version,
    about = "Generate translated subtitles for videos",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Video files, or directories that contain videos.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Source language code. "auto" detects it.
    #[arg(long, default_value = "auto")]
    from: String,

    /// Whisper model name (tiny, base, small, medium, large-v3, large-v3-turbo) or a path.
    #[arg(long, default_value = models::DEFAULT_WHISPER)]
    model: String,

    /// Audio track to use, as listed by `verbacue tracks`.
    #[arg(long, default_value_t = 0)]
    audio_track: usize,

    /// List the audio tracks of the inputs and stop (the same as `verbacue tracks`).
    #[arg(long)]
    list_tracks: bool,

    /// Do not use voice activity detection to skip silence.
    #[arg(long)]
    no_vad: bool,

    /// Also write movie.<to>.mkv: all streams copied, plus the subtitles as soft tracks.
    #[arg(long)]
    embed: bool,

    /// Also write movie.<to>.mp4 with the subtitles burned into the picture.
    #[arg(long)]
    burn: bool,

    #[command(flatten)]
    translate: TranslateArgs,

    /// Show debug logs. Repeat for more detail.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Args, Clone)]
struct TranslateArgs {
    /// Target language: an ISO 639-1 code such as en, hu, de or ja.
    #[arg(long, env = "VERBACUE_TO", default_value = "en")]
    to: String,

    #[arg(long, value_enum, default_value_t = Backend::Local)]
    translator: Backend,

    /// GGUF model for the local translator: a known name or a path.
    #[arg(long, default_value = models::DEFAULT_LLM)]
    llm_model: String,

    /// Base URL of the server for --translator openai-compat.
    #[arg(
        long,
        env = "VERBACUE_ENDPOINT",
        default_value = "http://localhost:8080/v1"
    )]
    endpoint: String,

    /// Model name on the server; without it, verbacue uses the first model it lists.
    #[arg(long, env = "VERBACUE_ENDPOINT_MODEL")]
    endpoint_model: Option<String>,

    /// File with "term = translation" lines that the translator must follow.
    #[arg(long)]
    glossary: Option<PathBuf>,

    /// Form of address (for Hungarian: informal "te" or formal "ön").
    #[arg(long, value_enum, default_value_t = Register::Informal)]
    register: Register,

    #[arg(long, value_enum, default_value_t = Format::Srt)]
    format: Format,

    /// Output file. Only valid with one input.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Overwrite outputs that already exist.
    #[arg(long)]
    force: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Translate an existing SRT file.
    Translate {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Source language code. By default it comes from the file name (movie.en.srt).
        #[arg(long)]
        from: Option<String>,
        #[command(flatten)]
        args: TranslateArgs,
    },
    /// Burn an existing SRT file into a video, without transcription or translation.
    Burn {
        video: PathBuf,
        subtitles: PathBuf,
        /// Output file. By default `movie.<lang>.mp4`, with the language from the SRT file
        /// name (movie.hu.srt), or `movie.subtitled.mp4`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite the output file if it exists.
        #[arg(long)]
        force: bool,
    },
    /// List the language codes for --to and --from.
    Languages,
    /// List the audio tracks of a video.
    Tracks { video: PathBuf },
    /// Manage the model cache.
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },
}

#[derive(Subcommand)]
enum ModelsAction {
    /// List known models and show which are downloaded.
    List,
    /// Download a model into the cache.
    Download { name: String },
    /// Delete a model from the cache.
    Rm { name: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match cli.command {
        Some(Command::Tracks { video }) => print_tracks(&video),
        Some(Command::Languages) => {
            print_languages();
            Ok(())
        }
        Some(Command::Models { action }) => match action {
            ModelsAction::List => print_models(),
            ModelsAction::Download { name } => {
                let spec =
                    models::find(&name).with_context(|| format!("unknown model {name:?}"))?;
                let path = ensure_model(spec)?;
                out!("{}", path.display());
                Ok(())
            }
            ModelsAction::Rm { name } => {
                models::remove(&name)?;
                out!("removed {name}");
                Ok(())
            }
        },
        Some(Command::Translate { inputs, from, args }) => translate_files(&inputs, from, &args),
        Some(Command::Burn {
            video,
            subtitles,
            output,
            force,
        }) => burn_file(&video, &subtitles, output, force),
        None if cli.list_tracks => {
            for (i, video) in cli.inputs.iter().enumerate() {
                if cli.inputs.len() > 1 {
                    out!("{}{}", if i > 0 { "\n" } else { "" }, video.display());
                }
                print_tracks(video)?;
            }
            Ok(())
        }
        None => process_videos(&cli),
    }
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        // llama-cpp-2 logs llama.cpp and ggml messages under the target "llama-cpp-2".
        0 => "verbacue=info,whisper_rs=error,llama-cpp-2=error",
        1 => "verbacue=debug,whisper_rs=info,llama-cpp-2=warn",
        _ => "verbacue=trace,whisper_rs=debug,llama-cpp-2=debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

/// Prints the languages that Whisper can detect. The same codes work for --to; the
/// translation model also understands other codes and plain language names.
fn print_languages() {
    out!("code  language");
    for (code, name) in translate::languages() {
        out!("{code:<5} {name}");
    }
    out!("\nAny of these codes works with --from and --to (default --to en).");
    out!("--to also accepts other codes; the translation model decides if it knows them.");
}

fn print_tracks(video: &Path) -> Result<()> {
    let tracks = audio::list_tracks(video)?;
    if tracks.is_empty() {
        bail!("{} has no audio tracks", video.display());
    }
    out!("index  codec    lang  channels  rate     title");
    for t in tracks {
        out!(
            "{:<6} {:<8} {:<5} {:<9} {:<8} {}",
            t.index,
            t.codec,
            t.language.as_deref().unwrap_or("und"),
            t.channels,
            t.sample_rate,
            t.title.unwrap_or_default(),
        );
    }
    Ok(())
}

fn process_videos(cli: &Cli) -> Result<()> {
    let videos = collect_videos(&cli.inputs, &cli.translate.to)?;
    check_output_flag(&cli.translate, videos.len())?;

    let whisper = resolve_model(&cli.model, models::Kind::Whisper)?;
    let vad = if cli.no_vad {
        None
    } else {
        Some(ensure_model(
            models::find(models::VAD).context("VAD model missing")?,
        )?)
    };
    let language = (cli.from != "auto").then_some(cli.from.as_str());

    // The translator loads on first use, after the first transcription has finished
    // and Whisper has been unloaded.
    let mut translator: Option<Box<dyn Translator>> = None;
    let mut failed = 0;
    for (i, video) in videos.iter().enumerate() {
        if videos.len() > 1 {
            tracing::info!("[{}/{}] {}", i + 1, videos.len(), video.display());
        }
        let opts = transcribe::Options {
            model: &whisper,
            vad_model: vad.as_deref(),
            language,
        };
        if let Err(e) = process_video(video, cli, &opts, &mut translator) {
            if videos.len() == 1 {
                return Err(e);
            }
            tracing::error!("{}: {e:#}", video.display());
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} of {} files failed", videos.len());
    }
    Ok(())
}

fn process_video(
    video: &Path,
    cli: &Cli,
    opts: &transcribe::Options<'_>,
    translator: &mut Option<Box<dyn Translator>>,
) -> Result<()> {
    let args = &cli.translate;
    let target_out = output_path(video, &args.to, args);
    let embed_out = video.with_extension(format!("{}.mkv", args.to));
    let burn_out = video.with_extension(format!("{}.mp4", args.to));
    // The last file this run writes decides whether the video is already done.
    let done_marker = if cli.burn {
        Some(&burn_out)
    } else if cli.embed {
        Some(&embed_out)
    } else if args.translator != Backend::None {
        Some(&target_out)
    } else {
        None
    };
    if let Some(done) = done_marker
        && done.exists()
        && !args.force
    {
        tracing::info!("skipped {}: {} exists", video.display(), done.display());
        return Ok(());
    }

    if cli.burn {
        video::check_burn(video)?;
    }

    let bar = percent_bar("decoding");
    let pcm = audio::decode_pcm(video, cli.audio_track, &bar)?;
    bar.finish();
    let secs = pcm.len() as f64 / f64::from(audio::SAMPLE_RATE);
    tracing::info!("decoded {}", format_duration(secs));

    let bar = percent_bar("transcribing");
    let transcript = transcribe::transcribe(&pcm, opts, &bar)?;
    bar.finish();
    drop(pcm);
    tracing::info!(
        "transcribed {} cues, language {}",
        transcript.cues.len(),
        transcript.language
    );

    let source_lang = transcript.language.as_str();
    let source_cues = subtitle::layout_transcript(&transcript.cues);
    // With --translator none, -o names the original transcript, since nothing else is written.
    let source_out = match (&args.output, args.translator) {
        (Some(out), Backend::None) => out.clone(),
        _ => video.with_extension(format!("{source_lang}.{}", args.format.extension())),
    };
    write_output(&source_out, &source_cues, args.format)?;

    let target_cues = if args.translator == Backend::None {
        None
    } else if source_lang == args.to {
        tracing::warn!(
            "source language is already {}; no translation needed",
            args.to
        );
        None
    } else {
        let cues = run_translation(&source_cues, source_lang, args, translator)?;
        write_output(&target_out, &cues, args.format)?;
        Some(cues)
    };

    // The translated track comes first, so it becomes the default one.
    let mut tracks = Vec::new();
    if let Some(cues) = &target_cues {
        tracks.push(video::SubtitleTrack {
            language: &args.to,
            cues,
        });
    }
    tracks.push(video::SubtitleTrack {
        language: source_lang,
        cues: &source_cues,
    });

    if cli.embed {
        let bar = percent_bar("embedding");
        video::embed(video, &embed_out, &tracks, &bar)?;
        bar.finish();
        tracing::info!("wrote {}", embed_out.display());
    }
    if cli.burn {
        let bar = percent_bar("burning");
        let cues = tracks[0].cues;
        video::burn(video, &burn_out, cues, &bar)?;
        bar.finish();
        tracing::info!("wrote {}", burn_out.display());
    }
    Ok(())
}

fn translate_files(inputs: &[PathBuf], from: Option<String>, args: &TranslateArgs) -> Result<()> {
    check_output_flag(args, inputs.len())?;
    if args.translator == Backend::None {
        bail!("`verbacue translate` needs a translator other than none");
    }
    let mut translator = None;
    for input in inputs {
        let (base, lang) = split_lang_suffix(input);
        let source = from.clone().or(lang).unwrap_or_else(|| "auto".to_string());
        let out = output_path(&base, &args.to, args);
        if out.exists() && !args.force {
            tracing::info!("skipped {}: {} exists", input.display(), out.display());
            continue;
        }
        let text = fs::read_to_string(input)
            .with_context(|| format!("cannot read {}", input.display()))?;
        let cues = subtitle::parse_srt(&text)
            .with_context(|| format!("cannot parse {}", input.display()))?;
        let translated = run_translation(&cues, &source, args, &mut translator)?;
        write_output(&out, &translated, args.format)?;
    }
    Ok(())
}

fn burn_file(video: &Path, subtitles: &Path, output: Option<PathBuf>, force: bool) -> Result<()> {
    let out = output.unwrap_or_else(|| burn_output_path(video, subtitles));
    if out.exists() && !force {
        bail!("{} exists; use --force to overwrite it", out.display());
    }
    let text = fs::read_to_string(subtitles)
        .with_context(|| format!("cannot read {}", subtitles.display()))?;
    let cues = subtitle::parse_srt(&text)
        .with_context(|| format!("cannot parse {}", subtitles.display()))?;
    video::check_burn(video)?;
    let bar = percent_bar("burning");
    video::burn(video, &out, &cues, &bar)?;
    bar.finish();
    tracing::info!("wrote {}", out.display());
    Ok(())
}

/// `movie.mkv` with `movie.hu.srt` gives `movie.hu.mp4`. Without a language in the SRT
/// file name it gives `movie.subtitled.mp4`.
fn burn_output_path(video: &Path, subtitles: &Path) -> PathBuf {
    let lang = split_lang_suffix(subtitles).1;
    video.with_extension(format!("{}.mp4", lang.as_deref().unwrap_or("subtitled")))
}

/// Translates cues and lays out the result. The translator loads on first use.
fn run_translation(
    cues: &[Cue],
    source_lang: &str,
    args: &TranslateArgs,
    translator: &mut Option<Box<dyn Translator>>,
) -> Result<Vec<Cue>> {
    let translator = match translator {
        Some(t) => t,
        None => {
            // Download a missing local model here, so the download has a progress bar.
            let llm_model = if args.translator == Backend::Local {
                resolve_model(&args.llm_model, models::Kind::Llm)?
                    .display()
                    .to_string()
            } else {
                args.llm_model.clone()
            };
            translator.insert(translate::create(&translate::Config {
                backend: args.translator,
                target: args.to.clone(),
                register: args.register,
                glossary: match &args.glossary {
                    Some(path) => translate::load_glossary(path)?,
                    None => Vec::new(),
                },
                llm_model,
                endpoint: args.endpoint.clone(),
                endpoint_model: args.endpoint_model.clone(),
            })?)
        }
    };

    let bar = Bar::new(
        "translating",
        "{msg:>12} [{bar:30}] {pos}/{len} cues eta {eta}",
    );
    let translated = translate::translate_cues(translator.as_mut(), cues, source_lang, &bar)?;
    bar.finish();

    Ok(subtitle::layout_translation(&translated))
}

fn write_output(path: &Path, cues: &[Cue], format: Format) -> Result<()> {
    fs::write(path, subtitle::write(cues, format))
        .with_context(|| format!("cannot write {}", path.display()))?;
    tracing::info!("wrote {}", path.display());
    Ok(())
}

/// `dir/movie.mkv` becomes `dir/movie.<lang>.srt`. The `-o` flag replaces the path for
/// the target language.
fn output_path(input: &Path, lang: &str, args: &TranslateArgs) -> PathBuf {
    if lang == args.to
        && let Some(out) = &args.output
    {
        return out.clone();
    }
    input.with_extension(format!("{lang}.{}", args.format.extension()))
}

/// `movie.en.srt` gives (`movie.srt`, `Some("en")`). A file without a language part
/// gives (`movie.srt`, `None`).
fn split_lang_suffix(path: &Path) -> (PathBuf, Option<String>) {
    let stem = Path::new(path.file_stem().unwrap_or_default());
    match stem.extension().and_then(|e| e.to_str()) {
        Some(lang)
            if (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            (
                path.with_file_name(stem.with_extension("srt")),
                Some(lang.to_lowercase()),
            )
        }
        _ => (path.to_path_buf(), None),
    }
}

/// Expands directories into the media files they contain. A file counts as media when
/// FFmpeg can open it and it has an audio stream, so any format FFmpeg reads works.
/// Files that an earlier run wrote (movie.<to>.mkv, movie.<to>.mp4) are skipped.
fn collect_videos(inputs: &[PathBuf], target: &str) -> Result<Vec<PathBuf>> {
    let mut videos = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let mut found: Vec<PathBuf> = fs::read_dir(input)
                .with_context(|| format!("cannot read {}", input.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && !is_own_output(p, target) && has_audio(p))
                .collect();
            found.sort();
            videos.extend(found);
        } else if input.is_file() {
            videos.push(input.clone());
        } else {
            bail!("{} does not exist", input.display());
        }
    }
    if videos.is_empty() {
        bail!("no media files with audio found");
    }
    Ok(videos)
}

fn has_audio(path: &Path) -> bool {
    audio::list_tracks(path).is_ok_and(|t| !t.is_empty())
}

fn is_own_output(path: &Path, target: &str) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    matches!(ext, "srt" | "vtt")
        || (matches!(ext, "mkv" | "mp4") && split_lang_suffix(path).1.as_deref() == Some(target))
}

fn check_output_flag(args: &TranslateArgs, inputs: usize) -> Result<()> {
    if args.output.is_some() && inputs > 1 {
        bail!("-o/--output works only with one input file");
    }
    Ok(())
}

/// A progress bar on stderr that the library can update.
struct Bar(ProgressBar);

impl Bar {
    fn new(message: &'static str, template: &str) -> Self {
        let bar = ProgressBar::new(0).with_style(
            ProgressStyle::with_template(template)
                .expect("valid template")
                .progress_chars("=> "),
        );
        bar.set_message(message);
        bar.enable_steady_tick(Duration::from_millis(250));
        Self(bar)
    }

    fn finish(&self) {
        self.0.finish_and_clear();
    }
}

impl verbacue::Progress for Bar {
    fn update(&self, done: u64, total: u64) {
        self.0.set_length(total);
        self.0.set_position(done);
    }
}

fn percent_bar(message: &'static str) -> Bar {
    Bar::new(
        message,
        "{msg:>12} [{bar:30}] {pos:>3}% elapsed {elapsed} eta {eta}",
    )
}

fn download_bar(name: &'static str) -> Bar {
    Bar::new(
        name,
        "{msg:>12} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec} eta {eta}",
    )
}

/// Returns the cached path of a model, and downloads it with a progress bar first if
/// it is not there yet.
fn ensure_model(spec: &'static models::ModelSpec) -> Result<PathBuf> {
    let bar = download_bar(spec.name);
    let path = models::ensure(spec, &bar)?;
    bar.finish();
    Ok(path)
}

/// Like [`models::resolve`], with a download progress bar.
fn resolve_model(value: &str, kind: models::Kind) -> Result<PathBuf> {
    match models::find(value) {
        Some(spec) if spec.kind == kind && !Path::new(value).is_file() => ensure_model(spec),
        _ => Ok(models::resolve(value, kind, &())?),
    }
}

/// Prints all known models and shows which ones are in the cache.
fn print_models() -> Result<()> {
    out!("cache: {}", models::cache_dir()?.display());
    for m in models::known() {
        let mark = if models::cached_path(m)?.is_file() {
            "downloaded"
        } else {
            ""
        };
        out!(
            "  {:<16} {:<8} {:>9}  {mark}",
            m.name,
            m.kind.to_string(),
            models::human_size(m.size)
        );
    }
    Ok(())
}

fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    format!("{}:{:02}:{:02} of audio", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_language_suffix() {
        assert_eq!(
            split_lang_suffix(Path::new("/x/movie.en.srt")),
            (PathBuf::from("/x/movie.srt"), Some("en".to_string()))
        );
        assert_eq!(
            split_lang_suffix(Path::new("movie.srt")),
            (PathBuf::from("movie.srt"), None)
        );
        assert_eq!(
            split_lang_suffix(Path::new("Show.S01E02.srt")),
            (PathBuf::from("Show.S01E02.srt"), None)
        );
    }

    #[test]
    fn burn_output_takes_the_language_from_the_srt_name() {
        assert_eq!(
            burn_output_path(Path::new("/x/movie.mkv"), Path::new("/y/movie.hu.srt")),
            PathBuf::from("/x/movie.hu.mp4")
        );
        assert_eq!(
            burn_output_path(Path::new("movie.mp4"), Path::new("fixed.srt")),
            PathBuf::from("movie.subtitled.mp4")
        );
    }
}
