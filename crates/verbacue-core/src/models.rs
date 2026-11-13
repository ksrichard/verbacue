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
    /// A file of the text-to-speech model, for dubbing.
    Voice,
    /// A file of the model that takes the voices out of the audio, for dubbing.
    Separation,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Whisper => "whisper",
            Kind::Vad => "vad",
            Kind::Llm => "llm",
            Kind::Voice => "voice",
            Kind::Separation => "separation",
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

// The ONNX exports from tools/xtts-export. The cache keeps each bundle in its own folder.
// TODO: pin the URLs to the upload commit, as publish.md says, when the files are published.
macro_rules! voice_file {
    ($kind:expr, $file:literal, $sha:literal, $size:literal) => {
        ModelSpec {
            name: $file,
            kind: $kind,
            file: $file,
            url: concat!(
                "https://huggingface.co/richardbertok/verbacue-voice/resolve/main/",
                $file
            ),
            sha256: $sha,
            size: $size,
        }
    };
}

const BUNDLE_FILES: &[ModelSpec] = &[
    voice_file!(
        Kind::Voice,
        "xtts-v2/conditioning.onnx",
        "aff301386f5eb1c54f44bffc5c4d535f330ef82309a3c2743763f30d86afcb26",
        185360140
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/speaker.onnx",
        "c2c912737091006adace0e76e4e1831ade0d8b063ffe706dd29a8ae41d200994",
        32214928
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/gpt.onnx",
        "4c9bf9662962a84af563fd7cca21e061a020a13365f189229d4b6827ca1dd90d",
        1516406822
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/gpt_int8.onnx",
        "854d85e1d492beb03e9af4ce81eb7bcef5e9c3097af4bb3bd6bb9cb52ec84fc8",
        382595950
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/vocoder.onnx",
        "6fa9396258f7dd36e26894a1abca36c222e9588118e309fa7824039f0c4cfae2",
        71335407
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/embeddings.safetensors",
        "acaa255cb35393bb95d43da38118fba3d382103070c12d1b84f98836a651eed5",
        35713384
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/speakers.safetensors",
        "3d02a5681d1b775033a985bb16ab6409c7afe048e489eab35d9a2bd17c7ef521",
        7731832
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/vocab.json",
        "928260878a59da8a72a2a5b7687fea29d5106137669d90945430fe17e415304a",
        361219
    ),
    voice_file!(
        Kind::Voice,
        "xtts-v2/config.json",
        "7ff7649f6bb52a0dfff23d5931f05e73d18380302f41eb110f37ed31030b98ac",
        3210
    ),
    voice_file!(
        Kind::Separation,
        "htdemucs/htdemucs_vocals.onnx",
        "acc80e2c5f47c01579f2871f17ef424f3ff4d5fa5aa7963b8c94b9af80d215db",
        316446953
    ),
];

/// A model that has more than one file. All files are in one cache folder.
#[derive(Debug)]
pub struct Bundle {
    pub name: &'static str,
    pub kind: Kind,
    /// The names of the files, as `find` knows them.
    pub files: &'static [&'static str],
    /// The licence that the user must accept before the download.
    pub licence: Option<&'static str>,
}

pub const XTTS_LICENCE: &str = "XTTS-v2 is under the Coqui Public Model License: \
non-commercial use only. See https://coqui.ai/cpml";

const BUNDLES: &[Bundle] = &[
    Bundle {
        name: "xtts-v2",
        kind: Kind::Voice,
        files: &[
            "xtts-v2/conditioning.onnx",
            "xtts-v2/speaker.onnx",
            "xtts-v2/gpt.onnx",
            "xtts-v2/vocoder.onnx",
            "xtts-v2/embeddings.safetensors",
            "xtts-v2/speakers.safetensors",
            "xtts-v2/vocab.json",
            "xtts-v2/config.json",
        ],
        licence: Some(XTTS_LICENCE),
    },
    // About 4 times smaller and faster on a CPU, with a small loss in quality.
    Bundle {
        name: "xtts-v2-int8",
        kind: Kind::Voice,
        files: &[
            "xtts-v2/conditioning.onnx",
            "xtts-v2/speaker.onnx",
            "xtts-v2/gpt_int8.onnx",
            "xtts-v2/vocoder.onnx",
            "xtts-v2/embeddings.safetensors",
            "xtts-v2/speakers.safetensors",
            "xtts-v2/vocab.json",
            "xtts-v2/config.json",
        ],
        licence: Some(XTTS_LICENCE),
    },
    Bundle {
        name: "htdemucs-vocals",
        kind: Kind::Separation,
        files: &["htdemucs/htdemucs_vocals.onnx"],
        licence: None,
    },
];

impl Bundle {
    /// The total size of all files in bytes.
    pub fn size(&self) -> u64 {
        self.specs().map(|s| s.size).sum()
    }

    fn specs(&self) -> impl Iterator<Item = &'static ModelSpec> + '_ {
        self.files
            .iter()
            .map(|f| find(f).expect("bundle file is in BUNDLE_FILES"))
    }

    /// The cache folder of the bundle. The files may not exist yet.
    ///
    /// # Errors
    /// Returns an error if the OS has no cache directory.
    pub fn cached_dir(&self) -> Result<PathBuf> {
        let first = self.files[0];
        let folder = first.split_once('/').map_or(first, |(d, _)| d);
        Ok(cache_dir()?.join(folder))
    }

    /// True if all files are in the cache.
    pub fn is_cached(&self) -> bool {
        self.specs()
            .all(|s| cached_path(s).is_ok_and(|p| p.is_file()))
    }
}

/// The bundle with this name.
pub fn bundle(name: &str) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| b.name == name)
}

/// All known bundles.
pub fn bundles() -> &'static [Bundle] {
    BUNDLES
}

/// Downloads the missing files of a bundle and returns its cache folder. `progress`
/// receives bytes for each file in turn.
///
/// # Errors
/// Returns an error if a download or a checksum check fails.
pub fn ensure_bundle(b: &'static Bundle, progress: &dyn Progress) -> Result<PathBuf> {
    for spec in b.specs() {
        ensure(spec, progress)?;
    }
    b.cached_dir()
}

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

/// The model or bundle file with this name.
pub fn find(name: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().chain(BUNDLE_FILES).find(|m| m.name == name)
}

/// All known single-file models. `bundles` lists the models that have more files.
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
        let parent = path.parent().unwrap_or(&dir);
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
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
        Progress, check_cancel,
        error::{Context as _, Result, bail},
    };

    pub(super) fn download(spec: &ModelSpec, path: &Path, progress: &dyn Progress) -> Result<()> {
        check_cancel(progress)?;
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
                // A cancelled download keeps its .part file, so the next run continues it.
                check_cancel(progress)?;
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

/// Deletes a cached model or bundle, and the partial downloads if they exist.
///
/// A bundle shares files with other bundles (`xtts-v2` and `xtts-v2-int8`), so removing
/// one bundle also removes the shared files.
///
/// # Errors
/// Returns an error for an unknown name, if the model is not in the cache, or if a
/// file cannot be removed.
pub fn remove(name: &str) -> Result<()> {
    let specs: Vec<&ModelSpec> = match (find(name), bundle(name)) {
        (Some(spec), _) => vec![spec],
        (None, Some(b)) => b.specs().collect(),
        (None, None) => bail!(NotFound; "unknown model {name:?}"),
    };
    let mut removed = false;
    for spec in specs {
        let path = cached_path(spec)?;
        for p in [
            path.clone(),
            PathBuf::from(format!("{}.part", path.display())),
        ] {
            if p.is_file() {
                fs::remove_file(&p).with_context(|| format!("cannot remove {}", p.display()))?;
                removed = true;
            }
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

    #[cfg(feature = "download")]
    #[test]
    fn download_checks_cancel_before_the_request() {
        struct Stopped;
        impl Progress for Stopped {
            fn update(&self, _done: u64, _total: u64) {}
            fn cancelled(&self) -> bool {
                true
            }
        }
        // Port 9 (discard) is closed on test machines, so a request fails fast.
        let spec = ModelSpec {
            name: "test",
            kind: Kind::Whisper,
            file: "test.bin",
            url: "http://127.0.0.1:9/test.bin",
            sha256: "",
            size: 10,
        };
        let path = std::env::temp_dir().join("verbacue-cancel-test.bin");
        let err = download::download(&spec, &path, &Stopped).unwrap_err();
        assert_eq!(err.kind(), crate::ErrorKind::Cancelled);
    }

    #[test]
    fn model_names_are_unique_and_hashes_look_valid() {
        let all: Vec<&ModelSpec> = MODELS.iter().chain(BUNDLE_FILES).collect();
        for (i, m) in all.iter().enumerate() {
            assert!(
                all[i + 1..].iter().all(|o| o.name != m.name),
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
    fn xtts_bundle_lists_all_files() {
        let names = |b: &Bundle| -> Vec<&str> {
            b.files
                .iter()
                .map(|f| f.rsplit('/').next().unwrap())
                .collect()
        };
        let fp32 = bundle("xtts-v2").unwrap();
        let int8 = bundle("xtts-v2-int8").unwrap();
        let common = [
            "conditioning.onnx",
            "speaker.onnx",
            "vocoder.onnx",
            "embeddings.safetensors",
            "speakers.safetensors",
            "vocab.json",
            "config.json",
        ];
        for f in common {
            assert!(names(fp32).contains(&f), "{f}");
            assert!(names(int8).contains(&f), "{f}");
        }
        assert!(names(fp32).contains(&"gpt.onnx"));
        assert!(!names(int8).contains(&"gpt.onnx"));
        assert!(names(int8).contains(&"gpt_int8.onnx"));
        for b in [fp32, int8, bundle("htdemucs-vocals").unwrap()] {
            for f in b.files {
                assert!(find(f).is_some(), "{f}");
            }
        }
        assert!(fp32.licence.is_some());
        assert!(int8.licence.is_some());
        assert!(bundle("htdemucs-vocals").unwrap().licence.is_none());
    }

    #[test]
    fn resolve_rejects_wrong_kind() {
        let e = resolve("gemma-3-12b", Kind::Whisper, &()).unwrap_err();
        assert_eq!(e.kind(), crate::ErrorKind::NotFound);
        assert!(resolve("no-such-model", Kind::Llm, &()).is_err());
    }
}
