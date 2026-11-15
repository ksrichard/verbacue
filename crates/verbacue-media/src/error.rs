//! The error type of verbacue-media.

use verbacue_core::error::ErrorKind;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Ffmpeg(#[from] ffmpeg_next::Error),
    /// Not transparent, so that the chain keeps the shared error and its kind.
    #[error("{0}")]
    Core(#[from] verbacue_core::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Ffmpeg(_) => ErrorKind::Other,
            Self::Core(e) => e.kind(),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Core(e.into())
    }
}

impl From<Error> for verbacue_core::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Core(e) => e,
            e => Self::wrap(e.kind(), e),
        }
    }
}
