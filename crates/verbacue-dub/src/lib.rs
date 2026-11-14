//! Dubbing: speaks the translated subtitles in a new voice track.
//!
//! XTTS-v2 makes the speech and HTDemucs takes the original voices out of the audio.
//! Both run as ONNX graphs with ONNX Runtime. `tools/xtts-export` makes the graphs.

pub mod bpe;
pub mod fit;
pub mod mel;
pub mod numbers;
pub mod resample;
pub mod romanize;
pub mod safetensors;
pub mod sampling;
pub mod session;
pub mod text;
pub mod wav;
pub mod xtts;

pub use text::{LANGUAGES, supports};
pub use xtts::{DEFAULT_STUDIO_VOICE, Latents, SAMPLE_RATE, Voice, Xtts};
