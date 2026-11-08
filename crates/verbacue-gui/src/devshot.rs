//! Debug builds only: `VERBACUE_GUI_SHOT=<file.png>` saves a screenshot of the window
//! a few seconds after start, then closes the app. It lets developers compare the
//! screens with the mockups without screen-capture rights. `VERBACUE_GUI_SHOT_FILE`
//! opens a video first, and `VERBACUE_GUI_SHOT_SCREEN=settings` opens Settings.
//! `VERBACUE_GUI_SHOT_START=1` presses Start, and `VERBACUE_GUI_SHOT_MODEL` sets the
//! Whisper model. In this mode the app never saves `gui.toml`.
//!
//! `VERBACUE_GUI_REC=<folder>` records the whole flow instead: it saves about 10 JPEG
//! frames per second into the folder, opens `VERBACUE_GUI_SHOT_FILE` after 2.5 s,
//! sets the target language to `VERBACUE_GUI_REC_TARGET` after 5 s, presses Start
//! after 7 s, and closes 4.5 s after the Done screen opens.

use std::{
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant},
};

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

/// The folder for recorded frames, if the variable is set.
pub fn rec_dir() -> Option<PathBuf> {
    std::env::var_os("VERBACUE_GUI_REC").map(PathBuf::from)
}

fn rec_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// The scripted flow and the first frame.
pub fn rec_begin() -> Task<Message> {
    rec_start();
    let mut tasks = vec![rec_take()];
    if let Some(file) = std::env::var_os("VERBACUE_GUI_SHOT_FILE") {
        let file = PathBuf::from(file);
        tasks.push(
            Task::future(tokio::time::sleep(Duration::from_millis(2500)))
                .map(move |()| Message::FileChosen(file.clone())),
        );
        if let Ok(target) = std::env::var("VERBACUE_GUI_REC_TARGET") {
            tasks.push(
                Task::future(tokio::time::sleep(Duration::from_millis(5000)))
                    .map(move |()| Message::Target(target.clone())),
            );
        }
        tasks.push(
            Task::future(tokio::time::sleep(Duration::from_millis(7000))).map(|()| Message::Start),
        );
    }
    Task::batch(tasks)
}

/// Takes one frame.
pub fn rec_take() -> Task<Message> {
    window::latest()
        .and_then(window::screenshot)
        .map(Message::RecFrame)
}

/// Saves a frame as a half-size JPEG named after its time in ms, on another thread,
/// then waits for the next frame or closes the window.
pub fn rec_save(shot: window::Screenshot, done: bool) -> Task<Message> {
    if let Some(dir) = rec_dir() {
        let ms = rec_start().elapsed().as_millis();
        std::thread::spawn(move || {
            let Some(frame) =
                image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())
            else {
                return;
            };
            let small = image::imageops::resize(
                &frame,
                shot.size.width / 2,
                shot.size.height / 2,
                image::imageops::FilterType::Triangle,
            );
            let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
            if let Err(e) = rgb.save(dir.join(format!("{ms:08}.jpg"))) {
                tracing::error!("cannot save a frame: {e}");
            }
        });
    }
    if done {
        return window::latest().and_then(window::close);
    }
    Task::future(tokio::time::sleep(Duration::from_millis(60))).then(|()| rec_take())
}
