//! Opens ONNX graphs on the best device of this machine.

use std::{path::Path, sync::OnceLock};

use ort::{environment::Environment, session::Session};
use verbacue_core::{Error, ErrorKind, Result};

/// ONNX Runtime needs one environment for the process.
fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<Environment> = OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let env = builder()?
        .with_name("verbacue")
        .build()
        .map_err(model_error)?;
    Ok(ENV.get_or_init(|| env))
}

#[cfg(not(windows))]
fn builder() -> Result<ort::environment::EnvironmentBuilder> {
    Ok(ort::init())
}

/// On Windows, ONNX Runtime is `onnxruntime.dll` next to `verbacue.exe`, or the file in
/// `ORT_DYLIB_PATH`.
#[cfg(windows)]
fn builder() -> Result<ort::environment::EnvironmentBuilder> {
    let path = match std::env::var_os("ORT_DYLIB_PATH") {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => std::env::current_exe()
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?
            .with_file_name("onnxruntime.dll"),
    };
    if !path.is_file() {
        return Err(Error::new(
            ErrorKind::NotFound,
            format!(
                "dubbing needs {}, which is missing; install verbacue again",
                path.display()
            ),
        ));
    }
    ort::init_from(&path).map_err(|e| {
        Error::new(
            ErrorKind::Model,
            format!("cannot load {}: {e}", path.display()),
        )
    })
}

/// Makes `argv[0]` the real path of the program, so that ONNX Runtime finds its CUDA
/// libraries. Call it first in `main`.
///
/// ONNX Runtime loads `libonnxruntime_providers_shared.so` and
/// `libonnxruntime_providers_cuda.so` from the folder of `argv[0]`. It does not follow
/// symlinks, and a name from `PATH` gives the current folder. In these cases, this
/// function starts the program again with the real path as `argv[0]`. It does
/// nothing in other builds.
pub fn fix_argv0() {
    #[cfg(all(feature = "cuda", target_os = "linux"))]
    {
        use std::os::unix::process::CommandExt as _;

        const GUARD: &str = "VERBACUE_ARGV0_FIXED";
        if std::env::var_os(GUARD).is_some() {
            return;
        }
        let (Ok(exe), Some(arg0)) = (std::env::current_exe(), std::env::args_os().next()) else {
            return;
        };
        let Ok(arg0) = std::path::absolute(arg0) else {
            return;
        };
        if arg0.parent() == exe.parent() {
            return;
        }
        let err = std::process::Command::new(&exe)
            .arg0(&exe)
            .args(std::env::args_os().skip(1))
            .env(GUARD, "1")
            .exec();
        eprintln!("warning: cannot start {} again: {err}", exe.display());
    }
}

/// Drops the value that some ort errors carry, so that the error can be stored.
pub(crate) fn model_error<R>(e: ort::Error<R>) -> crate::error::Error {
    ort::Error::new_with_code(e.code(), e.message()).into()
}

/// Opens an ONNX graph. It uses CUDA in a `cuda` build, else the CPU. ONNX Runtime
/// runs a node on the CPU when CUDA cannot run it.
///
/// CoreML is not used: it gives `inf` in the XTTS-v2 conditioning encoder, refuses
/// the growing KV cache of the GPT, and cannot compile HTDemucs.
///
/// # Errors
/// Returns an error if the file is not a valid ONNX graph.
pub fn open(path: &Path) -> Result<Session> {
    #[cfg(feature = "cuda")]
    if let Some(session) = open_cuda(path) {
        return Ok(session);
    }
    let mut builder = Session::builder(environment()?).map_err(model_error)?;
    builder.commit_from_file(path).map_err(|e| {
        Error::new(
            ErrorKind::Model,
            format!("cannot open {}: {e}", path.display()),
        )
    })
}

/// Opens a graph with CUDA. CUDA can fail late, when ONNX Runtime opens the graph:
/// for example without a GPU, or without cuDNN. Then this returns `None`, and all
/// later graphs of the process use the CPU.
#[cfg(feature = "cuda")]
fn open_cuda(path: &Path) -> Option<Session> {
    use std::sync::atomic::{AtomicBool, Ordering};

    static FAILED: AtomicBool = AtomicBool::new(false);
    if FAILED.load(Ordering::Relaxed) {
        return None;
    }
    let commit = || -> std::result::Result<Session, String> {
        let builder = Session::builder(environment().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mut builder = builder
            .with_execution_providers([ort::ep::CUDA::default().build()])
            .map_err(|e| e.to_string())?;
        builder.commit_from_file(path).map_err(|e| e.to_string())
    };
    match commit() {
        Ok(session) => Some(session),
        Err(e) => {
            FAILED.store(true, Ordering::Relaxed);
            tracing::warn!("dubbing runs on the CPU, because CUDA failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_a_file_that_is_not_onnx() {
        let path = std::env::temp_dir().join("verbacue-not-onnx.onnx");
        std::fs::write(&path, b"not onnx").unwrap();
        let err = open(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Model);
    }
}
