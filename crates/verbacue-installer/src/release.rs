//! The latest verbacue release on GitHub: its files, the download, and the SHA256
//! check against the SHA256SUMS file of the release.

use crate::error::{Error, Result};
use std::fmt::Write as _;
use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const LATEST: &str = "https://api.github.com/repos/ksrichard/verbacue/releases/latest";

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

impl Release {
    /// The version without the "v" of the tag.
    pub fn version(&self) -> &str {
        self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name)
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    verbacue_core::http::client_builder()
        // GitHub needs a user agent for its API.
        .user_agent(concat!("verbacue-installer/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(Error::from)
}

/// Reads the latest release from GitHub.
///
/// # Errors
/// Returns an error without a network connection, or if GitHub refuses the request.
pub fn latest() -> Result<Release> {
    client()?
        .get(LATEST)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::json)
        .map_err(|e| Error::context("cannot read the latest release", e))
}

/// Downloads `asset` into `dir`, checks it against the SHA256SUMS file of `release`,
/// and returns its path. `progress` gets the bytes done and the total.
///
/// # Errors
/// Returns an error if the download fails or the SHA256 does not match.
pub fn download(
    release: &Release,
    asset: &Asset,
    dir: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<PathBuf> {
    let client = client()?;
    let sums = release
        .asset("SHA256SUMS")
        .ok_or("the release has no SHA256SUMS file")?;
    let sums = client
        .get(&sums.browser_download_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::text)
        .map_err(|e| Error::context("cannot read SHA256SUMS", e))?;
    let expected = expected_sha256(&sums, &asset.name)
        .ok_or_else(|| format!("SHA256SUMS has no line for {}", asset.name))?;

    std::fs::create_dir_all(dir)?;
    let path = dir.join(&asset.name);
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("cannot download {}: {e}", asset.name))?;
    let mut file = std::fs::File::create(&path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut done = 0;
    loop {
        let n = response.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n])?;
        done += n as u64;
        progress(done, asset.size);
    }
    let actual: String = hasher.finalize().iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    if actual != expected {
        let _ = std::fs::remove_file(&path);
        return Err(Error::Damaged {
            file: asset.name.clone(),
        });
    }
    Ok(path)
}

/// The SHA256 of `name` in a SHA256SUMS file ("<hash>  <name>" lines).
pub fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256sums_gives_the_hash_of_a_file() {
        let sums =
            "abc  verbacue-0.1.0-macos-universal.dmg\nDEF *verbacue-linux-amd64-cpu.tar.gz\n";
        assert_eq!(
            expected_sha256(sums, "verbacue-0.1.0-macos-universal.dmg").as_deref(),
            Some("abc")
        );
        assert_eq!(
            expected_sha256(sums, "verbacue-linux-amd64-cpu.tar.gz").as_deref(),
            Some("def")
        );
        assert_eq!(expected_sha256(sums, "missing"), None);
    }

    #[test]
    fn the_version_has_no_v() {
        let r = Release {
            tag_name: "v0.2.0".into(),
            assets: Vec::new(),
        };
        assert_eq!(r.version(), "0.2.0");
    }
}
