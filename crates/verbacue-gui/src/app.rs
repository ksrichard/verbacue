//! The app state, its messages, and the screen it shows.

use iced::{
    Element, Length, Task,
    widget::{canvas, column, container, row, rule, stack, text},
};
use verbacue::models;

use crate::{
    settings::{self, Settings},
    theme,
    widgets::{backdrop::Backdrop, nav},
};

/// The screens of the app, in the order of the main flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Pick,
    #[expect(dead_code, reason = "the Choose screen comes in a later task")]
    Choose,
    #[expect(dead_code, reason = "the Converting screen comes in a later task")]
    Convert,
    #[expect(dead_code, reason = "the Done screen comes in a later task")]
    Done,
    Settings,
}

/// Everything that can happen in the app.
#[derive(Debug, Clone)]
pub enum Message {
    Show(Screen),
    #[cfg(debug_assertions)]
    Screenshot(iced::window::Screenshot),
}

/// The whole state of the app.
pub struct App {
    pub screen: Screen,
    pub settings: Settings,
    /// A notice about the settings file, shown once.
    pub notice: Option<String>,
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let (mut settings, notice) = verbacue::config_dir()
            .map(|dir| settings::load(&dir))
            .unwrap_or_default();
        settings.drop_missing();
        let app = Self {
            screen: Screen::Pick,
            settings,
            notice,
        };
        #[cfg(debug_assertions)]
        if crate::devshot::target().is_some() {
            return (app, crate::devshot::take());
        }
        (app, Task::none())
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Show(screen) => {
                self.screen = screen;
                Task::none()
            }
            #[cfg(debug_assertions)]
            Message::Screenshot(shot) => crate::devshot::save(&shot),
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
        let page: Element<'_, Message> = container(
            text(format!("{:?}", self.screen))
                .font(theme::HEADING)
                .size(40)
                .color(theme::TEXT),
        )
        .padding([48, 64])
        .into();
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
            nav::view(self.screen, self.model_count()),
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
