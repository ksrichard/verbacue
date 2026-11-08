//! The error type of all verbacue crates.
//!
//! An [`Error`] has a [`kind`](Error::kind), so callers can react to some problems (for
//! example a used-up quota), and a chain of messages for people. `{}` shows the
//! outermost message; `{:#}` shows the whole chain.

use std::fmt::{self, Display};

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
    /// Any other problem, for example a file that cannot be written.
    Other,
}

/// The error type of this crate.
pub struct Error {
    kind: ErrorKind,
    inner: anyhow::Error,
}

/// The result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Makes an error with a message.
    pub fn new(kind: ErrorKind, message: impl Display + Send + Sync + 'static) -> Self {
        Self {
            kind,
            inner: anyhow::Error::msg(message.to_string()),
        }
    }

    /// Makes an error from another error. Its message becomes the message of this one.
    pub fn wrap(kind: ErrorKind, error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            kind,
            inner: anyhow::Error::new(error),
        }
    }

    /// The kind of problem.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    fn context(self, message: impl Display + Send + Sync + 'static) -> Self {
        Self {
            kind: self.kind,
            inner: self.inner.context(message),
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(f, "{:#}", self.inner)
        } else {
            write!(f, "{}", self.inner)
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} ({:?})", self.inner, self.kind)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.chain().nth(1)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        let kind = match e.kind() {
            std::io::ErrorKind::NotFound => ErrorKind::NotFound,
            _ => ErrorKind::Other,
        };
        Self::wrap(kind, e)
    }
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

from_errors!(ErrorKind::Other =>
    serde_json::Error,
    std::fmt::Error,
    std::str::Utf8Error,
    std::string::FromUtf8Error,
    std::num::ParseIntError,
);

#[cfg(feature = "http")]
from_errors!(ErrorKind::Network => reqwest::Error);

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

/// Adds a message to an error, the same as anyhow's `Context`.
pub trait Context<T> {
    /// Adds a message to the error, or turns `None` into an error with this message.
    ///
    /// # Errors
    ///
    /// Returns the error of `self` with the message added.
    fn context<C: Display + Send + Sync + 'static>(self, message: C) -> Result<T>;
    /// The same as [`context`](Context::context), but makes the message only when
    /// there is an error.
    ///
    /// # Errors
    ///
    /// Returns the error of `self` with the message added.
    fn with_context<C: Display + Send + Sync + 'static, F: FnOnce() -> C>(
        self,
        message: F,
    ) -> Result<T>;
}

impl<T, E: Into<Error>> Context<T> for std::result::Result<T, E> {
    fn context<C: Display + Send + Sync + 'static>(self, message: C) -> Result<T> {
        self.map_err(|e| e.into().context(message))
    }

    fn with_context<C: Display + Send + Sync + 'static, F: FnOnce() -> C>(
        self,
        message: F,
    ) -> Result<T> {
        self.map_err(|e| e.into().context(message()))
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: Display + Send + Sync + 'static>(self, message: C) -> Result<T> {
        self.ok_or_else(|| Error::new(ErrorKind::Other, message))
    }

    fn with_context<C: Display + Send + Sync + 'static, F: FnOnce() -> C>(
        self,
        message: F,
    ) -> Result<T> {
        self.ok_or_else(|| Error::new(ErrorKind::Other, message()))
    }
}

/// Returns an error. `bail!(Kind; "...")` sets the kind; plain `bail!("...")` uses
/// [`ErrorKind::Other`].
#[macro_export]
macro_rules! bail {
    ($kind:ident; $($arg:tt)+) => {
        return Err($crate::error::Error::new($crate::error::ErrorKind::$kind, format!($($arg)+)))
    };
    ($($arg:tt)+) => {
        return Err($crate::error::Error::new($crate::error::ErrorKind::Other, format!($($arg)+)))
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
        assert_eq!(format!("{e:#}"), "cannot translate: no characters left");
    }

    #[test]
    fn missing_files_are_not_found() {
        let e: Error = std::fs::read("/no/such/file").unwrap_err().into();
        assert_eq!(e.kind(), ErrorKind::NotFound);
    }
}
