//! Reads and writes simple WAV files: mono 16-bit for output, and 16-bit or 32-bit
//! float PCM for input.

use std::{fs, io::Write, path::Path};

use verbacue_core::{Context as _, Error, ErrorKind, Result};

/// Writes mono 16-bit PCM. Samples outside -1..1 are clipped.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub fn write(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let data_len = u32::try_from(samples.len() * 2)
        .ok()
        .context("the audio is too long for WAV")?;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        #[expect(clippy::cast_possible_truncation, reason = "clamped to the i16 range")]
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut file =
        fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    file.write_all(&out)
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Reads a WAV file as mono samples and the sample rate. It takes the mean of the
/// channels.
///
/// # Errors
/// Returns an error if the file is not 16-bit PCM or 32-bit float WAV.
pub fn read(path: &Path) -> Result<(Vec<f32>, u32)> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&bytes).map_err(|e| Error::new(ErrorKind::Other, format!("{}: {e}", path.display())))
}

fn parse(bytes: &[u8]) -> std::result::Result<(Vec<f32>, u32), &'static str> {
    let u16_at = |i: usize| {
        bytes
            .get(i..i + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |i: usize| {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV file");
    }
    let mut format = None;
    let mut at = 12;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(at + 4)) {
        let body = at + 8;
        let len = len as usize;
        match id {
            b"fmt " => {
                let tag = u16_at(body).ok_or("short fmt chunk")?;
                let channels = u16_at(body + 2).ok_or("short fmt chunk")?;
                let rate = u32_at(body + 4).ok_or("short fmt chunk")?;
                let bits = u16_at(body + 14).ok_or("short fmt chunk")?;
                // 0xFFFE is WAVE_FORMAT_EXTENSIBLE; its sub-format is at offset 24.
                let tag = if tag == 0xFFFE {
                    u16_at(body + 24).ok_or("short fmt chunk")?
                } else {
                    tag
                };
                format = Some((tag, channels.max(1) as usize, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.ok_or("data before fmt")?;
                let data = bytes
                    .get(body..(body + len).min(bytes.len()))
                    .ok_or("short data")?;
                let samples: Vec<f32> = match (tag, bits) {
                    (1, 16) => data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&b| f32::from(i16::from_le_bytes(b)) / 32768.0)
                        .collect(),
                    (3, 32) => data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|&b| f32::from_le_bytes(b))
                        .collect(),
                    _ => return Err("only 16-bit PCM and 32-bit float WAV are supported"),
                };
                #[expect(clippy::cast_precision_loss, reason = "a channel count")]
                let mono = samples
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                    .collect();
                return Ok((mono, rate));
            }
            _ => {}
        }
        at = body + len + (len & 1);
    }
    Err("no data chunk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_gives_the_same_audio() {
        let path = std::env::temp_dir().join("verbacue-wav-test.wav");
        let audio = [0.0, 0.5, -0.5, 1.0, -1.0, 2.0];
        write(&path, &audio, 24_000).unwrap();
        let (back, rate) = read(&path).unwrap();
        assert_eq!(rate, 24_000);
        assert_eq!(back.len(), audio.len());
        for (a, b) in back.iter().zip([0.0, 0.5, -0.5, 1.0, -1.0, 1.0]) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn read_rejects_other_files() {
        assert!(parse(b"not a wav file at all").is_err());
    }
}
