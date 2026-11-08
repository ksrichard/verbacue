//! Debug builds only: `VERBACUE_GUI_SHOT=<file.png>` saves a screenshot of the window
//! a few seconds after start, then closes the app. It lets developers compare the
//! screens with the mockups without screen-capture rights. `VERBACUE_GUI_SHOT_FILE`
//! opens a video first, and `VERBACUE_GUI_SHOT_SCREEN=settings` opens Settings.
//! `VERBACUE_GUI_SHOT_START=1` presses Start, and `VERBACUE_GUI_SHOT_MODEL` sets the
//! Whisper model. In this mode the app never saves `gui.toml`.

use std::{path::PathBuf, time::Duration};

use iced::{Task, window};

use crate::app::Message;

/// The file to write, if the variable is set.
pub fn target() -> Option<PathBuf> {
    std::env::var_os("VERBACUE_GUI_SHOT").map(PathBuf::from)
}

/// Waits, takes the screenshot and sends it back as a message.
pub fn take() -> Task<Message> {
    let wait = std::env::var("VERBACUE_GUI_SHOT_WAIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3.0);
    Task::future(tokio::time::sleep(Duration::from_secs_f64(wait)))
        .then(|()| window::latest())
        .and_then(window::screenshot)
        .map(Message::Screenshot)
}

/// The Whisper model to use instead of the setting.
pub fn model() -> Option<String> {
    std::env::var("VERBACUE_GUI_SHOT_MODEL").ok()
}

/// Presses Start once the preview had time to load.
pub fn start() -> Task<Message> {
    if std::env::var_os("VERBACUE_GUI_SHOT_START").is_none() {
        return Task::none();
    }
    Task::future(tokio::time::sleep(Duration::from_millis(1500))).map(|()| Message::Start)
}

/// The messages that bring the app to the screen to capture.
pub fn setup() -> Vec<Message> {
    let mut messages = Vec::new();
    if let Some(file) = std::env::var_os("VERBACUE_GUI_SHOT_FILE") {
        messages.push(Message::FileChosen(PathBuf::from(file)));
    }
    if std::env::var("VERBACUE_GUI_SHOT_SCREEN").is_ok_and(|s| s == "settings") {
        messages.push(Message::Show(crate::app::Screen::Settings));
    }
    messages
}

/// Saves the screenshot and closes the window.
pub fn save(shot: &window::Screenshot) -> Task<Message> {
    if let Some(path) = target() {
        let saved = image::save_buffer(
            &path,
            &shot.rgba,
            shot.size.width,
            shot.size.height,
            image::ExtendedColorType::Rgba8,
        );
        if let Err(e) = saved {
            tracing::error!("cannot save the screenshot: {e}");
        }
    }
    window::latest().and_then(window::close)
}
