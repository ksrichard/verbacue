//! The error type of all verbacue crates.
//!
//! An [`Error`] has a [`kind`](Error::kind), so callers can react to some problems (for
//! example a used-up quota), and a chain of sources. `{}` shows the outermost message;
//! [`report`] shows the whole chain.

use std::fmt::Display;

/// What kind of problem an [`Error`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A file or a model does not exist.
    NotFound,
    /// The input is not usable, for example a broken subtitle file, or a video that
    /// this build cannot decode.
    InvalidInput,
    /// An online service needs a key, and none is set.
    MissingKey,
    /// An online service refused the key.
    KeyRefused,
    /// An online service has no characters left for this key.
    QuotaExceeded,
    /// A network request failed.
    Network,
    /// Whisper or the translation model failed.
    Model,
    /// The caller stopped the task.
    Cancelled,
    /// Any other problem, for example a file that cannot be written.
    Other,
}

/// The shared error type: messages, context, and the errors of common libraries.
/// Each crate has its own error type for its own libraries, which holds this one.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{message}")]
    Message { kind: ErrorKind, message: String },
    /// A message about what failed, with the error that caused it.
    #[error("{message}")]
    Context {
        message: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Text(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    Number(#[from] std::num::ParseIntError),
    #[cfg(feature = "http")]
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// An error of another library, with the kind that the caller gave it.
    #[error("{source}")]
    Foreign {
        kind: ErrorKind,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// The result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Makes an error with a message.
    pub fn new(kind: ErrorKind, message: impl Display) -> Self {
        Self::Message {
            kind,
            message: message.to_string(),
        }
    }

    /// Makes an error of `kind` from an error of another library.
    pub fn wrap(kind: ErrorKind, error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Foreign {
            kind,
            source: Box::new(error),
        }
    }

    /// The kind of problem. A context takes the kind of the first shared error in its
    /// chain.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Message { kind, .. } | Self::Foreign { kind, .. } => *kind,
            Self::Context { source, .. } => kind_of(source.as_ref()),
            Self::Io(e) if e.kind() == std::io::ErrorKind::NotFound => ErrorKind::NotFound,
            #[cfg(feature = "http")]
            Self::Http(_) => ErrorKind::Network,
            _ => ErrorKind::Other,
        }
    }
}

/// The kind of the first [`Error`] in the chain of `error`, or `Other`.
pub fn kind_of(error: &(dyn std::error::Error + 'static)) -> ErrorKind {
    std::iter::successors(Some(error), |e| e.source())
        .find_map(|e| e.downcast_ref::<Error>().map(Error::kind))
        .unwrap_or(ErrorKind::Other)
}

/// The messages of `error` and its sources, joined with ": ". A message that repeats
/// the one before it (an error that shows its source) is left out.
pub fn report(error: &(dyn std::error::Error + 'static)) -> String {
    let mut out = String::new();
    let mut last = String::new();
    for e in std::iter::successors(Some(error), |e| e.source()) {
        let message = e.to_string();
        if message != last {
            if !out.is_empty() {
                out.push_str(": ");
            }
            out.push_str(&message);
        }
        last = message;
    }
    out
}

/// Implements `From` for foreign error types with one kind.
macro_rules! from_errors {
    ($kind:expr => $($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for Error {
            fn from(e: $ty) -> Self {
                Self::wrap($kind, e)
            }
        }
    )+};
}

from_errors!(ErrorKind::Other => std::fmt::Error, std::str::Utf8Error);

/// Gives an error from another library a kind, for example
/// `whisper_call().with_kind(ErrorKind::Model)?`.
pub trait WithKind<T> {
    /// Turns the error into an [`Error`] of this kind.
    ///
    /// # Errors
    ///
    /// Returns the error of `self` with the kind.
    fn with_kind(self, kind: ErrorKind) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> WithKind<T> for std::result::Result<T, E> {
    fn with_kind(self, kind: ErrorKind) -> Result<T> {
        self.map_err(|e| Error::wrap(kind, e))
    }
}

/// Adds a message to an error. The error stays the typed source of the message.
pub trait Context<T> {
    /// Adds a message to the error, or turns `None` into an error with this message.
    ///
    /// # Errors
    ///
    /// Returns the error of `self` with the message added.
    fn context<C: Display>(self, message: C) -> Result<T>;
    /// The same as [`context`](Context::context), but makes the message only when
    /// there is an error.
    ///
    /// # Errors
    ///
    /// Returns the error of `self` with the message added.
    fn with_context<C: Display, F: FnOnce() -> C>(self, message: F) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> Context<T> for std::result::Result<T, E> {
    fn context<C: Display>(self, message: C) -> Result<T> {
        self.with_context(|| message)
    }

    fn with_context<C: Display, F: FnOnce() -> C>(self, message: F) -> Result<T> {
        self.map_err(|e| Error::Context {
            message: message().to_string(),
            source: Box::new(e),
        })
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: Display>(self, message: C) -> Result<T> {
        self.ok_or_else(|| Error::new(ErrorKind::Other, message))
    }

    fn with_context<C: Display, F: FnOnce() -> C>(self, message: F) -> Result<T> {
        self.ok_or_else(|| Error::new(ErrorKind::Other, message()))
    }
}

/// Returns an error. `bail!(Kind; "...")` sets the kind; plain `bail!("...")` uses
/// [`ErrorKind::Other`].
#[macro_export]
macro_rules! bail {
    ($kind:ident; $($arg:tt)+) => {
        return Err($crate::error::Error::new($crate::error::ErrorKind::$kind, format!($($arg)+)).into())
    };
    ($($arg:tt)+) => {
        return Err($crate::error::Error::new($crate::error::ErrorKind::Other, format!($($arg)+)).into())
    };
}
pub use crate::bail;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_keeps_the_kind() {
        let e: Result<()> = Err(Error::new(ErrorKind::QuotaExceeded, "no characters left"));
        let e = e.context("cannot translate").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::QuotaExceeded);
        assert_eq!(e.to_string(), "cannot translate");
        assert_eq!(report(&e), "cannot translate: no characters left");
    }

    #[test]
    fn missing_files_are_not_found() {
        let e: Error = std::fs::read("/no/such/file").unwrap_err().into();
        assert_eq!(e.kind(), ErrorKind::NotFound);
    }
}
