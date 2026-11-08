//! Media input and output for verbacue, with a statically linked FFmpeg.
//!
//! | Module | Does | Feature |
//! |---|---|---|
//! | [`audio`] | Reads the audio of any file that FFmpeg can open, as 16 kHz mono PCM | always |
//! | [`video`] | Writes MKV with subtitle tracks, or MP4 with burned-in text | `video` |
//!
//! All functions block. From async code, call them with `spawn_blocking`.

#[cfg(all(feature = "video", any(target_os = "macos", windows)))]
mod hwdecode;

pub mod audio;
#[cfg(feature = "video")]
pub mod video;
