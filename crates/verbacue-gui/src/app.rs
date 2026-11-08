//! The app state, its messages, and the screen it shows.

use iced::{Element, Task, widget::text};

/// The screens of the app, in the order of the main flow.
#[expect(dead_code, reason = "the screens come in the next tasks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Pick,
    Choose,
    Convert,
    Done,
    Settings,
}

/// Everything that can happen in the app.
#[derive(Debug, Clone)]
pub enum Message {}

/// The whole state of the app.
#[derive(Default)]
pub struct App {
    pub screen: Screen,
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        (Self::default(), Task::none())
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        text(format!("verbacue: {:?}", self.screen)).into()
    }
}
