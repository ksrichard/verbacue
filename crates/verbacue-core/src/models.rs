//! Model files: a fixed list of known models, a local cache, and verified downloads.
//!
//! Downloads go to `<file>.part` first. An interrupted download continues from the
//! end of that file with an HTTP `Range` request. The file gets its final name only
//! after its SHA256 matches, so a file with the final name is always complete.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use crate::{
    Progress,
    error::{Context as _, Result, bail},
};

/// What a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Speech to text.
    Whisper,
    /// Voice activity detection for Whisper.
    Vad,
    /// A translation model (GGUF).
    Llm,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Whisper => "whisper",
            Kind::Vad => "vad",
            Kind::Llm => "llm",
        })
    }
}

/// A known model: where to download it and how to check it.
#[derive(Debug)]
pub struct ModelSpec {
    pub name: &'static str,
    pub kind: Kind,
    /// The file name in the cache.
    pub file: &'static str,
    pub url: &'static str,
    /// The SHA256 of the file, as lowercase hex.
    pub sha256: &'static str,
    /// The file size in bytes.
    pub size: u64,
}

macro_rules! whisper {
    ($name:literal, $sha:literal, $size:literal) => {
        ModelSpec {
            name: $name,
            kind: Kind::Whisper,
            file: concat!("ggml-", $name, ".bin"),
            url: concat!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-",
                $name,
                ".bin"
            ),
            sha256: $sha,
            size: $size,
        }
    };
}

/// Hashes are the Git LFS object ids that Hugging Face publishes for each file.
const MODELS: &[ModelSpec] = &[
    whisper!(
        "tiny",
        "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        77691713
    ),
    whisper!(
        "base",
        "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        147951465
    ),
    whisper!(
        "small",
        "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        487601967
    ),
    whisper!(
        "medium",
        "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        1533763059
    ),
    whisper!(
        "large-v3",
        "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        3095033483
    ),
    whisper!(
        "large-v3-turbo",
        "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        1624555275
    ),
    ModelSpec {
        name: "silero-vad",
        kind: Kind::Vad,
        file: "ggml-silero-v5.1.2.bin",
        url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin",
        sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
        size: 885098,
    },
    ModelSpec {
        name: "gemma-3-12b",
        kind: Kind::Llm,
        file: "gemma-3-12b-it-Q4_K_M.gguf",
        url: "https://huggingface.co/ggml-org/gemma-3-12b-it-GGUF/resolve/main/gemma-3-12b-it-Q4_K_M.gguf",
        sha256: "7bb69bff3f48a7b642355d64a90e481182a7794707b3133890646b1efa778ff5",
        size: 7300574976,
    },
    // Gemma 3 12B trained by Google for translation. It made the fewest errors in
    // Hungarian and no wrong-script words in Hindi, Arabic, Japanese and Thai.
    ModelSpec {
        name: "translategemma-12b",
        kind: Kind::Llm,
        file: "translategemma-12b-it.Q4_K_M.gguf",
        url: "https://huggingface.co/mradermacher/translategemma-12b-it-GGUF/resolve/main/translategemma-12b-it.Q4_K_M.gguf",
        sha256: "b7aac4b4be7ab0c49b6556c29c4467e74313df7f1e95d9f9676bb2adf0afa528",
        size: 7300794112,
    },
    // IQ4_XS, not Q4_K_M: Q4_K_M (16.5 GB) leaves too little GPU memory on a 24 GB Mac.
    ModelSpec {
        name: "translategemma-27b",
        kind: Kind::Llm,
        file: "translategemma-27b-it.IQ4_XS.gguf",
        url: "https://huggingface.co/mradermacher/translategemma-27b-it-GGUF/resolve/main/translategemma-27b-it.IQ4_XS.gguf",
        sha256: "910ee24bce3751dcdf89711e155227ca8f04b5952897cda3ee1d92937963fb68",
        size: 14893907040,
    },
    // About twice as fast, but it needs about 14 GB of GPU memory and puts more words
    // in the wrong language.
    ModelSpec {
        name: "gemma-4-26b-a4b",
        kind: Kind::Llm,
        file: "gemma-4-26B-A4B-it-UD-IQ4_XS.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-26B-A4B-it-GGUF/resolve/main/gemma-4-26B-A4B-it-UD-IQ4_XS.gguf",
        sha256: "babd1e389d386352f71600765d37390f7dc993fbfad6725caccf996ffe34aecf",
        size: 13597177568,
    },
];

// large-v3 is about 2.5 times slower than large-v3-turbo, but it makes fewer errors in
// languages other than English.
pub const DEFAULT_WHISPER: &str = "large-v3";
pub const DEFAULT_LLM: &str = "translategemma-12b";
pub const VAD: &str = "silero-vad";

/// Returns `<cache dir>/verbacue/models`, for example `~/Library/Caches/verbacue/models`.
///
/// # Errors
/// Returns an error if the OS has no cache directory.
pub fn cache_dir() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("no cache directory on this system")?
        .join("verbacue/models"))
}

/// The model with this name.
pub fn find(name: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.name == name)
}

/// All known models.
pub fn known() -> &'static [ModelSpec] {
    MODELS
}

/// The cache path of a model. The file may not exist yet.
///
/// # Errors
/// Returns an error if the OS has no cache directory.
pub fn cached_path(spec: &ModelSpec) -> Result<PathBuf> {
    Ok(cache_dir()?.join(spec.file))
}

/// Turns a model value (a file path or a known model name) into a local file path.
///
/// An existing file path is used as it is. A known model name is downloaded on first
/// use (with the `download` feature). Any other value is an error that lists the known
/// names.
///
/// # Errors
/// Returns an error for an unknown name, or if the download fails.
pub fn resolve(value: &str, kind: Kind, progress: &dyn Progress) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    match find(value) {
        Some(spec) if spec.kind == kind => ensure(spec, progress),
        _ => {
            let names: Vec<&str> = MODELS
                .iter()
                .filter(|m| m.kind == kind)
                .map(|m| m.name)
                .collect();
            bail!(
                NotFound;
                "{value:?} is not a file or a known model ({})",
                names.join(", ")
            )
        }
    }
}

/// Returns the cached path of a model and downloads it first if it is not there.
/// `progress` receives bytes.
///
/// # Errors
/// Returns an error if the download or the checksum check fails, or if the model is
/// not in the cache and this build cannot download (no `download` feature).
pub fn ensure(spec: &'static ModelSpec, progress: &dyn Progress) -> Result<PathBuf> {
    let dir = cache_dir()?;
    let path = dir.join(spec.file);
    if path.is_file() {
        return Ok(path);
    }
    #[cfg(feature = "download")]
    {
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        download::download(spec, &path, progress)?;
        Ok(path)
    }
    #[cfg(not(feature = "download"))]
    {
        let _ = progress;
        bail!(NotFound; "{} is not in the cache, and this build cannot download", spec.name)
    }
}

#[cfg(feature = "download")]
mod download {
    use std::{
        fs::{File, OpenOptions},
        io::{Read, Write},
        path::{Path, PathBuf},
    };

    use reqwest::{StatusCode, header};
    use sha2::{Digest, Sha256};

    use super::{ModelSpec, human_size};
    use crate::{
        Progress,
        error::{Context as _, Result, bail},
    };

    pub(super) fn download(spec: &ModelSpec, path: &Path, progress: &dyn Progress) -> Result<()> {
        let part = PathBuf::from(format!("{}.part", path.display()));
        let mut have = std::fs::metadata(&part).map_or(0, |m| m.len());
        if have > spec.size {
            std::fs::remove_file(&part)?;
            have = 0;
        }

        tracing::info!("downloading {} ({})", spec.name, human_size(spec.size));
        // No total timeout: a large model on a slow line takes hours.
        let mut request = crate::http::client_builder()
            .timeout(None)
            .build()?
            .get(spec.url);
        if have > 0 {
            request = request.header(header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request
            .send()
            .with_context(|| format!("cannot download {}", spec.url))?;
        let status = response.status();
        let append =
            status == StatusCode::PARTIAL_CONTENT || status == StatusCode::RANGE_NOT_SATISFIABLE;
        if !append && !status.is_success() {
            bail!(Network; "download of {} failed: HTTP {status}", spec.url);
        }
        // A full 200 response means the server ignored the range, so start the file again.
        let mut file = if append {
            OpenOptions::new().append(true).open(&part)?
        } else {
            have = 0;
            File::create(&part)?
        };

        progress.update(have, spec.size);
        if status != StatusCode::RANGE_NOT_SATISFIABLE {
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = response
                    .read(&mut buf)
                    .context("download interrupted; run again to resume")?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])?;
                have += n as u64;
                progress.update(have, spec.size);
            }
        }
        file.flush()?;
        drop(file);

        tracing::info!("verifying {}", spec.name);
        let actual = sha256_file(&part)?;
        if actual != spec.sha256 {
            std::fs::remove_file(&part)?;
            bail!(
                "checksum mismatch for {}: expected {}, got {actual}",
                spec.name,
                spec.sha256
            );
        }
        std::fs::rename(&part, path)?;
        Ok(())
    }

    fn sha256_file(path: &Path) -> Result<String> {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
}

/// Deletes a cached model, and its partial download if one exists.
///
/// # Errors
/// Returns an error for an unknown name, if the model is not in the cache, or if the
/// file cannot be removed.
pub fn remove(name: &str) -> Result<()> {
    let Some(spec) = find(name) else {
        bail!(NotFound; "unknown model {name:?}");
    };
    let path = cached_path(spec)?;
    let mut removed = false;
    for p in [
        path.clone(),
        PathBuf::from(format!("{}.part", path.display())),
    ] {
        if p.is_file() {
            fs::remove_file(&p).with_context(|| format!("cannot remove {}", p.display()))?;
            removed = true;
        }
    }
    if !removed {
        bail!(NotFound; "{name} is not downloaded");
    }
    Ok(())
}

/// A file size for people, for example "3.1 GB".
pub fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_unique_and_hashes_look_valid() {
        for (i, m) in MODELS.iter().enumerate() {
            assert!(
                MODELS[i + 1..].iter().all(|o| o.name != m.name),
                "duplicate {}",
                m.name
            );
            assert_eq!(m.sha256.len(), 64);
            assert!(m.url.ends_with(m.file));
        }
        assert!(find(DEFAULT_WHISPER).is_some());
        assert!(find(DEFAULT_LLM).is_some());
        assert!(find(VAD).is_some());
    }

    #[test]
    fn resolve_rejects_wrong_kind() {
        let e = resolve("gemma-3-12b", Kind::Whisper, &()).unwrap_err();
        assert_eq!(e.kind(), crate::ErrorKind::NotFound);
        assert!(resolve("no-such-model", Kind::Llm, &()).is_err());
    }
}
