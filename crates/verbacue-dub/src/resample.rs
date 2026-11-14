//! Changes the sample rate of audio, as `torchaudio.functional.resample` does with
//! its defaults. XTTS-v2 was trained on audio resampled this way.
//!
//! The method is a windowed sinc filter with a Hann window and 6 zero crossings on
//! each side. The cut-off is at 99 % of the lower Nyquist frequency.

use std::f64::consts::PI;

const ZEROS: f64 = 6.0;
const ROLLOFF: f64 = 0.99;

/// Resamples `audio` from `from` Hz to `to` Hz.
pub fn resample(audio: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || audio.is_empty() {
        return audio.to_vec();
    }
    let g = gcd(from, to);
    let (orig, new) = ((from / g) as usize, (to / g) as usize);
    let base = orig.min(new) as f64 * ROLLOFF;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "small"
    )]
    let width = (ZEROS * orig as f64 / base).ceil() as usize;

    // One filter for each of the `new` output phases.
    let taps = 2 * width + orig;
    let kernels: Vec<Vec<f64>> = (0..new)
        .map(|phase| {
            (0..taps)
                .map(|j| {
                    let idx = (j as f64 - width as f64) / orig as f64;
                    let t = ((idx - phase as f64 / new as f64) * base).clamp(-ZEROS, ZEROS);
                    let window = (t * PI / ZEROS / 2.0).cos().powi(2);
                    let t = t * PI;
                    let sinc = if t == 0.0 { 1.0 } else { t.sin() / t };
                    sinc * window * base / orig as f64
                })
                .collect()
        })
        .collect();

    // The input with `width` zeros in front and `width + orig` zeros after it.
    let at = |i: usize| -> f64 {
        i.checked_sub(width)
            .and_then(|i| audio.get(i))
            .map_or(0.0, |&s| f64::from(s))
    };
    let out_len = (new * audio.len()).div_ceil(orig);
    (0..out_len)
        .map(|n| {
            let (block, phase) = (n / new, n % new);
            let start = block * orig;
            #[expect(clippy::cast_possible_truncation, reason = "audio samples")]
            let s = kernels[phase]
                .iter()
                .enumerate()
                .map(|(j, k)| k * at(start + j))
                .sum::<f64>() as f32;
            s
        })
        .collect()
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{safetensors, wav};

    #[test]
    fn keeps_a_low_tone() {
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let tone: Vec<f32> = (0..24_000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 24_000.0).sin())
            .collect();
        let out = resample(&tone, 24_000, 16_000);
        assert_eq!(out.len(), 16_000);
        // Away from the edges, the output is the same tone at the new rate.
        for (i, got) in out.iter().enumerate().take(15_000).skip(1000) {
            #[expect(clippy::cast_precision_loss, reason = "test")]
            let want = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin();
            assert!((got - want).abs() < 1e-3, "{i}: {got} vs {want}");
        }
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn matches_torchaudio() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out");
        let fx = safetensors::load(&out.join("fixtures/clone.safetensors")).unwrap();
        let (audio, rate) = wav::read(&out.join("fixtures/reference.wav")).unwrap();
        for (to, name) in [(22_050, "audio_22k"), (16_000, "audio_16k")] {
            let got = resample(&audio, rate, to);
            let want = &fx[name].data;
            assert_eq!(got.len(), want.len(), "{name}");
            let diff = got
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(diff < 1e-5, "{name}: max diff {diff}");
        }
    }
}
