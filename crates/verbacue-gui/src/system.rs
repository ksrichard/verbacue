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

/// Shows `path` in the file manager, selected where the system allows it.
pub fn show_in_folder(path: &Path) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(windows) {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path);
        Command::new("explorer").arg(select).spawn()
    } else {
        let folder = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(folder).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("cannot show {}: {e}", path.display());
    }
}
