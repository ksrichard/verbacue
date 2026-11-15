//! Which verbacue builds fit this computer, which one to recommend, and the release
//! file of each build.

use crate::detect::{Arch, Facts, Os, Version};

/// The oldest NVIDIA driver for the CUDA builds (CUDA 13).
pub const MIN_NVIDIA_DRIVER: Version = (580, 0);
/// The oldest glibc for the Linux builds with the app, and for the Vulkan build.
const GLIBC_APP: Version = (2, 35);
const GLIBC_VULKAN: Version = (2, 39);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// macOS: one universal build with Metal.
    Mac,
    /// The CPU build with the desktop app.
    Cpu,
    /// Linux only: the CPU build without the app, for servers and old systems.
    CpuCommandLine,
    Cuda,
    Vulkan,
}

impl Build {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mac => "macOS (Apple Silicon and Intel, Metal GPU)",
            Self::Cpu => "CPU",
            Self::CpuCommandLine => "CPU, command line only",
            Self::Cuda => "NVIDIA GPU (CUDA)",
            Self::Vulkan => "Any GPU (Vulkan)",
        }
    }

    /// The part of the release file name for this build.
    fn tag(self) -> &'static str {
        match self {
            Self::Mac => "universal",
            Self::Cpu => "cpu",
            Self::CpuCommandLine => "cpu",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
        }
    }
}

impl std::fmt::Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A build and whether it runs on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub build: Build,
    /// `Err` says why the build does not run here.
    pub fits: Result<(), String>,
}

/// Every build for this system, with the reason when one does not fit.
pub fn choices(facts: &Facts) -> Vec<Choice> {
    let choice = |build, fits| Choice { build, fits };
    match facts.os {
        Os::Mac => vec![choice(Build::Mac, Ok(()))],
        Os::Windows => vec![
            choice(Build::Cuda, cuda_fits(facts)),
            choice(Build::Vulkan, vulkan_fits(facts)),
            choice(Build::Cpu, Ok(())),
        ],
        Os::Linux => vec![
            choice(
                Build::Cuda,
                cuda_fits(facts).and(glibc_fits(facts, GLIBC_APP)),
            ),
            choice(
                Build::Vulkan,
                vulkan_fits(facts).and(glibc_fits(facts, GLIBC_VULKAN)),
            ),
            choice(Build::Cpu, glibc_fits(facts, GLIBC_APP)),
            choice(Build::CpuCommandLine, Ok(())),
        ],
        Os::Other => Vec::new(),
    }
}

/// The best build that fits: the first one in the list of [`choices`].
pub fn recommend(facts: &Facts) -> Option<Build> {
    choices(facts)
        .into_iter()
        .find(|c| c.fits.is_ok())
        .map(|c| c.build)
}

fn cuda_fits(facts: &Facts) -> Result<(), String> {
    if facts.arch == Arch::Aarch64 && facts.os == Os::Windows {
        return Err("There is no CUDA build for Windows on Arm.".into());
    }
    match &facts.nvidia {
        None => Err("No NVIDIA GPU found.".into()),
        Some((_, driver)) if *driver < MIN_NVIDIA_DRIVER => Err(format!(
            "The NVIDIA driver is {}.{}; CUDA needs {} or newer.",
            driver.0, driver.1, MIN_NVIDIA_DRIVER.0
        )),
        Some(_) => Ok(()),
    }
}

fn vulkan_fits(facts: &Facts) -> Result<(), String> {
    if facts.vulkan {
        Ok(())
    } else {
        Err("No Vulkan GPU driver found.".into())
    }
}

fn glibc_fits(facts: &Facts, needed: Version) -> Result<(), String> {
    match facts.glibc {
        Some(have) if have >= needed => Ok(()),
        Some(have) => Err(format!(
            "This Linux has glibc {}.{}; this build needs {}.{} or newer.",
            have.0, have.1, needed.0, needed.1
        )),
        None => Err(format!(
            "This build needs glibc {}.{} or newer.",
            needed.0, needed.1
        )),
    }
}

/// The release file of `build` for version `version` (without the "v").
///
/// # Errors
/// Returns an error for a processor that has no build.
pub fn asset_name(build: Build, facts: &Facts, version: &str) -> Result<String, String> {
    let arch = match facts.arch {
        Arch::X86_64 => "amd64",
        Arch::Aarch64 => "arm64",
        Arch::Other => return Err("There is no verbacue build for this processor.".into()),
    };
    Ok(match (facts.os, build) {
        (Os::Mac, _) => format!("verbacue-{version}-macos-universal.dmg"),
        // Windows on Arm runs the x64 build.
        (Os::Windows, b) => format!("verbacue-{version}-windows-x64-{}-setup.exe", b.tag()),
        (Os::Linux, Build::CpuCommandLine) => format!("verbacue-linux-{arch}-cpu.tar.gz"),
        (Os::Linux, Build::Cpu) => format!("verbacue-{version}-linux-{arch}-cpu-gui.AppImage"),
        (Os::Linux, b) => format!("verbacue-{version}-linux-{arch}-{}.AppImage", b.tag()),
        (Os::Other, _) => return Err("There is no verbacue build for this system.".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(nvidia: Option<Version>, vulkan: bool, glibc: Option<Version>) -> Facts {
        Facts {
            os: Os::Linux,
            arch: Arch::X86_64,
            nvidia: nvidia.map(|v| ("RTX".to_string(), v)),
            vulkan,
            glibc,
        }
    }

    #[test]
    fn a_new_nvidia_driver_gets_cuda() {
        let f = linux(Some((580, 65)), true, Some((2, 39)));
        assert_eq!(recommend(&f), Some(Build::Cuda));
    }

    #[test]
    fn an_old_nvidia_driver_gets_vulkan_and_says_why() {
        let f = linux(Some((550, 0)), true, Some((2, 39)));
        assert_eq!(recommend(&f), Some(Build::Vulkan));
        let cuda = &choices(&f)[0];
        assert!(cuda.fits.as_ref().unwrap_err().contains("580"));
    }

    #[test]
    fn vulkan_needs_glibc_2_39_on_linux() {
        let f = linux(None, true, Some((2, 35)));
        assert_eq!(recommend(&f), Some(Build::Cpu));
    }

    #[test]
    fn an_old_or_missing_glibc_gets_the_command_line_build() {
        assert_eq!(
            recommend(&linux(None, false, Some((2, 31)))),
            Some(Build::CpuCommandLine)
        );
        assert_eq!(
            recommend(&linux(None, false, None)),
            Some(Build::CpuCommandLine)
        );
    }

    #[test]
    fn windows_without_a_gpu_gets_the_cpu_build() {
        let f = Facts {
            os: Os::Windows,
            arch: Arch::X86_64,
            nvidia: None,
            vulkan: false,
            glibc: None,
        };
        assert_eq!(recommend(&f), Some(Build::Cpu));
        assert_eq!(
            asset_name(Build::Cpu, &f, "0.2.0").unwrap(),
            "verbacue-0.2.0-windows-x64-cpu-setup.exe"
        );
    }

    #[test]
    fn asset_names_match_the_release_files() {
        let f = linux(None, false, Some((2, 39)));
        assert_eq!(
            asset_name(Build::Cpu, &f, "0.1.0").unwrap(),
            "verbacue-0.1.0-linux-amd64-cpu-gui.AppImage"
        );
        assert_eq!(
            asset_name(Build::Vulkan, &f, "0.1.0").unwrap(),
            "verbacue-0.1.0-linux-amd64-vulkan.AppImage"
        );
        assert_eq!(
            asset_name(Build::CpuCommandLine, &f, "0.1.0").unwrap(),
            "verbacue-linux-amd64-cpu.tar.gz"
        );
        let mac = Facts {
            os: Os::Mac,
            arch: Arch::Aarch64,
            nvidia: None,
            vulkan: false,
            glibc: None,
        };
        assert_eq!(recommend(&mac), Some(Build::Mac));
        assert_eq!(
            asset_name(Build::Mac, &mac, "0.1.0").unwrap(),
            "verbacue-0.1.0-macos-universal.dmg"
        );
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;
    use crate::release;

    /// Every build that the installer can pick has its file in the latest release.
    #[test]
    #[ignore = "reads the latest release from GitHub"]
    fn the_latest_release_has_every_build() {
        let latest = release::latest().unwrap();
        for (os, arch) in [
            (Os::Mac, Arch::Aarch64),
            (Os::Windows, Arch::X86_64),
            (Os::Linux, Arch::X86_64),
            (Os::Linux, Arch::Aarch64),
        ] {
            let facts = Facts {
                os,
                arch,
                nvidia: Some(("RTX".into(), (590, 0))),
                vulkan: true,
                glibc: Some((2, 39)),
            };
            for choice in choices(&facts) {
                let name = asset_name(choice.build, &facts, latest.version()).unwrap();
                assert!(latest.asset(&name).is_some(), "{name}");
            }
        }
        assert!(latest.asset("SHA256SUMS").is_some());
    }
}
