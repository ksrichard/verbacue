//! Reads the F32 tensors of a `.safetensors` file.
//!
//! The format is an 8-byte little-endian header length, a JSON header with the
//! dtype, shape and byte offsets of each tensor, and then the raw data.

use std::{collections::HashMap, path::Path};

use serde_json::Value;
use verbacue_core::{Context as _, Result};

/// A tensor: its shape and its values in row-major order.
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// Reads all tensors of a file. All tensors must be F32.
///
/// # Errors
/// Returns an error if the file cannot be read or is not a valid F32 safetensors file.
pub fn load(path: &Path) -> Result<HashMap<String, Tensor>> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&bytes).with_context(|| format!("{} is not a valid safetensors file", path.display()))
}

fn parse(bytes: &[u8]) -> Option<HashMap<String, Tensor>> {
    let len = usize::try_from(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?)).ok()?;
    let header: Value = serde_json::from_slice(bytes.get(8..8 + len)?).ok()?;
    let data = bytes.get(8 + len..)?;
    let mut out = HashMap::new();
    for (name, info) in header.as_object()? {
        if name == "__metadata__" {
            continue;
        }
        if info["dtype"].as_str()? != "F32" {
            return None;
        }
        let shape = info["shape"]
            .as_array()?
            .iter()
            .map(|d| d.as_u64().and_then(|d| usize::try_from(d).ok()))
            .collect::<Option<Vec<_>>>()?;
        let offsets = info["data_offsets"].as_array()?;
        let start = usize::try_from(offsets.first()?.as_u64()?).ok()?;
        let end = usize::try_from(offsets.get(1)?.as_u64()?).ok()?;
        let raw = data.get(start..end)?;
        if raw.len() != 4 * shape.iter().product::<usize>() {
            return None;
        }
        let values = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect();
        out.insert(
            name.clone(),
            Tensor {
                shape,
                data: values,
            },
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_file() {
        let header = br#"{"a":{"dtype":"F32","shape":[2],"data_offsets":[0,8]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&(-2.0f32).to_le_bytes());
        let t = parse(&bytes).unwrap();
        assert_eq!(t["a"].shape, [2]);
        assert_eq!(t["a"].data, [1.5, -2.0]);
    }

    #[test]
    fn rejects_a_wrong_size() {
        let header = br#"{"a":{"dtype":"F32","shape":[3],"data_offsets":[0,8]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&[0; 8]);
        assert!(parse(&bytes).is_none());
    }
}
