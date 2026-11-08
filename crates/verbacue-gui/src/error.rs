//! The error type of the desktop app.

/// Something that stops the desktop app.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Pipeline(#[from] verbacue::Error),
    #[error("cannot read or write the settings: {0}")]
    Settings(String),
    #[error("cannot open the window: {0}")]
    Window(String),
}
