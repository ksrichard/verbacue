//! The verbacue desktop app: pick a video, choose burned-in subtitles or a subtitle
//! file, and watch the barbecue while verbacue works.
//!
//! [`run`] opens the window and returns when it closes.

mod anim;
mod app;
#[cfg(debug_assertions)]
mod devshot;
mod error;
mod screens;
mod settings;
mod system;
mod theme;
mod widgets;
mod worker;

pub use error::Error;

const ICON: &[u8] = include_bytes!("../assets/icon.png");

/// Opens the app window and returns when it closes.
///
/// # Errors
/// Returns an error if the window cannot open.
pub fn run() -> Result<(), Error> {
    let icon = iced::window::icon::from_file_data(ICON, None).ok();
    let mut app = iced::application(app::App::new, app::App::update, app::App::view)
        .title(app::App::title)
        .subscription(app::App::subscription)
        .theme(|_: &app::App| theme::app_theme())
        .exit_on_close_request(false)
        .scale_factor(|app: &app::App| app.scale)
        .default_font(theme::BODY);
    for font in theme::FONTS {
        app = app.font(font);
    }
    app.window(iced::window::Settings {
        size: start_size(),
        min_size: Some(iced::Size::new(960.0, 640.0)),
        icon,
        ..iced::window::Settings::default()
    })
    .run()
    .map_err(|e| Error::Window(e.to_string()))
}

/// The window size at start. Debug builds take `VERBACUE_GUI_SHOT_SIZE=<w>x<h>`.
fn start_size() -> iced::Size {
    #[cfg(debug_assertions)]
    if let Some((w, h)) = std::env::var("VERBACUE_GUI_SHOT_SIZE")
        .ok()
        .and_then(|v| v.split_once('x').map(|(w, h)| (w.parse(), h.parse())))
        .and_then(|(w, h)| Some((w.ok()?, h.ok()?)))
    {
        return iced::Size::new(w, h);
    }
    iced::Size::new(1280.0, 800.0)
}
