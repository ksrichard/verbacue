//! Debug builds only: `VERBACUE_GUI_SHOT=<file.png>` saves a screenshot of the window
//! a few seconds after start, then closes the app. It lets developers compare the
//! screens with the mockups without screen-capture rights.

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
