//! Mixes the spoken lines into the background audio.
//!
//! The background is a raw stereo file ([`RawAudio`]), which can be hours long. The
//! mixer keeps only the spoken lines in memory and reads the background once, in
//! chunks, when it writes the mix.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Seek as _, SeekFrom, Write as _},
    path::Path,
};
use verbacue_media::audio::read_full;

use verbacue_core::{Context as _, Result};
use verbacue_media::audio::RawAudio;

use crate::{resample, xtts::SAMPLE_RATE};

/// The background level while the original speech is lowered: 0.25 is -12 dB.
const DUCK_GAIN: f32 = 0.25;
const DUCK_RAMP_MS: u64 = 50;
/// The fade before a line is cut at the next line.
const CUT_FADE_MS: u64 = 30;
const CHUNK_FRAMES: usize = 8192;

/// One spoken line at 24 kHz, with its gain and cut already applied.
struct Line {
    start_ms: u64,
    audio: Vec<f32>,
}

pub struct Mixer {
    background: RawAudio,
    /// The lowered parts, as `(start_ms, end_ms)`.
    ducks: Vec<(u64, u64)>,
    lines: Vec<Line>,
}

impl Mixer {
    pub fn new(background: RawAudio) -> Mixer {
        Mixer {
            background,
            ducks: Vec::new(),
            lines: Vec::new(),
        }
    }

    /// Lowers the background between `start_ms` and `end_ms`, with 50 ms ramps.
    pub fn duck(&mut self, start_ms: u64, end_ms: u64) {
        if end_ms > start_ms {
            self.ducks.push((start_ms, end_ms));
        }
    }

    /// Adds a spoken line at 24 kHz, times `gain`. With `cut_at_ms`, the line stops at
    /// that time, with a 30 ms fade.
    pub fn add(&mut self, start_ms: u64, voice_24k: &[f32], gain: f32, cut_at_ms: Option<u64>) {
        let mut audio: Vec<f32> = voice_24k.iter().map(|s| s * gain).collect();
        if let Some(cut) = cut_at_ms {
            let to_samples =
                |ms: u64| usize::try_from(ms * u64::from(SAMPLE_RATE) / 1000).unwrap_or(usize::MAX);
            let end = to_samples(cut.saturating_sub(start_ms)).min(audio.len());
            let fade = to_samples(CUT_FADE_MS).min(end);
            audio.truncate(end);
            for (i, s) in audio[end - fade..].iter_mut().enumerate() {
                #[expect(clippy::cast_precision_loss, reason = "a short fade")]
                let w = 1.0 - (i + 1) as f32 / fade as f32;
                *s *= w;
            }
        }
        if !audio.is_empty() {
            self.lines.push(Line { start_ms, audio });
        }
    }

    /// Writes the mix to `out` in the format of the background.
    ///
    /// # Errors
    /// Returns an error if a file cannot be read or written.
    pub fn finish(mut self, out: &Path) -> Result<RawAudio> {
        let rate = self.background.rate.max(1);
        let channels = usize::from(self.background.channels.max(1));
        let ms_to_frame = |ms: u64| ms * u64::from(rate) / 1000;

        let ducks = merge(&mut self.ducks);
        self.lines.sort_by_key(|l| l.start_ms);
        // The lines in frames of the mix, resampled when they are first needed.
        let mut pending = self.lines.into_iter().peekable();
        let mut active: Vec<(u64, Vec<f32>)> = Vec::new();
        let mut duck_at = 0;

        let file = File::open(&self.background.path)
            .with_context(|| format!("cannot read {}", self.background.path.display()))?;
        let mut reader = BufReader::new(file);
        let mut writer = BufWriter::new(
            File::create(out).with_context(|| format!("cannot write {}", out.display()))?,
        );
        let mut bytes = vec![0u8; CHUNK_FRAMES * channels * 4];
        let mut frame = 0u64;
        loop {
            let n = read_full(&mut reader, &mut bytes).context("cannot read the audio")?;
            let frames = n / (channels * 4);
            if frames == 0 {
                break;
            }
            let chunk_end = frame + frames as u64;
            while let Some(line) = pending.next_if(|l| ms_to_frame(l.start_ms) < chunk_end) {
                active.push((
                    ms_to_frame(line.start_ms),
                    resample::resample(&line.audio, SAMPLE_RATE, rate),
                ));
            }
            let mut out_bytes = Vec::with_capacity(frames * channels * 4);
            for (i, sample) in bytes[..frames * channels * 4]
                .chunks_exact(channels * 4)
                .enumerate()
            {
                let f = frame + i as u64;
                let ms = f * 1000 / u64::from(rate);
                while duck_at < ducks.len() && ducks[duck_at].1 + DUCK_RAMP_MS <= ms {
                    duck_at += 1;
                }
                let gain = ducks[duck_at..]
                    .iter()
                    .take(2)
                    .map(|&d| duck_gain(d, ms))
                    .fold(1.0, f32::min);
                let voice: f32 = active
                    .iter()
                    .filter_map(|(start, a)| {
                        f.checked_sub(*start)
                            .and_then(|k| a.get(usize::try_from(k).ok()?))
                    })
                    .sum();
                for &c in sample.as_chunks::<4>().0 {
                    let bg = f32::from_le_bytes(c);
                    let v = (bg * gain + voice).clamp(-1.0, 1.0);
                    out_bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            writer
                .write_all(&out_bytes)
                .with_context(|| format!("cannot write {}", out.display()))?;
            frame = chunk_end;
            active.retain(|(start, a)| start + a.len() as u64 > frame);
        }
        writer
            .flush()
            .with_context(|| format!("cannot write {}", out.display()))?;
        Ok(RawAudio {
            path: out.to_path_buf(),
            rate,
            channels: self.background.channels,
        })
    }
}

/// Sorts the parts and joins the ones that overlap.
fn merge(ducks: &mut [(u64, u64)]) -> Vec<(u64, u64)> {
    ducks.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &(s, e) in ducks.iter() {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// The background gain at `ms` for one lowered part.
fn duck_gain((start, end): (u64, u64), ms: u64) -> f32 {
    #[expect(clippy::cast_precision_loss, reason = "a ramp of 50 ms")]
    let ramp = |d: u64| 1.0 - (1.0 - DUCK_GAIN) * (1.0 - d as f32 / DUCK_RAMP_MS as f32);
    if ms >= start && ms < end {
        DUCK_GAIN
    } else if ms < start && start - ms < DUCK_RAMP_MS {
        ramp(start - ms)
    } else if ms >= end && ms - end < DUCK_RAMP_MS {
        ramp(ms - end)
    } else {
        1.0
    }
}

/// The gain that gives `line` the loudness (RMS) of `reference`, from 0.25 to 4.
/// A silent line keeps its level.
pub fn match_gain(reference: &[f32], line: &[f32]) -> f32 {
    let line_rms = rms(line);
    if line_rms <= f32::EPSILON {
        return 1.0;
    }
    (rms(reference) / line_rms).clamp(0.25, 4.0)
}

fn rms(audio: &[f32]) -> f32 {
    if audio.is_empty() {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, reason = "a sample count")]
    let n = audio.len() as f32;
    (audio.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
}

/// Reads the part of a raw file between `start_ms` and `end_ms`, as mono.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub fn read_mono(audio: &RawAudio, start_ms: u64, end_ms: u64) -> Result<Vec<f32>> {
    let channels = usize::from(audio.channels.max(1));
    let to_frame = |ms: u64| ms * u64::from(audio.rate) / 1000;
    let mut file =
        File::open(&audio.path).with_context(|| format!("cannot read {}", audio.path.display()))?;
    file.seek(SeekFrom::Start(to_frame(start_ms) * channels as u64 * 4))
        .with_context(|| format!("cannot read {}", audio.path.display()))?;
    let frames = usize::try_from(to_frame(end_ms).saturating_sub(to_frame(start_ms))).unwrap_or(0);
    let mut bytes = vec![0u8; frames * channels * 4];
    let n = read_full(&mut BufReader::new(file), &mut bytes).context("cannot read the audio")?;
    #[expect(clippy::cast_precision_loss, reason = "a channel count")]
    let mono = bytes[..n - n % (channels * 4)]
        .chunks_exact(channels * 4)
        .map(|f| {
            f.as_chunks::<4>()
                .0
                .iter()
                .map(|&c| f32::from_le_bytes(c))
                .sum::<f32>()
                / channels as f32
        })
        .collect();
    Ok(mono)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw stereo file with a constant value, at 1000 Hz so that 1 frame is 1 ms.
    fn constant(name: &str, value: f32, frames: usize) -> RawAudio {
        let path = std::env::temp_dir().join(name);
        let bytes: Vec<u8> = std::iter::repeat_n(value.to_le_bytes(), frames * 2)
            .flatten()
            .collect();
        std::fs::write(&path, bytes).unwrap();
        RawAudio {
            path,
            rate: 1000,
            channels: 2,
        }
    }

    fn read_left(audio: &RawAudio) -> Vec<f32> {
        std::fs::read(&audio.path)
            .unwrap()
            .as_chunks::<8>()
            .0
            .iter()
            .map(|f| f32::from_le_bytes([f[0], f[1], f[2], f[3]]))
            .collect()
    }

    #[test]
    fn duck_lowers_only_inside() {
        let mut mixer = Mixer::new(constant("verbacue-mix-duck.f32", 0.5, 1000));
        mixer.duck(200, 800);
        let out = mixer
            .finish(&std::env::temp_dir().join("verbacue-mix-duck-out.f32"))
            .unwrap();
        let left = read_left(&out);
        assert_eq!(left.len(), 1000);
        assert!((left[500] - 0.125).abs() < 1e-6, "{}", left[500]);
        assert!((left[0] - 0.5).abs() < 1e-6);
        assert!((left[999] - 0.5).abs() < 1e-6);
        // The ramp is between the two levels.
        assert!(left[175] < 0.5 && left[175] > 0.125, "{}", left[175]);
    }

    #[test]
    fn add_cuts_with_fade() {
        let mut mixer = Mixer::new(constant("verbacue-mix-cut.f32", 0.0, 2000));
        let tone = vec![0.5; 24_000];
        mixer.add(100, &tone, 1.0, Some(600));
        let out = mixer
            .finish(&std::env::temp_dir().join("verbacue-mix-cut-out.f32"))
            .unwrap();
        let left = read_left(&out);
        assert!(left[..95].iter().all(|&s| s.abs() < 1e-3));
        assert!((left[300] - 0.5).abs() < 0.02, "{}", left[300]);
        assert!(left[580] < 0.5, "the fade starts before the cut");
        assert!(
            left[600..].iter().all(|&s| s.abs() < 1e-3),
            "silent after the cut"
        );
    }

    #[test]
    fn match_gain_clamps() {
        let line = vec![0.1; 100];
        assert!((match_gain(&[0.0; 100], &line) - 0.25).abs() < 1e-6);
        assert!((match_gain(&[1.0; 100], &line) - 4.0).abs() < 1e-6);
        assert!((match_gain(&[0.2; 100], &line) - 2.0).abs() < 1e-5);
        assert!((match_gain(&[0.2; 100], &[0.0; 100]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn read_mono_reads_a_range() {
        let audio = constant("verbacue-mix-read.f32", 0.25, 1000);
        let part = read_mono(&audio, 100, 300).unwrap();
        assert_eq!(part.len(), 200);
        assert!(part.iter().all(|&s| (s - 0.25).abs() < 1e-6));
        assert_eq!(read_mono(&audio, 900, 2000).unwrap().len(), 100);
    }
}
