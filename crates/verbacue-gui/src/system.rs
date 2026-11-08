//! Opens files with the programs of the system.

use std::{path::Path, process::Command};

/// Opens `path` with its default program, for example a video player.
pub fn open(path: &Path) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
    } else {
        Command::new("xdg-open").arg(path).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("cannot open {}: {e}", path.display());
    }
}
