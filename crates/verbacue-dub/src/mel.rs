//! The two mel spectrograms that XTTS-v2 needs to clone a voice. ONNX cannot export
//! the STFT of torchaudio, so Rust makes them. `tools/xtts-export/parity.py` has the
//! numpy reference.
//!
//! The settings are the values in the `cond_mel` and `speaker_mel` parts of
//! `config.json`.

use std::f64::consts::PI;

use rustfft::{FftPlanner, num_complex::Complex};

/// A mel spectrogram, `[n_mels, frames]` in row-major order.
pub struct Mel {
    pub n_mels: usize,
    pub frames: usize,
    pub data: Vec<f32>,
}

#[derive(Clone, Copy)]
enum Window {
    Hann,
    Hamming,
}

struct Spec {
    sample_rate: u32,
    n_fft: usize,
    hop: usize,
    win_length: usize,
    window: Window,
    n_mels: usize,
    f_min: f64,
    f_max: f64,
    slaney: bool,
}

/// The mel for the GPT conditioning encoder, from 22.05 kHz audio. `stats` are the
/// 80 `mel_stats` of `config.json`.
pub fn cond_mel(audio_22k: &[f32], stats: &[f32]) -> Mel {
    let spec = Spec {
        sample_rate: 22_050,
        n_fft: 2048,
        hop: 256,
        win_length: 1024,
        window: Window::Hann,
        n_mels: 80,
        f_min: 0.0,
        f_max: 8000.0,
        slaney: true,
    };
    let x: Vec<f64> = audio_22k.iter().map(|&s| f64::from(s)).collect();
    let (mel, frames) = spec.mel(&x);
    let data = mel
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            #[expect(clippy::cast_possible_truncation, reason = "f32 output")]
            let v = (v.max(1e-5).ln() / f64::from(stats[i / frames])) as f32;
            v
        })
        .collect();
    Mel {
        n_mels: 80,
        frames,
        data,
    }
}

/// The mel for the speaker encoder of the vocoder, from 16 kHz audio. The encoder
/// takes the log itself.
pub fn speaker_mel(audio_16k: &[f32]) -> Mel {
    let spec = Spec {
        sample_rate: 16_000,
        n_fft: 512,
        hop: 160,
        win_length: 400,
        window: Window::Hamming,
        n_mels: 64,
        f_min: 0.0,
        f_max: 8000.0,
        slaney: false,
    };
    // Pre-emphasis 0.97, with a reflected first sample, as the encoder does.
    let first = audio_16k.get(1).copied().unwrap_or(0.0);
    let x: Vec<f64> = std::iter::once(first)
        .chain(audio_16k.iter().copied())
        .collect::<Vec<f32>>()
        .windows(2)
        .map(|w| f64::from(w[1]) - 0.97 * f64::from(w[0]))
        .collect();
    let (mel, frames) = spec.mel(&x);
    #[expect(clippy::cast_possible_truncation, reason = "f32 output")]
    let data = mel.iter().map(|&v| v as f32).collect();
    Mel {
        n_mels: 64,
        frames,
        data,
    }
}

impl Spec {
    /// The mel power spectrogram, `[n_mels, frames]`, and the frame count.
    fn mel(&self, x: &[f64]) -> (Vec<f64>, usize) {
        let power = self.power(x);
        let n_freqs = self.n_fft / 2 + 1;
        let frames = power.len() / n_freqs;
        let fb = self.filters();
        let mut mel = vec![0.0; self.n_mels * frames];
        for f in 0..frames {
            let p = &power[f * n_freqs..(f + 1) * n_freqs];
            for m in 0..self.n_mels {
                mel[m * frames + f] = (0..n_freqs).map(|k| fb[k * self.n_mels + m] * p[k]).sum();
            }
        }
        (mel, frames)
    }

    /// The power STFT, `[frames, n_freqs]`, as `torch.stft` with `center=True` and
    /// reflect padding. The window is in the middle of the `n_fft` frame.
    fn power(&self, x: &[f64]) -> Vec<f64> {
        let n = self.n_fft;
        let wl = self.win_length;
        let mut window = vec![0.0; n];
        let left = (n - wl) / 2;
        for (i, w) in window[left..left + wl].iter_mut().enumerate() {
            let c = (2.0 * PI * i as f64 / wl as f64).cos();
            *w = match self.window {
                Window::Hann => 0.5 - 0.5 * c,
                Window::Hamming => 0.54 - 0.46 * c,
            };
        }
        let pad = n / 2;
        let len = x.len();
        // numpy "reflect": the edge sample is not repeated.
        let at = |i: isize| -> f64 {
            let last = len as isize - 1;
            let mut i = i;
            if last <= 0 {
                return x.first().copied().unwrap_or(0.0);
            }
            while i < 0 || i > last {
                i = if i < 0 { -i } else { 2 * last - i };
            }
            x[i as usize]
        };
        let frames = 1 + len / self.hop;
        let fft = FftPlanner::new().plan_fft_forward(n);
        let n_freqs = n / 2 + 1;
        let mut out = Vec::with_capacity(frames * n_freqs);
        let mut buf = vec![Complex::new(0.0, 0.0); n];
        for f in 0..frames {
            let start = (f * self.hop) as isize - pad as isize;
            for (j, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(at(start + j as isize) * window[j], 0.0);
            }
            fft.process(&mut buf);
            out.extend(buf[..n_freqs].iter().map(Complex::norm_sqr));
        }
        out
    }

    /// Triangular filters on the HTK mel scale, `[n_freqs, n_mels]`, as
    /// `torchaudio.functional.melscale_fbanks`.
    fn filters(&self) -> Vec<f64> {
        let hz_to_mel = |f: f64| 2595.0 * (1.0 + f / 700.0).log10();
        let mel_to_hz = |m: f64| 700.0 * (10f64.powf(m / 2595.0) - 1.0);
        let n_freqs = self.n_fft / 2 + 1;
        let nyquist = f64::from(self.sample_rate / 2);
        let (lo, hi) = (hz_to_mel(self.f_min), hz_to_mel(self.f_max));
        let f_pts: Vec<f64> = (0..self.n_mels + 2)
            .map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (self.n_mels + 1) as f64))
            .collect();
        let mut fb = vec![0.0; n_freqs * self.n_mels];
        for k in 0..n_freqs {
            let freq = nyquist * k as f64 / (n_freqs - 1) as f64;
            for m in 0..self.n_mels {
                let down = (freq - f_pts[m]) / (f_pts[m + 1] - f_pts[m]);
                let up = (f_pts[m + 2] - freq) / (f_pts[m + 2] - f_pts[m + 1]);
                let mut v = down.min(up).max(0.0);
                if self.slaney {
                    v *= 2.0 / (f_pts[m + 2] - f_pts[m]);
                }
                fb[k * self.n_mels + m] = v;
            }
        }
        fb
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::safetensors;

    #[test]
    fn frame_count_matches_torch() {
        let mel = speaker_mel(&vec![0.1; 16_000]);
        assert_eq!((mel.n_mels, mel.frames), (64, 101));
        let mel = cond_mel(&vec![0.1; 22_050], &[1.0; 80]);
        assert_eq!((mel.n_mels, mel.frames), (80, 87));
    }

    #[test]
    fn a_tone_lands_in_the_right_band() {
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let tone: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16_000.0).sin())
            .collect();
        let mel = speaker_mel(&tone);
        let mid = mel.frames / 2;
        let loudest = (0..64)
            .max_by(|&a, &b| {
                mel.data[a * mel.frames + mid].total_cmp(&mel.data[b * mel.frames + mid])
            })
            .unwrap();
        // 1 kHz is about 1000 mel; 64 bands up to 8 kHz (2840 mel) put it in band 21 or 22.
        assert!((21..=22).contains(&loudest), "band {loudest}");
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn mel_matches_python() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out");
        let fx = safetensors::load(&out.join("fixtures/clone.safetensors")).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("config.json")).unwrap())
                .unwrap();
        #[expect(clippy::cast_possible_truncation, reason = "test")]
        let stats: Vec<f32> = config["cond_mel"]["mel_stats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();

        let audio = &fx["audio_22k"].data;
        let got = cond_mel(&audio[..audio.len().min(22_050 * 6)], &stats);
        let want = &fx["cond_mel"];
        assert_eq!([got.n_mels, got.frames], want.shape[..]);
        let diff = max_diff(&got.data, &want.data);
        assert!(diff < 1e-4, "cond mel: max diff {diff}");

        // The speaker mel is a power, so compare relative to its largest value.
        let got = speaker_mel(&fx["audio_16k"].data);
        let want = &fx["speaker_mel"];
        assert_eq!([got.n_mels, got.frames], want.shape[..]);
        let scale = want.data.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let diff = max_diff(&got.data, &want.data) / scale;
        assert!(diff < 1e-4, "speaker mel: relative max diff {diff}");
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }
}
