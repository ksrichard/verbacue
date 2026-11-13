//! Dubbing: speaks the translated subtitles in a new voice track.
//!
//! XTTS-v2 makes the speech and HTDemucs takes the original voices out of the audio.
//! Both run as ONNX graphs with ONNX Runtime. `tools/xtts-export` makes the graphs.

pub mod bpe;
pub mod numbers;
pub mod romanize;
pub mod session;
pub mod text;

pub use text::{LANGUAGES, supports};
