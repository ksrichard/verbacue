//! Makes a spoken line sound like part of the scene. XTTS speaks like a close studio
//! microphone: more bass, less treble, and no room. In a film, the actors have the
//! sound of the room and the microphone of the scene, and the background keeps it.
//! [`match_tone`] gives a new line the sound colour of the original line, and
//! [`room`] adds a short room echo.

use rustfft::{FftPlanner, num_complex::Complex};

use crate::xtts::SAMPLE_RATE;

const FFT: usize = 1024;
const HOP: usize = 256;
/// The largest boost or cut of the tone match, in dB.
const MAX_DB: f32 = 6.0;
/// The tone match ignores the frequencies outside this range.
const LOW_HZ: f32 = 80.0;
const HIGH_HZ: f32 = 8000.0;
/// The room echo: its decay time (RT60), its delay, and its level against the voice.
const ROOM_RT60_S: f32 = 0.35;
const ROOM_DELAY_MS: usize = 8;
const ROOM_LEVEL_DB: f32 = -16.0;

/// `line` (24 kHz) with the sound colour of `original` (24 kHz): a smooth EQ of at
/// most ±6 dB from the average spectra. The loudness stays for [`crate::mix::match_gain`].
/// A line or an original shorter than 0.1 s stays as it is.
pub fn match_tone(original: &[f32], line: &[f32]) -> Vec<f32> {
    let (Some(want), Some(have)) = (spectrum(original), spectrum(line)) else {
        return line.to_vec();
    };
    let gains = eq_gains(&want, &have);
    filter(line, &gains)
}

/// The mean power of each FFT bin. `None` for audio shorter than 0.1 s.
fn spectrum(audio: &[f32]) -> Option<Vec<f32>> {
    if audio.len() < SAMPLE_RATE as usize / 10 {
        return None;
    }
    let window = hann();
    let fft = FftPlanner::new().plan_fft_forward(FFT);
    let mut power = vec![0.0f32; FFT / 2 + 1];
    let mut buf = vec![Complex::new(0.0, 0.0); FFT];
    let mut frames = 0;
    let mut start = 0;
    while start < audio.len() {
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(
                audio.get(start + i).copied().unwrap_or(0.0) * window[i],
                0.0,
            );
        }
        fft.process(&mut buf);
        power
            .iter_mut()
            .zip(&buf)
            .for_each(|(p, c)| *p += c.norm_sqr());
        frames += 1;
        start += HOP;
    }
    #[expect(clippy::cast_precision_loss, reason = "a frame count")]
    let n = frames as f32;
    Some(power.into_iter().map(|p| p / n).collect())
}

/// The gain of each bin, as a factor: the dB difference of the spectra, without its
/// mean (the loudness is matched elsewhere), smoothed over a third of an octave and
/// limited to ±6 dB.
fn eq_gains(want: &[f32], have: &[f32]) -> Vec<f32> {
    let bins = want.len();
    let hz = |k: usize| k as f32 * SAMPLE_RATE as f32 / FFT as f32;
    let raw: Vec<f32> = (0..bins)
        .map(|k| 10.0 * ((want[k] + 1e-12) / (have[k] + 1e-12)).log10())
        .collect();
    let used: Vec<usize> = (0..bins)
        .filter(|&k| (LOW_HZ..=HIGH_HZ).contains(&hz(k)))
        .collect();
    let mean = used.iter().map(|&k| raw[k]).sum::<f32>() / used.len().max(1) as f32;
    let third = 2.0f32.powf(1.0 / 6.0);
    let (first, last) = (used[0], used[used.len() - 1]);
    (0..bins)
        .map(|k| {
            // Outside the range, the gain of the nearest edge.
            let k = k.clamp(first, last);
            let (lo, hi) = (hz(k) / third, hz(k) * third);
            let near: Vec<f32> = used
                .iter()
                .filter(|&&j| (lo..=hi).contains(&hz(j)))
                .map(|&j| raw[j] - mean)
                .collect();
            let db = near.iter().sum::<f32>() / near.len().max(1) as f32;
            10.0f32.powf(db.clamp(-MAX_DB, MAX_DB) / 20.0)
        })
        .collect()
}

/// Applies one gain per bin with an overlap-add STFT. The audio gets one FFT of
/// silence on each side, so that every sample has the full window overlap.
fn filter(audio: &[f32], gains: &[f32]) -> Vec<f32> {
    let padded: Vec<f32> = std::iter::repeat_n(0.0, FFT)
        .chain(audio.iter().copied())
        .chain(std::iter::repeat_n(0.0, FFT))
        .collect();
    let out = filter_padded(&padded, gains);
    out[FFT..FFT + audio.len()].to_vec()
}

fn filter_padded(audio: &[f32], gains: &[f32]) -> Vec<f32> {
    let window = hann();
    let mut planner = FftPlanner::new();
    let (forward, inverse) = (planner.plan_fft_forward(FFT), planner.plan_fft_inverse(FFT));
    let mut out = vec![0.0f32; audio.len() + FFT];
    let mut norm = vec![0.0f32; audio.len() + FFT];
    let mut buf = vec![Complex::new(0.0, 0.0); FFT];
    let mut start = 0;
    while start < audio.len() {
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(
                audio.get(start + i).copied().unwrap_or(0.0) * window[i],
                0.0,
            );
        }
        forward.process(&mut buf);
        for (k, c) in buf.iter_mut().enumerate() {
            // The upper half mirrors the lower half.
            *c *= gains[k.min(FFT - k)];
        }
        inverse.process(&mut buf);
        for i in 0..FFT {
            out[start + i] += buf[i].re / FFT as f32 * window[i];
            norm[start + i] += window[i] * window[i];
        }
        start += HOP;
    }
    out.truncate(audio.len());
    out.iter()
        .zip(&norm)
        .map(|(&v, &n)| if n > 1e-3 { v / n } else { v })
        .collect()
}

fn hann() -> Vec<f32> {
    (0..FFT)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT as f32).cos())
        .collect()
}

/// `line` (24 kHz) with a short room echo after it. The result is longer than `line` by
/// the echo tail.
pub fn room(line: &[f32]) -> Vec<f32> {
    if line.is_empty() {
        return Vec::new();
    }
    let ir = impulse();
    let n = (line.len() + ir.len()).next_power_of_two();
    let mut planner = FftPlanner::new();
    let (forward, inverse) = (planner.plan_fft_forward(n), planner.plan_fft_inverse(n));
    let mut a: Vec<Complex<f32>> = (0..n)
        .map(|i| Complex::new(line.get(i).copied().unwrap_or(0.0), 0.0))
        .collect();
    let mut b: Vec<Complex<f32>> = (0..n)
        .map(|i| Complex::new(ir.get(i).copied().unwrap_or(0.0), 0.0))
        .collect();
    forward.process(&mut a);
    forward.process(&mut b);
    a.iter_mut().zip(&b).for_each(|(x, y)| *x *= y);
    inverse.process(&mut a);
    let level = 10.0f32.powf(ROOM_LEVEL_DB / 20.0);
    (0..line.len() + ir.len() - 1)
        .map(|i| line.get(i).copied().unwrap_or(0.0) + level * a[i].re / n as f32)
        .collect()
}

/// The room response: decaying noise after a short delay, a little darker over time,
/// with an energy of 1. A fixed seed gives the same echo every time.
fn impulse() -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let len = (ROOM_RT60_S * rate) as usize;
    let delay = ROOM_DELAY_MS * SAMPLE_RATE as usize / 1000;
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut low = 0.0f32;
    let mut ir: Vec<f32> = (0..delay + len)
        .map(|i| {
            if i < delay {
                return 0.0;
            }
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let noise = (seed >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0;
            let t = (i - delay) as f32 / rate;
            // -60 dB at RT60.
            let decay = (-6.9 * t / ROOM_RT60_S).exp();
            low = 0.6 * low + 0.4 * noise;
            low * decay
        })
        .collect();
    let energy = ir.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
    ir.iter_mut().for_each(|v| *v /= energy);
    ir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, seconds: f32) -> Vec<f32> {
        (0..(seconds * SAMPLE_RATE as f32) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SAMPLE_RATE as f32).sin())
            .collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    fn level_at(audio: &[f32], hz: f32) -> f32 {
        let p = spectrum(audio).unwrap();
        let k = (hz * FFT as f32 / SAMPLE_RATE as f32).round() as usize;
        10.0 * p[k].log10()
    }

    #[test]
    fn the_tone_moves_towards_the_original() {
        // The original has more treble than the new line.
        let original = mix(&tone(200.0, 0.1, 1.0), &tone(4000.0, 0.1, 1.0));
        let line = mix(&tone(200.0, 0.1, 1.0), &tone(4000.0, 0.02, 1.0));
        let out = match_tone(&original, &line);
        let before = level_at(&line, 4000.0) - level_at(&line, 200.0);
        let after = level_at(&out, 4000.0) - level_at(&out, 200.0);
        assert!(after > before + 6.0, "{before} -> {after}");
        // Never more than twice the limit apart: ±6 dB at each end.
        assert!(after < before + 2.0 * MAX_DB + 1.0);
    }

    #[test]
    fn the_same_tone_stays() {
        let line = mix(&tone(300.0, 0.1, 1.0), &tone(2000.0, 0.05, 1.0));
        let out = match_tone(&line, &line);
        let diff: f32 = line
            .iter()
            .zip(&out)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(diff < 1e-3, "{diff}");
        assert_eq!(match_tone(&[0.1; 100], &line), line);
    }

    #[test]
    fn the_room_adds_a_quiet_tail() {
        let click = {
            let mut v = vec![0.0f32; 2400];
            v[0] = 1.0;
            v
        };
        let out = room(&click);
        assert_eq!(out.len(), 2400 + impulse().len() - 1);
        assert!((out[0] - 1.0).abs() < 1e-3);
        let tail: f32 = out[1..].iter().map(|v| v * v).sum::<f32>().sqrt();
        let level = 10.0f32.powf(ROOM_LEVEL_DB / 20.0);
        assert!((tail - level).abs() < 0.02, "{tail}");
    }
}
