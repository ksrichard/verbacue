//! The verbacue config folder.

use std::path::PathBuf;

/// The folder for verbacue settings and service keys: `~/.config/verbacue` on macOS and
/// Linux, `%APPDATA%\verbacue` on Windows. `None` if the system has no home folder.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let dir = dirs::config_dir().map(|d| d.join("verbacue"));
    #[cfg(not(windows))]
    let dir = dirs::home_dir().map(|home| home.join(".config/verbacue"));
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_ends_with_verbacue() {
        assert!(config_dir().unwrap().ends_with("verbacue"));
    }
}
