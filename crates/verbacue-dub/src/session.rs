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
    let env = ort::init()
        .with_name("verbacue")
        .build()
        .map_err(model_error)?;
    Ok(ENV.get_or_init(|| env))
}

pub(crate) fn model_error<R>(e: ort::Error<R>) -> Error {
    Error::new(ErrorKind::Model, e.to_string())
}

/// Opens an ONNX graph. It uses CUDA in a `cuda` build, and CoreML on macOS when
/// `coreml` is true. ONNX Runtime runs a node on the CPU when the GPU cannot run it.
///
/// # Errors
/// Returns an error if the file is not a valid ONNX graph.
pub fn open(path: &Path, coreml: bool) -> Result<Session> {
    let builder = Session::builder(environment()?).map_err(model_error)?;
    #[cfg(target_os = "macos")]
    let builder = if coreml {
        builder
            .with_execution_providers([ort::ep::CoreML::default().build()])
            .map_err(model_error)?
    } else {
        builder
    };
    #[cfg(not(target_os = "macos"))]
    let _ = coreml;
    #[cfg(feature = "cuda")]
    let builder = builder
        .with_execution_providers([ort::ep::CUDA::default().build()])
        .map_err(model_error)?;
    let mut builder = builder;
    builder.commit_from_file(path).map_err(|e| {
        Error::new(
            ErrorKind::Model,
            format!("cannot open {}: {e}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_a_file_that_is_not_onnx() {
        let path = std::env::temp_dir().join("verbacue-not-onnx.onnx");
        std::fs::write(&path, b"not onnx").unwrap();
        let err = open(&path, false).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Model);
    }
}
