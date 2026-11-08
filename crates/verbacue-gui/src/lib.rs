//! The verbacue desktop app: pick a video, choose burned-in subtitles or a subtitle
//! file, and watch the barbecue while verbacue works.
//!
//! [`run`] opens the window and returns when it closes.

mod app;
#[cfg(debug_assertions)]
mod devshot;
mod error;
mod settings;
mod theme;
mod widgets;

pub use error::Error;

const ICON: &[u8] = include_bytes!("../assets/icon.png");

/// Opens the app window and returns when it closes.
///
/// # Errors
/// Returns an error if the window cannot open.
pub fn run() -> Result<(), Error> {
    let icon = iced::window::icon::from_file_data(ICON, None).ok();
    let mut app = iced::application(app::App::new, app::App::update, app::App::view)
        .title("verbacue")
        .default_font(theme::BODY);
    for font in theme::FONTS {
        app = app.font(font);
    }
    app.window(iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        min_size: Some(iced::Size::new(960.0, 640.0)),
        icon,
        ..iced::window::Settings::default()
    })
    .run()
    .map_err(|e| Error::Window(e.to_string()))
}
