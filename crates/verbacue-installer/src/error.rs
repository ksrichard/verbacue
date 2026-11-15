//! The error type of the installer.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// A message about what failed, with the error that caused it.
    #[error("{message}: {source}")]
    Context { message: String, source: Box<Error> },
    #[error("{0}")]
    Message(String),
    #[error("{file} is damaged: its SHA256 does not match the release")]
    Damaged { file: String },
    #[error("{program} failed ({status})")]
    Failed {
        program: String,
        status: std::process::ExitStatus,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// `source` with a message about what failed.
    pub fn context(message: impl Into<String>, source: impl Into<Error>) -> Self {
        Self::Context {
            message: message.into(),
            source: Box::new(source.into()),
        }
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Message(message.into())
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}
