//! The app state, its messages, and the screen it shows.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock, atomic::Ordering},
    time::Instant,
};

use iced::{
    Element, Length, Subscription, Task,
    widget::{canvas, column, container, row, rule, stack, text},
    window,
};
use verbacue::{
    ErrorKind, models,
    pipeline::{self, Job, Outcome, Translation},
    video::{MediaInfo, PreviewFrame},
};

use crate::{
    queue::{Item, Status},
    screens::{
        self,
        choose::{Choose, OutputMode},
        convert::Convert,
        pick::Pick,
        settings::SettingChange,
    },
    settings::{self, Recent, Settings},
    theme,
    widgets::{backdrop::Backdrop, nav},
    worker::{Failure, Update, Worker},
};

/// The screens of the app, in the order of the main flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Pick,
    Choose,
    Convert,
    Done,
    Settings,
}

/// The video the user picked and what to make from it.
#[derive(Debug, Clone)]
pub struct Selection {
    pub video: PathBuf,
    pub info: MediaInfo,
    /// Burned-in MP4 (true) or only subtitle files (false).
    pub burn: bool,
    /// A language code, or [`KEEP`] for no translation.
    pub target: String,
    pub track: usize,
    /// What the user wrote about this video, for the translator.
    pub notes: String,
}

/// How long iced may still report sizes with the old scale after a change.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// The target value for "Keep the original language".
pub const KEEP: &str = "";

/// The language code in output file names when the video keeps its language.
const KEEP_NAME: &str = "subtitled";

/// Everything that can happen in the app.
#[derive(Debug, Clone)]
pub enum Message {
    Show(Screen),
    /// A frame of the animations.
    Tick(Instant),
    /// The window size as iced reports it, after the app scale.
    Resized(iced::Size),
    /// The window size, read once the last scale change is applied.
    SizeChecked(iced::Size),
    /// Opens the file dialog; it takes several files.
    PickFile,
    FileChosen(PathBuf),
    /// A file is dragged over the window (true) or away from it (false).
    Hover(bool),
    MediaChecked(Result<(PathBuf, MediaInfo), String>),
    OpenRecent(usize),
    PreviewLoaded(PathBuf, Vec<PreviewFrame>),
    /// Shows this queue item on the Preview screen.
    Select(usize),
    /// Removes this queue item, unless it runs.
    Remove(usize),
    /// Copies the shown video's options to every waiting video.
    ApplyToAll,
    Mode(OutputMode),
    Target(String),
    Track(usize),
    /// The notes about the shown video.
    Notes(String),
    PlayVideo,
    Start,
    ConfirmReplace(bool),
    Pipeline(Update),
    /// An edit in the film notes editor.
    ReviewEdit(iced::widget::text_editor::Action),
    /// The user checked the film notes; the translation starts.
    ReviewDone,
    Cancel,
    CloseRequested,
    ConfirmClose(bool),
    ToggleDetails,
    /// Opens the main file of this queue item.
    OpenFile(usize),
    ShowInFolder(usize),
    Again,
    Setting(SettingChange),
    ChooseGlossary,
    ChooseOutDir,
    OpenConfigFolder,
    DownloadModel(&'static str),
    /// (model, bytes done, bytes total)
    ModelProgress(&'static str, u64, u64),
    ModelDone(&'static str, Result<(), String>),
    RemoveModel(&'static str),
    #[cfg(debug_assertions)]
    Screenshot(iced::window::Screenshot),
    #[cfg(debug_assertions)]
    RecFrame(iced::window::Screenshot),
}

/// The whole state of the app.
pub struct App {
    pub screen: Screen,
    pub settings: Settings,
    /// A notice about the settings file, shown once.
    pub notice: Option<String>,
    pub pick: Pick,
    /// The videos, in order.
    pub queue: Vec<Item>,
    /// The queue item that the Preview screen shows.
    pub current: usize,
    /// The id of the running item and its job.
    running: Option<(u64, Job)>,
    /// The ids of the items that the current run started, for the end screen.
    run: Vec<u64>,
    /// Overwrite existing outputs in the current run.
    force: bool,
    next_id: u64,
    /// The options of recent entries, for the files being checked.
    recent_options: HashMap<PathBuf, (String, bool)>,
    /// The id of each file being checked, given in the order the user chose them.
    chosen: HashMap<PathBuf, u64>,
    pub choose: Choose,
    pub convert: Convert,
    /// Running model downloads: name → (bytes done, bytes total).
    pub downloads: HashMap<&'static str, (u64, u64)>,
    /// When the silence switch last changed, for its animation.
    pub vad_changed: Instant,
    /// When the film notes switch last changed, for its animation.
    pub film_notes_changed: Instant,
    /// How much larger than the mockups the whole interface draws.
    pub scale: f32,
    /// When the scale last changed.
    scale_changed: Option<Instant>,
    /// The pipeline thread; it starts with the first job.
    worker: Arc<OnceLock<Worker>>,
    /// Where `gui.toml` lives; `None` does not save (tests).
    config: Option<PathBuf>,
    /// When the current screen opened, and the time of the last frame.
    opened: Instant,
    now: Instant,
    logo: iced::widget::image::Handle,
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let (mut settings, notice) = verbacue::config_dir()
            .map(|dir| settings::load(&dir))
            .unwrap_or_default();
        settings.drop_missing();
        let mut app = Self::with_settings(settings, notice);
        app.config = verbacue::config_dir();
        #[cfg(debug_assertions)]
        if crate::devshot::rec_dir().is_some() {
            app.config = None;
            if let Some(model) = crate::devshot::model() {
                app.settings.whisper_model = model;
            }
            return (app, crate::devshot::rec_begin());
        }
        #[cfg(debug_assertions)]
        if crate::devshot::target().is_some() {
            app.config = None;
            if std::env::var_os("VERBACUE_GUI_SHOT_DEFAULTS").is_some() {
                app.settings = Settings::default();
            }
            if let Some(model) = crate::devshot::model() {
                app.settings.whisper_model = model;
            }
            let setup = crate::devshot::setup().into_iter().map(Task::done);
            return (
                app,
                Task::batch(setup.chain([crate::devshot::start(), crate::devshot::take()])),
            );
        }
        (app, Task::none())
    }

    pub fn with_settings(settings: Settings, notice: Option<String>) -> Self {
        let now = Instant::now();
        Self {
            screen: Screen::Pick,
            settings,
            notice,
            pick: Pick::default(),
            queue: Vec::new(),
            current: 0,
            running: None,
            run: Vec::new(),
            force: false,
            next_id: 0,
            recent_options: HashMap::new(),
            chosen: HashMap::new(),
            choose: Choose::default(),
            convert: Convert::for_job(&Job::new(PathBuf::new(), "en"), String::new()),
            downloads: HashMap::new(),
            vad_changed: now,
            film_notes_changed: now,
            scale: 1.0,
            scale_changed: None,
            worker: Arc::new(OnceLock::new()),
            config: None,
            opened: now,
            now,
            logo: nav::logo(),
        }
    }

    /// Sets the scale for a window of `size` (as iced reports it, divided by the
    /// current scale). After a change it reads the size again when iced has applied
    /// the new scale.
    fn rescale(&mut self, size: iced::Size) -> Task<Message> {
        let scale = ui_scale(size * self.scale);
        if scale == self.scale {
            self.scale_changed = None;
            return Task::none();
        }
        self.scale = scale;
        self.scale_changed = Some(Instant::now());
        // The sleep is made inside the task, so `update` works without a runtime.
        Task::future(async { tokio::time::sleep(SETTLE).await })
            .then(|()| window::latest())
            .and_then(window::size)
            .map(Message::SizeChecked)
    }

    /// The window title: the app name, and during a conversion the file and the
    /// progress of the whole job.
    pub fn title(&self) -> String {
        let video = self.running_job().and_then(|job| job.video.file_name());
        match video {
            Some(name) if self.is_running() => {
                let mut title = format!(
                    "VERBACUE — {} — {:.0} %",
                    name.to_string_lossy(),
                    self.convert.overall() * 100.0
                );
                let (position, total) = self.run_position();
                if total > 1 {
                    title.push_str(&format!(" ({position}/{total})"));
                }
                title
            }
            _ => "VERBACUE".to_string(),
        }
    }

    /// The video that the Preview screen shows.
    pub fn selection(&self) -> Option<&Selection> {
        self.queue.get(self.current).map(|item| &item.selection)
    }

    fn selection_mut(&mut self) -> Option<&mut Selection> {
        self.queue
            .get_mut(self.current)
            .map(|item| &mut item.selection)
    }

    /// The job that runs now.
    pub fn running_job(&self) -> Option<&Job> {
        self.running.as_ref().map(|(_, job)| job)
    }

    /// (position of the running video, videos in this run), both from 1.
    pub fn run_position(&self) -> (usize, usize) {
        let running = self.running.as_ref().map(|(id, _)| *id);
        let position = self
            .run
            .iter()
            .position(|id| Some(*id) == running)
            .map_or(0, |i| i + 1);
        let waiting = self
            .queue
            .iter()
            .filter(|item| item.status == Status::Waiting && !self.run.contains(&item.id))
            .count();
        (position, self.run.len() + waiting)
    }

    /// The items of the current or last run, with their queue index.
    pub fn run_items(&self) -> Vec<(usize, &Item)> {
        self.run
            .iter()
            .filter_map(|id| self.index_of(*id).map(|i| (i, &self.queue[i])))
            .collect()
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.queue.iter().position(|item| item.id == id)
    }

    /// Seconds since the current screen opened, for the animations.
    pub fn seconds(&self) -> f32 {
        self.now.duration_since(self.opened).as_secs_f32()
    }

    fn show(&mut self, screen: Screen) {
        if self.screen != screen {
            self.screen = screen;
            self.opened = Instant::now();
            self.now = self.opened;
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Show(screen) => {
                self.show(screen);
                Task::none()
            }
            Message::Tick(now) => {
                self.now = now;
                Task::none()
            }
            Message::Resized(size) => {
                // A resize event right after a scale change can still use the old
                // scale, so it waits for the check after the settle time.
                if self.scale_changed.is_some_and(|t| t.elapsed() < SETTLE) {
                    return Task::none();
                }
                self.rescale(size)
            }
            Message::SizeChecked(size) => self.rescale(size),
            Message::PickFile => Task::future(async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a video")
                    .add_filter(
                        "Video",
                        &[
                            "mkv", "mp4", "avi", "webm", "mov", "ts", "m4v", "wmv", "flv",
                        ],
                    )
                    .add_filter("All files", &["*"])
                    .pick_files()
                    .await
            })
            .and_then(|files| {
                Task::batch(
                    files
                        .into_iter()
                        .map(|f| Task::done(Message::FileChosen(f.path().to_path_buf()))),
                )
            }),
            Message::FileChosen(path) => self.check(path, None),
            Message::Hover(hovering) => {
                self.pick.hovering = hovering;
                Task::none()
            }
            Message::OpenRecent(i) => match self.settings.recent.get(i).cloned() {
                Some(r) => self.check(r.path, Some((r.target, r.burn))),
                None => Task::none(),
            },
            Message::MediaChecked(result) => {
                self.pick.checking = None;
                match result {
                    Ok((video, info)) => return self.add(video, info),
                    Err(error) if self.screen == Screen::Pick => self.pick.error = Some(error),
                    Err(error) => self.notice = Some(error),
                }
                Task::none()
            }
            Message::PreviewLoaded(video, frames) => {
                let handles: Vec<_> = frames
                    .into_iter()
                    .map(|f| iced::widget::image::Handle::from_rgba(f.width, f.height, f.rgba))
                    .collect();
                if let Some(item) = self
                    .queue
                    .iter_mut()
                    .find(|item| item.selection.video == video && item.frames.is_empty())
                {
                    item.frames = handles;
                }
                Task::none()
            }
            Message::Select(index) => {
                if index < self.queue.len() {
                    self.current = index;
                    self.choose.confirm_replace = false;
                    self.show(Screen::Choose);
                }
                Task::none()
            }
            Message::Remove(index) => {
                self.remove(index);
                Task::none()
            }
            Message::ApplyToAll => {
                if let Some(shown) = self.selection().cloned() {
                    for item in &mut self.queue {
                        if item.status == Status::Waiting {
                            item.selection.burn = shown.burn;
                            item.selection.target.clone_from(&shown.target);
                            if shown.track < item.selection.info.audio.len() {
                                item.selection.track = shown.track;
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::Mode(mode) => {
                if let Some(s) = self.selection_mut() {
                    s.burn = mode == OutputMode::Burn;
                    self.choose.mode_changed = Instant::now();
                }
                Task::none()
            }
            Message::Target(code) => {
                if let Some(s) = self.selection_mut() {
                    s.target = code;
                }
                Task::none()
            }
            Message::Track(index) => {
                if let Some(s) = self.selection_mut() {
                    s.track = index;
                }
                Task::none()
            }
            Message::Notes(notes) => {
                if let Some(s) = self.selection_mut() {
                    s.notes = notes;
                }
                Task::none()
            }
            Message::PlayVideo => {
                if let Some(s) = self.selection() {
                    crate::system::open(&s.video);
                }
                Task::none()
            }
            Message::Start => {
                // "Try again" after a failure also uses Start.
                for item in &mut self.queue {
                    if matches!(item.status, Status::Failed(_)) {
                        item.status = Status::Waiting;
                    }
                }
                let exists = self.queue.iter().enumerate().any(|(i, item)| {
                    item.status == Status::Waiting
                        && self
                            .job_for(i)
                            .and_then(|job| pipeline::final_output(&job))
                            .is_some_and(|p| p.exists())
                });
                if exists {
                    self.choose.confirm_replace = true;
                    self.show(Screen::Choose);
                    return Task::none();
                }
                self.begin(false)
            }
            Message::ConfirmReplace(replace) => {
                self.choose.confirm_replace = false;
                if replace {
                    self.begin(true)
                } else {
                    Task::none()
                }
            }
            Message::Pipeline(Update::Finished(result)) => self.finished(result),
            Message::Pipeline(Update::Review(notes)) => {
                if self.settings.review_notes {
                    self.convert.review =
                        Some(iced::widget::text_editor::Content::with_text(&notes));
                } else if let Some(reply) = &self.convert.reply {
                    let _ = reply.send(notes);
                }
                Task::none()
            }
            Message::ReviewEdit(action) => {
                if let Some(content) = &mut self.convert.review {
                    content.perform(action);
                }
                Task::none()
            }
            Message::ReviewDone => {
                if let (Some(content), Some(reply)) =
                    (self.convert.review.take(), &self.convert.reply)
                {
                    let _ = reply.send(content.text());
                }
                Task::none()
            }
            Message::Pipeline(update) => {
                self.convert.apply(update);
                Task::none()
            }
            Message::Cancel => {
                self.convert.cancel.store(true, Ordering::Relaxed);
                self.convert.stopping = true;
                Task::none()
            }
            Message::CloseRequested => {
                if self.is_running() {
                    self.convert.confirm_close = true;
                    Task::none()
                } else {
                    window::latest().and_then(window::close)
                }
            }
            Message::ConfirmClose(close) => {
                self.convert.confirm_close = false;
                if close {
                    self.convert.close_after = true;
                    return self.update(Message::Cancel);
                }
                Task::none()
            }
            Message::OpenFile(index) => {
                if let Some(file) = self
                    .queue
                    .get(index)
                    .and_then(Item::outcome)
                    .and_then(screens::done::main_file)
                {
                    crate::system::open(file);
                }
                Task::none()
            }
            Message::ShowInFolder(index) => {
                if let Some(file) = self
                    .queue
                    .get(index)
                    .and_then(Item::outcome)
                    .and_then(screens::done::main_file)
                {
                    crate::system::show_in_folder(file);
                }
                Task::none()
            }
            Message::Again => {
                self.queue.clear();
                self.current = 0;
                self.run.clear();
                self.pick = Pick::default();
                self.show(Screen::Pick);
                Task::none()
            }
            Message::Setting(change) => {
                if matches!(change, SettingChange::Vad(_)) {
                    self.vad_changed = Instant::now();
                }
                if matches!(
                    change,
                    SettingChange::FilmNotes(_) | SettingChange::ReviewNotes(_)
                ) {
                    self.film_notes_changed = Instant::now();
                }
                change.apply(&mut self.settings);
                self.save_settings();
                Task::none()
            }
            Message::ChooseGlossary => Task::future(async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a glossary")
                    .add_filter("Text", &["txt"])
                    .pick_file()
                    .await
            })
            .and_then(|f| {
                Task::done(Message::Setting(SettingChange::Glossary(Some(
                    f.path().to_path_buf(),
                ))))
            }),
            Message::ChooseOutDir => Task::future(async {
                rfd::AsyncFileDialog::new()
                    .set_title("Save subtitles to")
                    .pick_folder()
                    .await
            })
            .and_then(|f| {
                Task::done(Message::Setting(SettingChange::OutDir(Some(
                    f.path().to_path_buf(),
                ))))
            }),
            Message::OpenConfigFolder => {
                if let Some(dir) = verbacue::config_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    crate::system::open(&dir);
                }
                Task::none()
            }
            Message::DownloadModel(name) => {
                self.downloads.insert(name, (0, 0));
                download(name)
            }
            Message::ModelProgress(name, done, total) => {
                self.downloads.insert(name, (done, total));
                Task::none()
            }
            Message::ModelDone(name, result) => {
                self.downloads.remove(name);
                if let Err(e) = result {
                    self.notice = Some(format!("Download of {name} failed: {e}"));
                }
                Task::none()
            }
            Message::RemoveModel(name) => {
                if let Err(e) = models::remove(name) {
                    self.notice = Some(format!("Cannot remove {name}: {e}"));
                }
                Task::none()
            }
            Message::ToggleDetails => {
                self.convert.show_details = !self.convert.show_details;
                Task::none()
            }
            #[cfg(debug_assertions)]
            Message::Screenshot(shot) => crate::devshot::save(&shot),
            #[cfg(debug_assertions)]
            Message::RecFrame(shot) => {
                let done = self.screen == Screen::Done && self.seconds() > 4.5;
                crate::devshot::rec_save(shot, done)
            }
        }
    }

    /// True while a job runs.
    fn is_running(&self) -> bool {
        self.screen == Screen::Convert && self.convert.error.is_none()
    }

    fn finished(&mut self, result: Result<Outcome, Failure>) -> Task<Message> {
        if self.convert.close_after {
            return window::latest().and_then(window::close);
        }
        let Some((id, job)) = self.running.take() else {
            return Task::none();
        };
        let Some(index) = self.index_of(id) else {
            return self.run_next();
        };
        match result {
            Ok(outcome) => {
                let s = self.queue[index].selection.clone();
                self.settings.remember(Recent {
                    path: s.video.clone(),
                    target: s.target.clone(),
                    burn: s.burn,
                });
                if s.target != KEEP {
                    self.settings.target = s.target.clone();
                }
                self.save_settings();
                let target = pipeline::output_path(&job, &job.target);
                let subtitles = Some(target)
                    .filter(|p| outcome.files.contains(p))
                    .or_else(|| {
                        outcome
                            .files
                            .iter()
                            .find(|p| p.extension().is_some_and(|e| e == "srt" || e == "vtt"))
                            .cloned()
                    });
                let lines = subtitles.map_or(0, |p| screens::done::count_cues(&p));
                self.queue[index].status = Status::Done(outcome, lines);
                self.run_next()
            }
            Err(failure) if failure.kind == ErrorKind::Cancelled => {
                self.queue[index].status = Status::Waiting;
                self.current = index;
                self.run.clear();
                self.show(Screen::Choose);
                Task::none()
            }
            Err(failure) => {
                tracing::warn!("{}", failure.message);
                self.queue[index].status = Status::Failed(failure.clone());
                // The only video of a run keeps the error card with "Try again".
                if self.run.len() == 1 && self.next_waiting().is_none() {
                    self.convert.error = Some(failure);
                    return Task::none();
                }
                self.run_next()
            }
        }
    }

    fn next_waiting(&self) -> Option<usize> {
        self.queue
            .iter()
            .position(|item| item.status == Status::Waiting)
    }

    /// Starts the queue: every waiting video, one after another.
    fn begin(&mut self, force: bool) -> Task<Message> {
        self.force = force;
        self.run.clear();
        self.run_next()
    }

    /// Starts the next waiting video, or shows the Done screen.
    fn run_next(&mut self) -> Task<Message> {
        let Some(index) = self.next_waiting() else {
            self.running = None;
            self.show(Screen::Done);
            return Task::none();
        };
        let Some(mut job) = self.job_for(index) else {
            return Task::none();
        };
        job.force = self.force;
        let item = &mut self.queue[index];
        item.status = Status::Running;
        self.run.push(item.id);
        self.running = Some((item.id, job.clone()));
        self.start(job, index)
    }

    /// Adds a checked video to the end of the queue.
    fn add(&mut self, video: PathBuf, info: MediaInfo) -> Task<Message> {
        let (target, burn) = self
            .recent_options
            .remove(&video)
            .unwrap_or_else(|| (self.settings.target.clone(), true));
        let id = self.chosen.remove(&video).unwrap_or_else(|| {
            self.next_id += 1;
            self.next_id - 1
        });
        // Checks can end in any order; the queue keeps the order of choosing.
        let at = self
            .queue
            .iter()
            .position(|item| item.status == Status::Waiting && item.id > id)
            .unwrap_or(self.queue.len());
        if at <= self.current && !self.queue.is_empty() {
            self.current += 1;
        }
        self.queue.insert(
            at,
            Item {
                id,
                selection: Selection {
                    video: video.clone(),
                    info,
                    burn,
                    target,
                    track: 0,
                    notes: String::new(),
                },
                frames: Vec::new(),
                status: Status::Waiting,
            },
        );
        self.pick.error = None;
        if matches!(self.screen, Screen::Pick | Screen::Done) {
            self.current = at;
            self.choose = Choose::default();
            self.show(Screen::Choose);
        }
        load_preview(video)
    }

    /// Removes a queue item that does not run.
    fn remove(&mut self, index: usize) {
        if self
            .queue
            .get(index)
            .is_none_or(|item| item.status == Status::Running)
        {
            return;
        }
        self.queue.remove(index);
        if self.current > index || self.current >= self.queue.len() {
            self.current = self.current.saturating_sub(1);
        }
        if self.queue.is_empty() && self.screen == Screen::Choose {
            self.show(Screen::Pick);
        }
    }

    /// Writes `gui.toml`; a failure shows as a notice.
    pub fn save_settings(&mut self) {
        if let Some(dir) = &self.config
            && let Err(e) = settings::save(dir, &self.settings)
        {
            self.notice = Some(e.to_string());
        }
    }

    /// The job for the shown video and the settings.
    #[cfg(test)]
    pub fn job(&self) -> Option<Job> {
        self.job_for(self.current)
    }

    /// The job for a queue item and the settings.
    pub fn job_for(&self, index: usize) -> Option<Job> {
        let s = &self.queue.get(index)?.selection;
        let settings = &self.settings;
        let keep = s.target == KEEP;
        let mut job = Job::new(
            s.video.clone(),
            if keep { KEEP_NAME } else { s.target.as_str() },
        );
        job.audio_track = s.track;
        job.whisper_model = settings.whisper_model.clone();
        job.vad = settings.vad;
        job.language = settings.language.clone();
        job.format = settings.format;
        job.out_dir = settings.out_dir.clone();
        job.burn = s.burn;
        job.translation = (!keep && settings.translator != verbacue::translate::Backend::None)
            .then(|| Translation {
                backend: settings.translator,
                register: settings.register,
                glossary: settings.glossary.clone(),
                llm_model: settings.llm_model.clone(),
                endpoint: settings.endpoint.clone(),
                endpoint_model: settings.endpoint_model.clone(),
                notes: s.notes.clone(),
                film_notes: settings.film_notes,
            });
        Some(job)
    }

    /// Opens the Converting screen and runs `job` on the pipeline thread.
    fn start(&mut self, job: Job, index: usize) -> Task<Message> {
        self.convert = Convert::for_job(&job, self.summary(index));
        self.show(Screen::Convert);
        let worker = Arc::clone(&self.worker);
        let cancel = Arc::clone(&self.convert.cancel);
        let (reply, replies) = std::sync::mpsc::channel();
        self.convert.reply = Some(reply);
        // The thread starts only when the task runs, so tests never start the pipeline.
        let updates = iced::futures::stream::once(async move {
            worker.get_or_init(Worker::new).run(job, cancel, replies)
        });
        Task::run(
            iced::futures::StreamExt::flatten(updates),
            Message::Pipeline,
        )
    }

    /// "movie.mkv · Hungarian · burned in"
    fn summary(&self, index: usize) -> String {
        let Some(s) = self.queue.get(index).map(|item| &item.selection) else {
            return String::new();
        };
        let name = s
            .video
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let language = if s.target == KEEP {
            "original language".to_string()
        } else {
            verbacue::translate::language_name(&s.target).to_string()
        };
        let mode = if s.burn { "burned in" } else { "subtitle file" };
        format!("{name} · {language} · {mode}")
    }

    /// Reads the facts of `path` on the blocking thread pool.
    fn check(&mut self, path: PathBuf, options: Option<(String, bool)>) -> Task<Message> {
        self.pick.hovering = false;
        self.pick.error = None;
        self.pick.checking = Some(path.clone());
        self.chosen.insert(path.clone(), self.next_id);
        self.next_id += 1;
        if let Some(options) = options {
            self.recent_options.insert(path.clone(), options);
        }
        Task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                verbacue::video::media_info(&path)
                    .map(|info| (path, info))
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            Message::MediaChecked(result)
        })
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let drops = window::events().filter_map(|(_, event)| match event {
            window::Event::FileHovered(_) => Some(Message::Hover(true)),
            window::Event::FilesHoveredLeft => Some(Message::Hover(false)),
            window::Event::FileDropped(path) => Some(Message::FileChosen(path)),
            _ => None,
        });
        let closes = window::close_requests().map(|_| Message::CloseRequested);
        let resizes = window::resize_events().map(|(_, size)| Message::Resized(size));
        let animated = matches!(
            self.screen,
            Screen::Pick | Screen::Choose | Screen::Convert | Screen::Done
        ) || (self.screen == Screen::Settings
            && (self.vad_changed.elapsed().as_secs_f32() < 0.5
                || self.film_notes_changed.elapsed().as_secs_f32() < 0.5
                || !self.downloads.is_empty()));
        if animated {
            Subscription::batch([drops, closes, resizes, window::frames().map(Message::Tick)])
        } else {
            Subscription::batch([drops, closes, resizes])
        }
    }

    /// (downloaded, needed) for the models that the current settings use.
    fn model_count(&self) -> (usize, usize) {
        let mut needed = vec![self.settings.whisper_model.as_str()];
        if self.settings.vad {
            needed.push(models::VAD);
        }
        if self.settings.translator == verbacue::translate::Backend::Local {
            needed.push(self.settings.llm_model.as_str());
        }
        let known: Vec<_> = needed.iter().filter_map(|n| models::find(n)).collect();
        let downloaded = known
            .iter()
            .filter(|m| models::cached_path(m).is_ok_and(|p| p.is_file()))
            .count();
        (downloaded, known.len())
    }

    pub fn view(&self) -> Element<'_, Message> {
        let page: Element<'_, Message> = match self.screen {
            Screen::Pick => screens::pick::view(self),
            Screen::Choose => screens::choose::view(self),
            Screen::Convert => screens::convert::view_screen(self),
            Screen::Done => screens::done::view(self),
            Screen::Settings => screens::settings::view(self),
        };
        let page = match &self.notice {
            Some(notice) => column![
                container(text(notice).size(14).color(theme::ACCENT_TEXT))
                    .padding([12, 16])
                    .width(Length::Fill)
                    .style(theme::card(14.0)),
                page
            ]
            .padding([16, 24])
            .into(),
            None => page,
        };
        let content = row![
            nav::view(self.screen, self.model_count(), &self.logo),
            rule::vertical(1).style(theme::divider),
            container(page).width(Length::Fill).height(Length::Fill),
        ];
        // Each page of the mockups has its own glow.
        let glow = match self.screen {
            Screen::Convert => Backdrop {
                center: (0.5, 1.1),
                glow: iced::Color::from_rgb8(0x3a, 0x1c, 0x10),
            },
            Screen::Done => Backdrop {
                center: (0.5, 0.0),
                glow: iced::Color::from_rgb8(0x24, 0x30, 0x1f),
            },
            _ => Backdrop {
                center: (0.7, -0.1),
                glow: theme::GLOW,
            },
        };
        stack![
            canvas(glow).width(Length::Fill).height(Length::Fill),
            content
        ]
        .into()
    }
}

/// The interface scale for a window of `size` (without the app scale). The mockups
/// are drawn for 1280 x 800; a larger window draws everything larger, up to 2.5 times,
/// in steps of 0.05 so that small resizes do not change it.
pub fn ui_scale(size: iced::Size) -> f32 {
    let fit = (size.width / 1280.0).min(size.height / 800.0);
    ((fit * 20.0).round() / 20.0).clamp(1.0, 2.5)
}

/// Decodes the preview frames of `video` on the blocking thread pool.
fn load_preview(video: PathBuf) -> Task<Message> {
    Task::future(async move {
        let source = video.clone();
        let frames = tokio::task::spawn_blocking(move || {
            verbacue::video::preview_frames(&source, 6, 960).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        Message::PreviewLoaded(video, frames)
    })
}

/// Downloads a known model on the blocking thread pool and reports its progress.
fn download(name: &'static str) -> Task<Message> {
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let result = match models::find(name) {
            Some(spec) => {
                let report = |done: u64, total: u64| {
                    let _ = sender.send(Message::ModelProgress(name, done, total));
                };
                models::ensure(spec, &report)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            None => Err(format!("unknown model {name}")),
        };
        let _ = sender.send(Message::ModelDone(name, result));
    });
    Task::stream(iced::futures::stream::poll_fn(move |cx| {
        updates.poll_recv(cx)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        screens::{choose::OutputMode, settings::SettingChange},
        settings::Recent,
        worker::{Failure, Update, friendly},
    };
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use verbacue::{
        ErrorKind,
        pipeline::{Outcome, Step},
        subtitle::Cue,
    };
    use verbacue::{audio::AudioTrack, video::MediaInfo};

    fn app() -> App {
        App::with_settings(Settings::default(), None)
    }

    fn info() -> MediaInfo {
        MediaInfo {
            container: "matroska,webm".into(),
            duration_ms: 6_128_000,
            width: 1920,
            height: 1080,
            video_codec: Some("h264".into()),
            audio: vec![AudioTrack {
                index: 0,
                codec: "ac3".into(),
                language: Some("tur".into()),
                title: None,
                channels: 6,
                sample_rate: 48_000,
            }],
        }
    }

    #[test]
    fn choosing_a_file_starts_the_media_check() {
        let mut a = app();
        let _ = a.update(Message::FileChosen(PathBuf::from("/v/movie.mkv")));
        assert_eq!(a.pick.checking, Some(PathBuf::from("/v/movie.mkv")));
        assert_eq!(a.screen, Screen::Pick);
    }

    #[test]
    fn a_good_file_opens_the_choose_screen() {
        let mut a = app();
        let path = PathBuf::from("/v/movie.mkv");
        let _ = a.update(Message::FileChosen(path.clone()));
        let _ = a.update(Message::MediaChecked(Ok((path.clone(), info()))));
        assert_eq!(a.screen, Screen::Choose);
        let selection = a.selection().unwrap();
        assert_eq!(selection.video, path);
        assert_eq!(selection.target, "en");
        assert!(a.pick.checking.is_none());
    }

    #[test]
    fn a_file_without_audio_shows_the_error_on_the_drop_zone() {
        let mut a = app();
        let _ = a.update(Message::FileChosen(PathBuf::from("/v/silent.mp4")));
        let _ = a.update(Message::MediaChecked(Err("silent.mp4 has no audio".into())));
        assert_eq!(a.screen, Screen::Pick);
        assert_eq!(a.pick.error.as_deref(), Some("silent.mp4 has no audio"));
        assert!(a.selection().is_none());
    }

    #[test]
    fn a_non_ascii_path_reaches_the_choose_screen() {
        let mut a = app();
        let path = PathBuf::from("/v/Szép film.mkv");
        let _ = a.update(Message::FileChosen(path.clone()));
        let _ = a.update(Message::MediaChecked(Ok((path.clone(), info()))));
        assert_eq!(a.selection().unwrap().video, path);
    }

    #[test]
    fn a_recent_entry_opens_its_file_with_its_options() {
        let mut a = app();
        let path = PathBuf::from("/v/movie.mkv");
        a.settings.recent.push(Recent {
            path: path.clone(),
            target: "hu".into(),
            burn: true,
        });
        let _ = a.update(Message::OpenRecent(0));
        assert_eq!(a.pick.checking, Some(path.clone()));
        let _ = a.update(Message::MediaChecked(Ok((path, info()))));
        let selection = a.selection().unwrap();
        assert_eq!(selection.target, "hu");
        assert!(selection.burn);
    }

    fn chosen(dir: &str) -> App {
        let mut a = app();
        let base = std::env::temp_dir().join(format!("verbacue-choose-{dir}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let video = base.join("movie.mkv");
        let _ = a.update(Message::FileChosen(video.clone()));
        let _ = a.update(Message::MediaChecked(Ok((video, info()))));
        let _ = a.update(Message::Target("hu".into()));
        a
    }

    #[test]
    fn burn_card_builds_a_burn_job() {
        let mut a = chosen("burn");
        let _ = a.update(Message::Mode(OutputMode::Burn));
        let job = a.job().unwrap();
        assert!(job.burn && !job.embed);
        assert_eq!(job.target, "hu");
        assert!(job.translation.is_some());
    }

    #[test]
    fn srt_card_builds_a_subtitle_only_job() {
        let mut a = chosen("srt");
        let _ = a.update(Message::Mode(OutputMode::Srt));
        let job = a.job().unwrap();
        assert!(!job.burn && !job.embed);
        assert!(job.translation.is_some());
    }

    #[test]
    fn keep_original_language_builds_a_job_without_translation() {
        let mut a = chosen("keep");
        let _ = a.update(Message::Target(KEEP.into()));
        let job = a.job().unwrap();
        assert!(job.translation.is_none());
    }

    #[test]
    fn start_asks_before_replacing_an_existing_output() {
        let mut a = chosen("replace");
        let _ = a.update(Message::Mode(OutputMode::Burn));
        let out = verbacue::pipeline::final_output(&a.job().unwrap()).unwrap();
        std::fs::write(&out, "").unwrap();
        let _ = a.update(Message::Start);
        assert_eq!(a.screen, Screen::Choose);
        assert!(a.choose.confirm_replace);
    }

    #[test]
    fn confirm_replace_starts_with_force() {
        let mut a = chosen("force");
        let _ = a.update(Message::Mode(OutputMode::Burn));
        let out = verbacue::pipeline::final_output(&a.job().unwrap()).unwrap();
        std::fs::write(&out, "").unwrap();
        let _ = a.update(Message::Start);
        let _ = a.update(Message::ConfirmReplace(true));
        assert_eq!(a.screen, Screen::Convert);
        assert!(a.running_job().unwrap().force);
    }

    fn converting(dir: &str) -> App {
        let mut a = chosen(dir);
        let _ = a.update(Message::Start);
        assert_eq!(a.screen, Screen::Convert);
        a
    }

    fn update(a: &mut App, u: Update) {
        let _ = a.update(Message::Pipeline(u));
    }

    #[test]
    fn step_events_move_the_steps_row() {
        let mut a = converting("steps");
        // Read audio, Transcribe, Read the story, Translate, Burn in.
        assert_eq!(a.convert.steps.len(), 5);
        update(&mut a, Update::Step(Step::Decode));
        assert_eq!(a.convert.current, Some(0));
        update(&mut a, Update::Step(Step::Download("large-v3")));
        assert_eq!(a.convert.steps.len(), 6);
        assert_eq!(a.convert.steps[1].label, "Download large-v3");
        update(&mut a, Update::Step(Step::Transcribe));
        assert_eq!(a.convert.current, Some(2));
        update(
            &mut a,
            Update::Progress {
                done: 40,
                total: 100,
            },
        );
        assert_eq!(a.convert.progress(), 0.4);
    }

    #[test]
    fn cue_events_keep_the_newest_three() {
        let mut a = converting("cues");
        for i in 0..5 {
            update(
                &mut a,
                Update::Cue(Cue::new(i * 1000, i * 1000 + 900, format!("line {i}"))),
            );
        }
        let texts: Vec<_> = a.convert.cues.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn cancel_sets_the_flag_and_shows_stopping() {
        let mut a = converting("cancel");
        let _ = a.update(Message::Cancel);
        assert!(a.convert.stopping);
        assert!(a.convert.cancel.load(Ordering::Relaxed));
        update(
            &mut a,
            Update::Finished(Err(Failure::new(ErrorKind::Cancelled, "cancelled"))),
        );
        assert_eq!(a.screen, Screen::Choose);
    }

    #[test]
    fn close_request_during_a_run_asks_first() {
        let mut a = converting("close");
        let _ = a.update(Message::CloseRequested);
        assert!(a.convert.confirm_close);
        assert!(!a.convert.cancel.load(Ordering::Relaxed));
        let _ = a.update(Message::ConfirmClose(true));
        assert!(a.convert.cancel.load(Ordering::Relaxed));
        assert!(a.convert.close_after);
    }

    #[test]
    fn missing_key_error_shows_the_settings_hint() {
        let mut a = converting("key");
        update(
            &mut a,
            Update::Finished(Err(Failure::new(ErrorKind::MissingKey, "no DEEPL_API_KEY"))),
        );
        assert_eq!(a.screen, Screen::Convert);
        let error = a.convert.error.as_ref().unwrap();
        assert_eq!(
            friendly(error.kind),
            "Add your key for this service in Settings."
        );
        assert_eq!(error.message, "no DEEPL_API_KEY");
    }

    #[test]
    fn write_error_shows_a_card_not_a_panic() {
        let mut a = converting("write");
        update(
            &mut a,
            Update::Finished(Err(Failure::new(
                ErrorKind::Other,
                "cannot write /x/movie.hu.srt",
            ))),
        );
        assert_eq!(a.screen, Screen::Convert);
        assert_eq!(
            friendly(a.convert.error.as_ref().unwrap().kind),
            "Something went wrong."
        );
    }

    #[test]
    fn success_opens_the_done_screen_and_remembers_the_file() {
        let mut a = converting("done");
        let video = a.selection().unwrap().video.clone();
        let outcome = Outcome {
            source_language: "tr".into(),
            files: vec![video.with_extension("hu.mp4")],
            skipped: false,
        };
        update(&mut a, Update::Finished(Ok(outcome.clone())));
        assert_eq!(a.screen, Screen::Done);
        assert_eq!(a.queue[0].outcome(), Some(&outcome));
        assert_eq!(a.settings.recent[0].path, video);
        assert_eq!(a.settings.target, "hu");
    }

    fn done(dir: &str) -> (App, Outcome) {
        let mut a = converting(dir);
        let video = a.selection().unwrap().video.clone();
        let srt = video.with_extension("hu.srt");
        std::fs::write(
            &srt,
            "1\n00:00:01,000 --> 00:00:02,000\nSzia\n\n2\n00:00:03,000 --> 00:00:04,000\nHelló\n",
        )
        .unwrap();
        let outcome = Outcome {
            source_language: "tr".into(),
            files: vec![
                video.with_extension("tr.srt"),
                srt,
                video.with_extension("hu.mp4"),
            ],
            skipped: false,
        };
        update(&mut a, Update::Finished(Ok(outcome.clone())));
        (a, outcome)
    }

    #[test]
    fn again_returns_to_pick() {
        let (mut a, _) = done("again");
        let _ = a.update(Message::Again);
        assert_eq!(a.screen, Screen::Pick);
        assert!(a.selection().is_none());
    }

    #[test]
    fn done_lists_every_written_file() {
        let (a, outcome) = done("files");
        let names = screens::done::file_names(&outcome);
        assert_eq!(names, ["movie.tr.srt", "movie.hu.srt", "movie.hu.mp4"]);
        assert_eq!(a.queue[0].lines(), 2);
    }

    #[test]
    fn changing_a_setting_saves_the_file() {
        let dir = std::env::temp_dir().join("verbacue-gui-settings-save");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = app();
        a.config = Some(dir.clone());
        let _ = a.update(Message::Setting(SettingChange::Vad(false)));
        assert!(!a.settings.vad);
        let (saved, _) = settings::load(&dir);
        assert!(!saved.vad);
    }

    #[test]
    fn server_translator_shows_the_url_fields() {
        let mut a = app();
        assert!(!screens::settings::shows_server_fields(&a.settings));
        let _ = a.update(Message::Setting(SettingChange::Translator(
            verbacue::translate::Backend::OpenaiCompat,
        )));
        assert!(screens::settings::shows_server_fields(&a.settings));
    }

    #[test]
    fn model_download_reports_progress_and_marks_downloaded() {
        let mut a = app();
        let _ = a.update(Message::ModelProgress("small", 50, 200));
        assert_eq!(a.downloads.get("small"), Some(&(50, 200)));
        let _ = a.update(Message::ModelDone("small", Ok(())));
        assert!(!a.downloads.contains_key("small"));
        let _ = a.update(Message::ModelDone("small", Err("network down".into())));
        assert_eq!(
            a.notice.as_deref(),
            Some("Download of small failed: network down")
        );
    }

    #[test]
    fn ui_scale_grows_with_the_window() {
        assert_eq!(ui_scale(iced::Size::new(1280.0, 800.0)), 1.0);
        assert_eq!(ui_scale(iced::Size::new(960.0, 640.0)), 1.0);
        assert_eq!(ui_scale(iced::Size::new(1920.0, 1080.0)), 1.35);
        assert_eq!(ui_scale(iced::Size::new(2560.0, 1600.0)), 2.0);
        assert_eq!(ui_scale(iced::Size::new(5120.0, 2880.0)), 2.5);
    }

    #[test]
    fn resize_events_do_not_feed_back_into_the_scale() {
        let mut a = app();
        let _ = a.update(Message::Resized(iced::Size::new(2560.0, 1600.0)));
        assert_eq!(a.scale, 2.0);
        // iced reports the size after the app scale: the same window is now 1280 x 800.
        let _ = a.update(Message::Resized(iced::Size::new(1280.0, 800.0)));
        assert_eq!(a.scale, 2.0);
        // Later the user makes the window small again.
        let _ = a.update(Message::SizeChecked(iced::Size::new(640.0, 400.0)));
        assert_eq!(a.scale, 1.0);
    }

    #[test]
    fn resize_events_right_after_a_scale_change_are_ignored() {
        let mut a = app();
        let _ = a.update(Message::Resized(iced::Size::new(2400.0, 1500.0)));
        assert_eq!(a.scale, 1.9);
        // iced made this event before it applied 1.9 (the window is clamped to
        // 2400 x 1378): its size is still divided by 1, so it is wrong now.
        let _ = a.update(Message::Resized(iced::Size::new(2400.0, 1378.0)));
        assert_eq!(a.scale, 1.9);
        // The check after the settle time reads the size with 1.9 applied.
        let _ = a.update(Message::SizeChecked(iced::Size::new(
            2400.0 / 1.9,
            1378.0 / 1.9,
        )));
        assert_eq!(a.scale, 1.7);
        let _ = a.update(Message::SizeChecked(iced::Size::new(
            2400.0 / 1.7,
            1378.0 / 1.7,
        )));
        assert_eq!(a.scale, 1.7);
    }

    #[test]
    fn window_title_names_the_file_while_converting() {
        let mut a = converting("title");
        update(&mut a, Update::Step(Step::Transcribe));
        update(
            &mut a,
            Update::Progress {
                done: 42,
                total: 100,
            },
        );
        // Transcribe is step 2 of 5 (with "Read the story") and 42 % done:
        // (1 + 0.42) / 5 is about 28 %.
        assert_eq!(a.title(), "VERBACUE — movie.mkv — 28 %");
        assert_eq!(app().title(), "VERBACUE");
    }

    /// A queue of videos in a fresh folder, on the Choose screen.
    fn queued(dir: &str, names: &[&str]) -> (App, Vec<PathBuf>) {
        let mut a = app();
        let base = std::env::temp_dir().join(format!("verbacue-queue-{dir}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths: Vec<PathBuf> = names.iter().map(|n| base.join(n)).collect();
        for path in &paths {
            let _ = a.update(Message::FileChosen(path.clone()));
        }
        for path in &paths {
            let _ = a.update(Message::MediaChecked(Ok((path.clone(), info()))));
        }
        (a, paths)
    }

    fn statuses(a: &App) -> Vec<&'static str> {
        a.queue.iter().map(|item| item.status.label()).collect()
    }

    #[test]
    fn several_files_join_the_queue() {
        let (a, paths) = queued("join", &["a.mkv", "b.mkv", "c.mkv"]);
        assert_eq!(a.screen, Screen::Choose);
        assert_eq!(a.queue.len(), 3);
        assert_eq!(a.current, 0);
        assert_eq!(a.selection().unwrap().video, paths[0]);
        assert_eq!(statuses(&a), ["waiting", "waiting", "waiting"]);
    }

    #[test]
    fn each_video_keeps_its_own_options() {
        let (mut a, _) = queued("own", &["a.mkv", "b.mkv"]);
        let _ = a.update(Message::Target("hu".into()));
        let _ = a.update(Message::Select(1));
        let _ = a.update(Message::Mode(OutputMode::Srt));
        let _ = a.update(Message::Target("de".into()));
        let _ = a.update(Message::Select(0));
        assert_eq!(a.selection().unwrap().target, "hu");
        assert!(a.selection().unwrap().burn);
        assert_eq!(a.queue[1].selection.target, "de");
        assert!(!a.queue[1].selection.burn);
    }

    #[test]
    fn apply_to_all_copies_the_options_to_waiting_videos() {
        let (mut a, _) = queued("apply", &["a.mkv", "b.mkv", "c.mkv"]);
        let _ = a.update(Message::Mode(OutputMode::Srt));
        let _ = a.update(Message::Target("hu".into()));
        let _ = a.update(Message::ApplyToAll);
        assert!(
            a.queue
                .iter()
                .all(|i| i.selection.target == "hu" && !i.selection.burn)
        );
    }

    #[test]
    fn the_queue_runs_videos_one_after_another() {
        let (mut a, paths) = queued("run", &["a.mkv", "b.mkv"]);
        let _ = a.update(Message::Start);
        assert_eq!(a.screen, Screen::Convert);
        assert_eq!(statuses(&a), ["running", "waiting"]);
        assert_eq!(a.running_job().unwrap().video, paths[0]);
        update(&mut a, Update::Finished(Ok(Outcome::default())));
        assert_eq!(statuses(&a), ["done", "running"]);
        assert_eq!(a.running_job().unwrap().video, paths[1]);
        assert_eq!(a.screen, Screen::Convert);
        update(&mut a, Update::Finished(Ok(Outcome::default())));
        assert_eq!(statuses(&a), ["done", "done"]);
        assert_eq!(a.screen, Screen::Done);
    }

    #[test]
    fn a_failed_video_does_not_stop_the_queue() {
        let (mut a, _) = queued("fail", &["a.mkv", "b.mkv"]);
        let _ = a.update(Message::Start);
        update(
            &mut a,
            Update::Finished(Err(Failure::new(ErrorKind::InvalidInput, "bad"))),
        );
        assert_eq!(statuses(&a), ["failed", "running"]);
        update(&mut a, Update::Finished(Ok(Outcome::default())));
        assert_eq!(a.screen, Screen::Done);
        assert_eq!(statuses(&a), ["failed", "done"]);
    }

    #[test]
    fn videos_added_during_a_run_join_the_end() {
        let (mut a, _) = queued("add", &["a.mkv"]);
        let _ = a.update(Message::Start);
        let late = std::env::temp_dir().join("verbacue-queue-add/late.mkv");
        let _ = a.update(Message::FileChosen(late.clone()));
        let _ = a.update(Message::MediaChecked(Ok((late.clone(), info()))));
        assert_eq!(a.screen, Screen::Convert);
        assert_eq!(statuses(&a), ["running", "waiting"]);
        update(&mut a, Update::Finished(Ok(Outcome::default())));
        assert_eq!(a.running_job().unwrap().video, late);
    }

    #[test]
    fn waiting_videos_can_be_removed() {
        let (mut a, paths) = queued("remove", &["a.mkv", "b.mkv", "c.mkv"]);
        let _ = a.update(Message::Start);
        let _ = a.update(Message::Remove(0));
        assert_eq!(a.queue.len(), 3, "a running video stays");
        let _ = a.update(Message::Remove(1));
        assert_eq!(a.queue.len(), 2);
        assert_eq!(a.queue[1].selection.video, paths[2]);
    }

    #[test]
    fn cancel_stops_the_queue_and_keeps_the_waiting_videos() {
        let (mut a, _) = queued("cancel", &["a.mkv", "b.mkv"]);
        let _ = a.update(Message::Start);
        let _ = a.update(Message::Cancel);
        update(
            &mut a,
            Update::Finished(Err(Failure::new(ErrorKind::Cancelled, "cancelled"))),
        );
        assert_eq!(a.screen, Screen::Choose);
        assert_eq!(statuses(&a), ["waiting", "waiting"]);
        assert!(a.running_job().is_none());
    }

    #[test]
    fn the_queue_keeps_the_order_in_which_files_were_chosen() {
        let mut a = app();
        let (first, second) = (PathBuf::from("/v/a.mkv"), PathBuf::from("/v/b.mkv"));
        let _ = a.update(Message::FileChosen(first.clone()));
        let _ = a.update(Message::FileChosen(second.clone()));
        // The check of the second file ends first.
        let _ = a.update(Message::MediaChecked(Ok((second.clone(), info()))));
        let _ = a.update(Message::MediaChecked(Ok((first.clone(), info()))));
        let order: Vec<_> = a.queue.iter().map(|i| i.selection.video.clone()).collect();
        assert_eq!(order, [first, second]);
    }

    #[test]
    fn video_notes_and_film_notes_reach_the_job() {
        let mut a = chosen("notes");
        let _ = a.update(Message::Notes("Two sisters argue.".into()));
        let _ = a.update(Message::Setting(SettingChange::FilmNotes(true)));
        let translation = a.job().unwrap().translation.unwrap();
        assert_eq!(translation.notes, "Two sisters argue.");
        assert!(translation.film_notes);
    }

    #[test]
    fn automatic_notes_open_an_editor_and_continue_sends_the_correction() {
        let mut a = converting("review");
        update(&mut a, Update::Review("Ayse is the sister.".into()));
        assert_eq!(
            a.convert.review.as_ref().unwrap().text().trim(),
            "Ayse is the sister."
        );
        let (reply, replies) = std::sync::mpsc::channel();
        a.convert.reply = Some(reply);
        let _ = a.update(Message::ReviewEdit(
            iced::widget::text_editor::Action::Edit(iced::widget::text_editor::Edit::Insert('!')),
        ));
        let _ = a.update(Message::ReviewDone);
        assert!(a.convert.review.is_none());
        assert!(replies.try_recv().unwrap().contains("sister"));
    }

    #[test]
    fn film_notes_and_their_review_are_on_by_default() {
        let s = Settings::default();
        assert!(s.film_notes);
        assert!(s.review_notes);
    }
}
