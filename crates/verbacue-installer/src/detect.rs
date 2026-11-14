//! Finds the facts about this computer that decide which build fits: the system, the
//! processor, the NVIDIA driver, Vulkan and, on Linux, the glibc version.

use std::{path::Path, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

/// A version such as "580.65" as (580, 65).
pub type Version = (u32, u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub os: Os,
    pub arch: Arch,
    /// The name of the NVIDIA GPU and its driver version, if `nvidia-smi` finds one.
    pub nvidia: Option<(String, Version)>,
    /// A Vulkan driver for a real GPU is installed (not only the software one).
    pub vulkan: bool,
    /// The glibc version on Linux. `None` on other systems, or without glibc.
    pub glibc: Option<Version>,
}

/// Reads the facts of this computer. It runs small system tools, so it takes up to a
/// second.
pub fn facts() -> Facts {
    let os = match std::env::consts::OS {
        "macos" => Os::Mac,
        "linux" => Os::Linux,
        "windows" => Os::Windows,
        _ => Os::Other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        _ => Arch::Other,
    };
    let nvidia = if os == Os::Mac {
        None
    } else {
        output(
            "nvidia-smi",
            &["--query-gpu=name,driver_version", "--format=csv,noheader"],
        )
        .and_then(|text| parse_nvidia(&text))
    };
    let glibc = if os == Os::Linux {
        output("getconf", &["GNU_LIBC_VERSION"]).and_then(|text| parse_glibc(&text))
    } else {
        None
    };
    Facts {
        os,
        arch,
        nvidia,
        vulkan: has_vulkan(os),
        glibc,
    }
}

/// The standard output of a command that succeeded.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first GPU of `nvidia-smi` output: "NVIDIA GeForce RTX 4090, 580.65.06".
pub fn parse_nvidia(text: &str) -> Option<(String, Version)> {
    let line = text.lines().next()?;
    let (name, driver) = line.rsplit_once(',')?;
    Some((name.trim().to_string(), parse_version(driver.trim())?))
}

/// The output of `getconf GNU_LIBC_VERSION`: "glibc 2.35".
pub fn parse_glibc(text: &str) -> Option<Version> {
    parse_version(text.trim().strip_prefix("glibc ")?)
}

/// "580.65.06" as (580, 65). A missing minor number is 0.
fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    Some((major, minor))
}

/// A Vulkan driver for a real GPU. On Linux, the driver files in `icd.d` count, except
/// the software driver (lavapipe). On Windows, the GPU driver installs `vulkan-1.dll`.
fn has_vulkan(os: Os) -> bool {
    match os {
        Os::Linux => ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| is_gpu_driver(&entry.file_name().to_string_lossy())),
        Os::Windows => std::env::var_os("SystemRoot")
            .is_some_and(|root| Path::new(&root).join("System32/vulkan-1.dll").is_file()),
        Os::Mac | Os::Other => false,
    }
}

/// True for a Vulkan driver file of a real GPU, such as "radeon_icd.x86_64.json".
pub fn is_gpu_driver(file: &str) -> bool {
    file.ends_with(".json") && !file.starts_with("lvp_icd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_gives_the_name_and_driver() {
        assert_eq!(
            parse_nvidia("NVIDIA GeForce RTX 4090, 580.65.06\n"),
            Some(("NVIDIA GeForce RTX 4090".to_string(), (580, 65)))
        );
        assert_eq!(parse_nvidia(""), None);
        assert_eq!(parse_nvidia("no devices"), None);
    }

    #[test]
    fn getconf_gives_the_glibc_version() {
        assert_eq!(parse_glibc("glibc 2.35\n"), Some((2, 35)));
        assert_eq!(parse_glibc("musl"), None);
    }

    #[test]
    fn the_software_vulkan_driver_does_not_count() {
        assert!(is_gpu_driver("radeon_icd.x86_64.json"));
        assert!(is_gpu_driver("nvidia_icd.json"));
        assert!(!is_gpu_driver("lvp_icd.x86_64.json"));
        assert!(!is_gpu_driver("README"));
    }
}
