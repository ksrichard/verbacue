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

#[cfg(windows)]
mod console;
mod launch;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clap::{Args, CommandFactory as _, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tracing_subscriber::EnvFilter;
use verbacue::{
    audio, models,
    pipeline::{Event, Job, Pipeline, Sink, Step, Translation},
    subtitle::{self, Cue, Format},
    translate::{self, Backend, Register, Translator},
    video,
};

#[derive(Parser)]
#[command(
    name = "verbacue",
    version,
    about = "Generate translated subtitles for videos",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Video files, or directories that contain videos.
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
    /// Open the desktop app. `verbacue` with no arguments also opens it.
    Gui,
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
    let args = std::env::args_os().len().saturating_sub(1);
    let gui_command = std::env::args_os().nth(1).is_some_and(|a| a == "gui");
    match launch::mode(
        args,
        gui_command,
        launch::has_display(),
        cfg!(feature = "gui"),
    ) {
        launch::Mode::Gui => return open_gui(),
        launch::Mode::Help => {
            print_help();
            return Ok(());
        }
        launch::Mode::Cli => {}
    }
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match cli.command {
        Some(Command::Gui) => bail!("this build has no GUI; see verbacue --help"),
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
        None if cli.inputs.is_empty() => {
            print_help();
            Ok(())
        }
        None => process_videos(&cli),
    }
}

/// Prints the help. A closed pipe (`verbacue | head`) is not an error.
fn print_help() {
    let _ = Cli::command().print_help();
}

#[cfg(feature = "gui")]
fn open_gui() -> Result<()> {
    init_logging(0);
    #[cfg(windows)]
    console::detach_if_alone();
    verbacue_gui::run()
        .map_err(|e| anyhow::anyhow!("{e}; use verbacue --help for the command line"))
}

#[cfg(not(feature = "gui"))]
fn open_gui() -> Result<()> {
    bail!("this build has no GUI; see verbacue --help")
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

    // One pipeline for all videos keeps the translator loaded. It loads after the first
    // transcription, when Whisper has been unloaded.
    let mut pipeline = Pipeline::new();
    let mut failed = 0;
    for (i, video) in videos.iter().enumerate() {
        if videos.len() > 1 {
            tracing::info!("[{}/{}] {}", i + 1, videos.len(), video.display());
        }
        let sink = BarSink::new(video);
        let result = pipeline.run(&job_for(video, cli), &sink);
        sink.finish();
        if let Err(e) = result {
            if videos.len() == 1 {
                return Err(e.into());
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

fn job_for(video: &Path, cli: &Cli) -> Job {
    let args = &cli.translate;
    Job {
        video: video.to_path_buf(),
        audio_track: cli.audio_track,
        whisper_model: cli.model.clone(),
        vad: !cli.no_vad,
        language: (cli.from != "auto").then(|| cli.from.clone()),
        target: args.to.clone(),
        translation: (args.translator != Backend::None).then(|| Translation {
            backend: args.translator,
            register: args.register,
            glossary: args.glossary.clone(),
            llm_model: args.llm_model.clone(),
            endpoint: args.endpoint.clone(),
            endpoint_model: args.endpoint_model.clone(),
        }),
        format: args.format,
        output: args.output.clone(),
        out_dir: None,
        embed: cli.embed,
        burn: cli.burn,
        force: args.force,
    }
}

/// Shows the pipeline steps as progress bars on stderr.
struct BarSink<'a> {
    video: &'a Path,
    bar: Mutex<Option<Bar>>,
}

impl<'a> BarSink<'a> {
    fn new(video: &'a Path) -> Self {
        Self {
            video,
            bar: Mutex::new(None),
        }
    }

    fn finish(&self) {
        if let Some(bar) = self.bar.lock().ok().and_then(|mut b| b.take()) {
            bar.finish();
        }
    }
}

impl Sink for BarSink<'_> {
    fn event(&self, event: Event<'_>) {
        match event {
            Event::Step(step) => {
                self.finish();
                let bar = match step {
                    Step::Download(name) => download_bar(name),
                    Step::Decode => percent_bar("decoding"),
                    Step::Transcribe => percent_bar("transcribing"),
                    Step::Translate => Bar::new(
                        "translating",
                        "{msg:>12} [{bar:30}] {pos}/{len} cues eta {eta}",
                    ),
                    Step::Embed => percent_bar("embedding"),
                    Step::Burn => percent_bar("burning"),
                    Step::Save => return,
                };
                if let Ok(mut current) = self.bar.lock() {
                    *current = Some(bar);
                }
            }
            Event::Progress { done, total } => {
                if let Some(bar) = self.bar.lock().ok().as_ref().and_then(|b| b.as_ref()) {
                    verbacue::Progress::update(bar, done, total);
                }
            }
            Event::Wrote(path) => {
                self.finish();
                tracing::info!("wrote {}", path.display());
            }
            Event::Skipped(path) => {
                tracing::info!(
                    "skipped {}: {} exists",
                    self.video.display(),
                    path.display()
                );
            }
            Event::Note(text) => tracing::warn!("{text}"),
            Event::Cue(_) => {}
        }
    }
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
