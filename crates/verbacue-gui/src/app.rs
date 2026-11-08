//! The app state, its messages, and the screen it shows.

use std::{path::PathBuf, time::Instant};

use iced::{
    Element, Length, Subscription, Task,
    widget::{canvas, column, container, row, rule, stack, text},
    window,
};
use verbacue::{models, video::MediaInfo};

use crate::{
    screens::{self, pick::Pick},
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
    #[expect(dead_code, reason = "the Converting screen comes in a later task")]
    Convert,
    #[expect(dead_code, reason = "the Done screen comes in a later task")]
    Done,
    Settings,
}

/// The video the user picked and what to make from it.
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "the Choose screen reads it in the next task")]
pub struct Selection {
    pub video: PathBuf,
    pub info: MediaInfo,
    /// Burned-in MP4 (true) or only subtitle files (false).
    pub burn: bool,
    /// A language code.
    pub target: String,
    pub track: usize,
}

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
            return (app, crate::devshot::take());
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
                        self.show(Screen::Choose);
                    }
                    Err(error) => self.pick.error = Some(error),
                }
                Task::none()
            }
            #[cfg(debug_assertions)]
            Message::Screenshot(shot) => crate::devshot::save(&shot),
        }
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
    use crate::settings::Recent;
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
}
