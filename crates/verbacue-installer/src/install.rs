//! Installs a downloaded release file, without admin rights:
//! - macOS: copies verbacue.app from the DMG to /Applications, else ~/Applications.
//! - Windows: runs the setup program for the current user.
//! - Linux: puts the AppImage in ~/Applications, with a menu entry and an icon, and
//!   links ~/.local/bin/verbacue to it. The command-line build goes to ~/.local/bin.

use crate::error::{Error, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const ICON: &[u8] = include_bytes!("../../../packaging/icons/verbacue-256.png");
const DESKTOP: &str = include_str!("../../../packaging/linux/verbacue.desktop");

/// What the user can do after the install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The app, which the "Open verbacue" button starts.
    App(PathBuf),
    /// Only the command line, at this path.
    CommandLine(PathBuf),
}

/// Installs `file`, a release file from [`crate::choose::asset_name`].
///
/// # Errors
/// Returns an error if a step fails; the text says which one.
pub fn install(file: &Path) -> Result<Installed> {
    let name = file
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().to_lowercase());
    if name.ends_with(".dmg") {
        mac(file)
    } else if name.ends_with("-setup.exe") {
        windows(file)
    } else if name.ends_with(".appimage") {
        app_image(file)
    } else if name.ends_with(".tar.gz") {
        command_line(file)
    } else {
        Err(format!("cannot install {name}").into())
    }
}

/// Starts the installed app.
pub fn open(app: &Path) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(app).spawn()
    } else {
        Command::new(app).spawn()
    };
    if let Err(e) = result {
        eprintln!("cannot start {}: {e}", app.display());
    }
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| Error::context(format!("cannot run {program}"), e))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Failed {
            program: program.to_string(),
            status,
        })
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".into())
}

fn mac(dmg: &Path) -> Result<Installed> {
    let mount = std::env::temp_dir().join("verbacue-installer-dmg");
    let _ = std::fs::create_dir_all(&mount);
    run(
        "hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            dmg.as_os_str(),
        ],
    )?;
    let result = (|| {
        let app = std::fs::read_dir(&mount)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or("the DMG has no app")?;
        let name = app.file_name().ok_or("the app has no name")?.to_owned();
        // /Applications needs admin rights on some Macs; ~/Applications never does.
        let system = PathBuf::from("/Applications");
        let folder = if writable(&system) {
            system
        } else {
            home()?.join("Applications")
        };
        std::fs::create_dir_all(&folder)?;
        let target = folder.join(name);
        if target.exists() {
            std::fs::remove_dir_all(&target)
                .map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
        }
        run("ditto", &[app.as_os_str(), target.as_os_str()])?;
        Ok(Installed::App(target))
    })();
    let _ = run("hdiutil", &["detach".as_ref(), mount.as_os_str()]);
    result
}

/// True when a file can be created in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".verbacue-installer-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn windows(setup: &Path) -> Result<Installed> {
    // Inno Setup: install for the current user only (no admin rights), show only the
    // progress, and do not restart Windows.
    let status = Command::new(setup)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"])
        .status()
        .map_err(|e| Error::context("cannot run the setup", e))?;
    if !status.success() {
        return Err(Error::Failed {
            program: "the setup".into(),
            status,
        });
    }
    // The default folder of a per-user Inno Setup install ({autopf}).
    let folder = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA is not set")?
        .join("Programs")
        .join("verbacue");
    Ok(Installed::App(folder.join("verbacue.exe")))
}

fn app_image(file: &Path) -> Result<Installed> {
    let home = home()?;
    let target = home.join("Applications").join("verbacue.AppImage");
    copy_executable(file, &target)?;

    let icons = home.join(".local/share/icons/hicolor/256x256/apps");
    std::fs::create_dir_all(&icons)?;
    std::fs::write(icons.join("verbacue.png"), ICON)?;

    let menu = home.join(".local/share/applications");
    std::fs::create_dir_all(&menu)?;
    std::fs::write(menu.join("verbacue.desktop"), desktop_entry(&target))?;

    link_command(&home, &target)?;
    Ok(Installed::App(target))
}

/// The menu entry of the packages, with the AppImage as the program.
pub fn desktop_entry(app_image: &Path) -> String {
    DESKTOP
        .lines()
        .map(|line| {
            if line.starts_with("Exec=") {
                format!("Exec=\"{}\" gui", app_image.display())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn command_line(archive: &Path) -> Result<Installed> {
    let home = home()?;
    let unpacked = std::env::temp_dir().join("verbacue-installer-unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked)?;
    run(
        "tar",
        &[
            "-xzf".as_ref(),
            archive.as_os_str(),
            "-C".as_ref(),
            unpacked.as_os_str(),
        ],
    )?;
    let target = home.join(".local/bin/verbacue");
    copy_executable(&unpacked.join("verbacue"), &target)?;
    let _ = std::fs::remove_dir_all(&unpacked);
    Ok(Installed::CommandLine(target))
}

/// Copies `from` to `to` and makes it executable. A running old copy is replaced, not
/// overwritten, so it keeps working.
fn copy_executable(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = to.with_extension("new");
    std::fs::copy(from, &temp).map_err(|e| format!("cannot copy to {}: {e}", to.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(std::fs::rename(&temp, to)?)
}

/// Links ~/.local/bin/verbacue to the AppImage, so `verbacue` works in a terminal.
fn link_command(home: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin)?;
        let link = bin.join("verbacue");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(target, &link)?;
    }
    #[cfg(not(unix))]
    let _ = (home, target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_entry_starts_the_app_image() {
        let entry = desktop_entry(Path::new("/home/ana/Applications/verbacue.AppImage"));
        assert!(entry.contains("Exec=\"/home/ana/Applications/verbacue.AppImage\" gui\n"));
        assert!(entry.contains("Icon=verbacue\n"));
        assert!(!entry.contains("Exec=verbacue gui"));
    }

    #[test]
    fn copy_executable_replaces_the_old_file() {
        let dir = std::env::temp_dir().join("verbacue-installer-copy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("new");
        let to = dir.join("bin/verbacue");
        std::fs::write(&from, b"new").unwrap();
        copy_executable(&from, &to).unwrap();
        std::fs::write(&from, b"newer").unwrap();
        copy_executable(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"newer");
    }
}
