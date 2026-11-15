//! Kaldi filter-bank features for the CAM++ speaker model, the same as
//! `torchaudio.compliance.kaldi.fbank` with 80 bins, 16 kHz and no dither.

use std::f64::consts::PI;

use rustfft::{FftPlanner, num_complex::Complex};

pub const SAMPLE_RATE: u32 = 16_000;
pub const BINS: usize = 80;
const FRAME: usize = 400;
const SHIFT: usize = 160;
const FFT: usize = 512;
const PREEMPHASIS: f64 = 0.97;
const LOW_HZ: f64 = 20.0;

/// The features of 16 kHz mono audio, `[frames][BINS]` in row-major order, with the
/// mean of each bin over time removed. Audio shorter than one frame gives no frames.
pub fn fbank(audio: &[f32]) -> (usize, Vec<f32>) {
    if audio.len() < FRAME {
        return (0, Vec::new());
    }
    let frames = 1 + (audio.len() - FRAME) / SHIFT;
    let window: Vec<f64> = (0..FRAME)
        .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / (FRAME - 1) as f64).cos()).powf(0.85))
        .collect();
    let banks = mel_banks();
    let fft = FftPlanner::new().plan_fft_forward(FFT);
    let mut out = vec![0.0f64; frames * BINS];
    let mut buf = vec![Complex::new(0.0, 0.0); FFT];
    let mut frame = [0.0f64; FRAME];
    for f in 0..frames {
        for (x, &s) in frame.iter_mut().zip(&audio[f * SHIFT..f * SHIFT + FRAME]) {
            *x = f64::from(s);
        }
        let mean = frame.iter().sum::<f64>() / FRAME as f64;
        frame.iter_mut().for_each(|x| *x -= mean);
        for i in (1..FRAME).rev() {
            frame[i] -= PREEMPHASIS * frame[i - 1];
        }
        frame[0] -= PREEMPHASIS * frame[0];
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(if i < FRAME { frame[i] * window[i] } else { 0.0 }, 0.0);
        }
        fft.process(&mut buf);
        let power: Vec<f64> = buf[..FFT / 2].iter().map(Complex::norm_sqr).collect();
        for (b, bank) in banks.iter().enumerate() {
            let energy: f64 = bank.iter().map(|&(i, w)| w * power[i]).sum();
            out[f * BINS + b] = energy.max(f64::from(f32::EPSILON)).ln();
        }
    }
    for b in 0..BINS {
        let mean = (0..frames).map(|f| out[f * BINS + b]).sum::<f64>() / frames as f64;
        (0..frames).for_each(|f| out[f * BINS + b] -= mean);
    }
    #[expect(clippy::cast_possible_truncation, reason = "f32 features")]
    (frames, out.into_iter().map(|v| v as f32).collect())
}

fn mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

/// The triangular mel filters as (FFT bin, weight) pairs. The Nyquist bin has no weight.
fn mel_banks() -> Vec<Vec<(usize, f64)>> {
    let bin_hz = f64::from(SAMPLE_RATE) / FFT as f64;
    let (low, high) = (mel(LOW_HZ), mel(f64::from(SAMPLE_RATE) / 2.0));
    let delta = (high - low) / (BINS + 1) as f64;
    (0..BINS)
        .map(|b| {
            let (left, centre, right) = (
                low + b as f64 * delta,
                low + (b + 1) as f64 * delta,
                low + (b + 2) as f64 * delta,
            );
            (0..FFT / 2)
                .filter_map(|i| {
                    let m = mel(bin_hz * i as f64);
                    let w = ((m - left) / (centre - left)).min((right - m) / (right - centre));
                    (w > 0.0).then_some((i, w))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_audio_has_no_frames() {
        assert_eq!(fbank(&[0.0; 399]).0, 0);
        assert_eq!(fbank(&[0.0; 400]).0, 1);
        assert_eq!(fbank(&[0.0; 16_000]).0, 98);
    }

    #[test]
    fn each_bin_has_zero_mean() {
        let audio: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let (frames, data) = fbank(&audio);
        for b in 0..BINS {
            let mean: f32 = (0..frames).map(|f| data[f * BINS + b]).sum::<f32>() / frames as f32;
            assert!(mean.abs() < 1e-3, "bin {b}: {mean}");
        }
    }
}
