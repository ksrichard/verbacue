//! Subtitles for any video, made on your own computer: speech to text with Whisper,
//! translation with a local language model or an online service, and video output
//! with soft or burned-in subtitles.
//!
//! This crate puts the verbacue crates together as modules. Each step can also be
//! used alone, from this crate with its feature, or from its own crate:
//!
//! | Module | Does | Feature | Crate |
//! |---|---|---|---|
//! | [`audio`] | Reads the audio of any file that FFmpeg can open | `audio` | `verbacue-media` |
//! | [`transcribe`] | Speech to text with Whisper | `transcribe` | `verbacue-transcribe` |
//! | [`subtitle`] | Reads, writes and lays out SRT and WebVTT | always | `verbacue-core` |
//! | [`translate`] | Translates cues with a language model or a service | `local`, `online` | `verbacue-translate` |
//! | [`video`] | Writes MKV with subtitle tracks, or MP4 with burned-in text | `video` | `verbacue-media` |
//! | [`models`] | Known models, the model cache and verified downloads | `download` | `verbacue-core` |
//!
//! All functions block. Whisper, llama.cpp and FFmpeg do their work on the calling
//! thread. From async code, call them with `spawn_blocking`.
//!
//! # Example
//!
//! ```no_run
//! use verbacue::{audio, models, subtitle, transcribe, translate};
//!
//! # fn main() -> verbacue::Result<()> {
//! let video = std::path::Path::new("movie.mkv");
//! let pcm = audio::decode_pcm(video, 0, &())?;
//! let whisper = models::resolve("large-v3", models::Kind::Whisper, &())?;
//! let options = transcribe::Options { model: &whisper, vad_model: None, language: None };
//! let transcript = transcribe::transcribe(&pcm, &options, &())?;
//!
//! let config = translate::Config {
//!     backend: translate::Backend::Local,
//!     target: "hu".into(),
//!     register: translate::Register::Informal,
//!     glossary: Vec::new(),
//!     llm_model: models::DEFAULT_LLM.into(),
//!     endpoint: String::new(),
//!     endpoint_model: None,
//! };
//! let mut translator = translate::create(&config)?;
//! let cues = translate::translate_cues(&mut *translator, &transcript.cues, &transcript.language, &())?;
//! std::fs::write("movie.hu.srt", subtitle::write(&cues, subtitle::Format::Srt))?;
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "audio")]
pub use verbacue_media::audio;
#[cfg(feature = "video")]
pub use verbacue_media::video;
#[cfg(feature = "transcribe")]
pub use verbacue_transcribe as transcribe;
pub use verbacue_translate as translate;

pub use verbacue_core::{Error, ErrorKind, Progress, Result, models, subtitle};
