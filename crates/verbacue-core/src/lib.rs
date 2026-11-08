//! The shared parts of the verbacue crates. This crate is pure Rust and has no C
//! libraries.
//!
//! | Module | Does |
//! |---|---|
//! | [`error`] | The [`Error`] type of all verbacue crates, with an [`ErrorKind`] |
//! | [`subtitle`] | Reads, writes and lays out SRT and WebVTT cues |
//! | [`languages`] | The languages that Whisper knows, with their English names |
//! | [`models`] | Known models, the model cache and verified downloads (`download`) |
//! | [`http`] | A blocking HTTP client with built-in root certificates (`http`) |
//!
//! [`Progress`] receives progress reports from all long tasks.

pub mod error;
#[cfg(feature = "http")]
pub mod http;
pub mod languages;
pub mod models;
mod progress;
pub mod subtitle;

pub use error::{Context, Error, ErrorKind, Result, WithKind};
pub use progress::Progress;
