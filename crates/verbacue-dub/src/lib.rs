//! Dubbing: speaks the translated subtitles in a new voice track.
//!
//! XTTS-v2 makes the speech and HTDemucs takes the original voices out of the audio.
//! Both run as ONNX graphs with ONNX Runtime. `tools/xtts-export` makes the graphs.

pub mod bpe;
pub mod fit;
#[cfg(old_glibc)]
mod glibc;
pub mod mel;
pub mod mix;
pub mod numbers;
pub mod resample;
pub mod romanize;
pub mod run;
pub mod safetensors;
pub mod sampling;
pub mod separate;
pub mod session;
pub mod speakers;
pub mod text;
pub mod wav;
pub mod xtts;

pub use run::{DubJob, DubStep, Events, Options, Original, VoiceChoice, default_model, dub};
pub use text::{LANGUAGES, supports};
pub use xtts::{DEFAULT_STUDIO_VOICE, Latents, SAMPLE_RATE, Voice, Xtts};
