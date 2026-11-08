//! The app state, its messages, and the screen it shows.

use std::{path::PathBuf, time::Instant};

use iced::{
    Element, Length, Subscription, Task,
    widget::{canvas, column, container, row, rule, stack, text},
    window,
};
use verbacue::{
    models,
    pipeline::{self, Job, Translation},
    video::{MediaInfo, PreviewFrame},
};

use crate::{
    screens::{
        self,
        choose::{Choose, OutputMode},
        pick::Pick,
    },
    settings::{self, Settings},
    theme,
    widgets::{backdrop::Backdrop, nav},
};

/// The screens of the app, in the order of the main flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Pick,
    Choose,
    Convert,
    #[expect(dead_code, reason = "the Done screen comes in a later task")]
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
}

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
    /// Opens the file dialog.
    PickFile,
    FileChosen(PathBuf),
    /// A file is dragged over the window (true) or away from it (false).
    Hover(bool),
    MediaChecked(Result<(PathBuf, MediaInfo), String>),
    OpenRecent(usize),
    PreviewLoaded(Vec<PreviewFrame>),
    Mode(OutputMode),
    Target(String),
    Track(usize),
    PlayVideo,
    Start,
    ConfirmReplace(bool),
    #[cfg(debug_assertions)]
    Screenshot(iced::window::Screenshot),
}

/// The whole state of the app.
pub struct App {
    pub screen: Screen,
    pub settings: Settings,
    /// A notice about the settings file, shown once.
    pub notice: Option<String>,
    pub pick: Pick,
    pub selection: Option<Selection>,
    pub choose: Choose,
    /// The job that runs or ran last.
    pub running: Option<Job>,
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
        let app = Self::with_settings(settings, notice);
        #[cfg(debug_assertions)]
        if crate::devshot::target().is_some() {
            let setup = crate::devshot::setup().into_iter().map(Task::done);
            return (app, Task::batch(setup.chain([crate::devshot::take()])));
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
            selection: None,
            choose: Choose::default(),
            running: None,
            opened: now,
            now,
            logo: nav::logo(),
        }
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
                    .pick_file()
                    .await
            })
            .and_then(|file| Task::done(Message::FileChosen(file.path().to_path_buf()))),
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
                let options = self.pick.recent_options.take();
                match result {
                    Ok((video, info)) => {
                        let (target, burn) =
                            options.unwrap_or_else(|| (self.settings.target.clone(), true));
                        self.selection = Some(Selection {
                            video,
                            info,
                            burn,
                            target,
                            track: 0,
                        });
                        self.pick.error = None;
                        self.choose = Choose::default();
                        self.show(Screen::Choose);
                        return self.load_preview();
                    }
                    Err(error) => self.pick.error = Some(error),
                }
                Task::none()
            }
            Message::PreviewLoaded(frames) => {
                self.choose.frames = frames
                    .into_iter()
                    .map(|f| iced::widget::image::Handle::from_rgba(f.width, f.height, f.rgba))
                    .collect();
                Task::none()
            }
            Message::Mode(mode) => {
                if let Some(s) = &mut self.selection {
                    s.burn = mode == OutputMode::Burn;
                    self.choose.mode_changed = Instant::now();
                }
                Task::none()
            }
            Message::Target(code) => {
                if let Some(s) = &mut self.selection {
                    s.target = code;
                }
                Task::none()
            }
            Message::Track(index) => {
                if let Some(s) = &mut self.selection {
                    s.track = index;
                }
                Task::none()
            }
            Message::PlayVideo => {
                if let Some(s) = &self.selection {
                    crate::system::open(&s.video);
                }
                Task::none()
            }
            Message::Start => {
                let Some(job) = self.job() else {
                    return Task::none();
                };
                if pipeline::final_output(&job).is_some_and(|p| p.exists()) {
                    self.choose.confirm_replace = true;
                    return Task::none();
                }
                self.start(job)
            }
            Message::ConfirmReplace(replace) => {
                self.choose.confirm_replace = false;
                match self.job() {
                    Some(mut job) if replace => {
                        job.force = true;
                        self.start(job)
                    }
                    _ => Task::none(),
                }
            }
            #[cfg(debug_assertions)]
            Message::Screenshot(shot) => crate::devshot::save(&shot),
        }
    }

    /// The job for the current selection and settings.
    pub fn job(&self) -> Option<Job> {
        let s = self.selection.as_ref()?;
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
            });
        Some(job)
    }

    /// Opens the Converting screen for `job`.
    fn start(&mut self, job: Job) -> Task<Message> {
        self.running = Some(job);
        self.show(Screen::Convert);
        Task::none()
    }

    /// Decodes the preview frames on the blocking thread pool.
    fn load_preview(&self) -> Task<Message> {
        let Some(video) = self.selection.as_ref().map(|s| s.video.clone()) else {
            return Task::none();
        };
        Task::future(async move {
            let frames = tokio::task::spawn_blocking(move || {
                verbacue::video::preview_frames(&video, 6, 960).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            Message::PreviewLoaded(frames)
        })
    }

    /// Reads the facts of `path` on the blocking thread pool.
    fn check(&mut self, path: PathBuf, options: Option<(String, bool)>) -> Task<Message> {
        self.pick.hovering = false;
        self.pick.error = None;
        self.pick.checking = Some(path.clone());
        self.pick.recent_options = options;
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
        let animated = matches!(self.screen, Screen::Pick | Screen::Choose);
        if animated {
            Subscription::batch([drops, window::frames().map(Message::Tick)])
        } else {
            drops
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
            other => container(
                text(format!("{other:?}"))
                    .font(theme::HEADING)
                    .size(40)
                    .color(theme::TEXT),
            )
            .padding([48, 64])
            .into(),
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
        let glow = Backdrop {
            center: (0.7, -0.1),
            glow: theme::GLOW,
        };
        stack![
            canvas(glow).width(Length::Fill).height(Length::Fill),
            content
        ]
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{screens::choose::OutputMode, settings::Recent};
    use std::path::PathBuf;
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
        let selection = a.selection.as_ref().unwrap();
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
        assert!(a.selection.is_none());
    }

    #[test]
    fn a_non_ascii_path_reaches_the_choose_screen() {
        let mut a = app();
        let path = PathBuf::from("/v/Szép film.mkv");
        let _ = a.update(Message::FileChosen(path.clone()));
        let _ = a.update(Message::MediaChecked(Ok((path.clone(), info()))));
        assert_eq!(a.selection.unwrap().video, path);
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
        let selection = a.selection.unwrap();
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
        assert!(a.running.as_ref().unwrap().force);
    }
}
