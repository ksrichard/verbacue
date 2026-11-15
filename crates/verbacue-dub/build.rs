//! Fixes two link problems of the prebuilt ONNX Runtime.
//!
//! On macOS, links the clang runtime. ONNX Runtime calls
//! `__isPlatformVersionAtLeast` from it. Rust's std has a weak copy of this
//! function, but thin LTO removes it, and then the release build does not link.
//!
//! On Linux with glibc older than 2.38, enables `src/glibc.rs`.

use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(old_glibc)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "linux" && env == "gnu" && old_glibc() {
        println!("cargo::rustc-cfg=old_glibc");
    }
    if os == "macos" {
        link_clang_runtime();
    }
}

/// True when the glibc of the build machine is older than 2.38. The Linux builds
/// are not cross builds, so this is also the glibc that the program links.
fn old_glibc() -> bool {
    let Some(out) = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .ok()
        .filter(|o| o.status.success())
    else {
        return false;
    };
    // The output is "glibc 2.35".
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().trim_start_matches("glibc ").split('.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major == 2 && minor < 38
}

fn link_clang_runtime() {
    let dir = Command::new("xcrun")
        .args(["clang", "--print-runtime-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match dir {
        Some(dir) if !dir.is_empty() => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        _ => println!("cargo:warning=no clang runtime found; a release build may not link"),
    }
}
