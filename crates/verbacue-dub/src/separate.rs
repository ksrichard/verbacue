//! Splits audio into speech and background with the HTDemucs vocals model.
//!
//! The model takes chunks of exactly [`CHUNK`] stereo frames at 44.1 kHz. The chunks
//! overlap by 25 %, and a linear crossfade joins them. The code reads and writes the
//! audio files in a stream, so the memory use does not grow with the film length.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Read as _, Write as _},
    path::Path,
};

use ort::{session::Session, value::Tensor};
use verbacue_core::{Context as _, Error, ErrorKind, Result};
use verbacue_media::audio::RawAudio;

use crate::{
    run::Events,
    session::{self, model_error},
};

/// The chunk length of the model, in frames (7.8 s at 44.1 kHz).
pub const CHUNK: usize = 343_980;
const OVERLAP: usize = CHUNK / 4;
const STRIDE: usize = CHUNK - OVERLAP;
/// The index of the vocals in the model output (drums, bass, other, vocals).
const VOCALS: usize = 3;

/// A model that finds the speech in one chunk. Input and output are planar stereo:
/// the left channel, then the right channel, each [`CHUNK`] frames.
pub trait Separator {
    fn vocals(&mut self, chunk: &[f32]) -> Result<Vec<f32>>;
}

/// HTDemucs as an ONNX graph.
pub struct Htdemucs(Session);

impl Htdemucs {
    /// Opens `htdemucs_vocals.onnx`.
    ///
    /// # Errors
    /// Returns an error if the file is not a valid ONNX graph.
    pub fn open(path: &Path) -> Result<Htdemucs> {
        Ok(Htdemucs(session::open(path)?))
    }
}

impl Separator for Htdemucs {
    fn vocals(&mut self, chunk: &[f32]) -> Result<Vec<f32>> {
        let input = Tensor::from_array(([1, 2, CHUNK], chunk.to_vec())).map_err(model_error)?;
        let out = self
            .0
            .run(ort::inputs!["mix" => input])
            .map_err(model_error)?;
        let (_, stems) = out["stems"]
            .try_extract_tensor::<f32>()
            .map_err(model_error)?;
        stems
            .get(VOCALS * 2 * CHUNK..(VOCALS + 1) * 2 * CHUNK)
            .map(<[f32]>::to_vec)
            .ok_or_else(|| Error::new(ErrorKind::Model, "the separation model gave too few stems"))
    }
}

/// Splits the stereo `mix` into speech and background (`mix - speech`), written as raw
/// files in the format of `mix`.
///
/// # Errors
/// Returns an error if a file cannot be read or written, the model fails, or the user
/// cancels.
pub fn separate(
    mix: &RawAudio,
    separator: &mut dyn Separator,
    speech_out: &Path,
    background_out: &Path,
    events: &dyn Events,
) -> Result<(RawAudio, RawAudio)> {
    if mix.channels != 2 {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "separation needs stereo audio",
        ));
    }
    let total = usize::try_from(
        std::fs::metadata(&mix.path)
            .with_context(|| format!("cannot read {}", mix.path.display()))?
            .len()
            / 8,
    )
    .unwrap_or(usize::MAX);
    let chunks = total.div_ceil(STRIDE).max(1);

    let mut reader = BufReader::new(
        File::open(&mix.path).with_context(|| format!("cannot read {}", mix.path.display()))?,
    );
    let create = |p: &Path| {
        File::create(p)
            .map(BufWriter::new)
            .with_context(|| format!("cannot write {}", p.display()))
    };
    let (mut speech_file, mut background_file) = (create(speech_out)?, create(background_out)?);

    // The input frames from `start` on, interleaved, and the weighted speech sums and
    // weights for the same frames.
    let mut window_mix: Vec<f32> = Vec::with_capacity(2 * CHUNK);
    let mut sums: Vec<f32> = Vec::with_capacity(2 * CHUNK);
    let mut weights: Vec<f32> = Vec::with_capacity(CHUNK);
    let mut start = 0usize;
    let mut planar = vec![0.0f32; 2 * CHUNK];

    for i in 0..chunks {
        if events.cancelled() {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        }
        events.progress(i as u64, chunks as u64);
        let end = (start + CHUNK).min(total);
        let len = end - start;
        // Read the frames of this chunk that are not in the buffer yet.
        let have = window_mix.len() / 2;
        read_frames(&mut reader, &mut window_mix, len - have.min(len))?;
        let len = (window_mix.len() / 2).min(len);
        sums.resize(2 * len, 0.0);
        weights.resize(len, 0.0);

        for (f, pair) in window_mix[..2 * len].as_chunks::<2>().0.iter().enumerate() {
            planar[f] = pair[0];
            planar[CHUNK + f] = pair[1];
        }
        planar[len..CHUNK].fill(0.0);
        planar[CHUNK + len..].fill(0.0);
        let vocals = separator.vocals(&planar)?;

        let first = i == 0;
        let last = start + len >= total;
        for f in 0..len {
            let w = fade(f, len, first, last);
            sums[2 * f] += vocals[f] * w;
            sums[2 * f + 1] += vocals[CHUNK + f] * w;
            weights[f] += w;
        }

        // The frames before the next chunk are final.
        let done = if last { len } else { STRIDE.min(len) };
        write_frames(
            &mut speech_file,
            &mut background_file,
            &window_mix[..2 * done],
            &sums[..2 * done],
            &weights[..done],
        )?;
        window_mix.drain(..2 * done);
        sums.drain(..2 * done);
        weights.drain(..done);
        start += done;
        if last {
            break;
        }
    }
    events.progress(chunks as u64, chunks as u64);
    speech_file
        .flush()
        .context("cannot write the speech audio")?;
    background_file
        .flush()
        .context("cannot write the background audio")?;
    let raw = |p: &Path| RawAudio {
        path: p.to_path_buf(),
        rate: mix.rate,
        channels: 2,
    };
    Ok((raw(speech_out), raw(background_out)))
}

/// The crossfade weight of frame `f` in a chunk of `len` frames. The first chunk has
/// no fade-in and the last chunk no fade-out. The weight is never 0.
fn fade(f: usize, len: usize, first: bool, last: bool) -> f32 {
    #[expect(clippy::cast_precision_loss, reason = "frame positions")]
    let ramp = |k: usize| (k + 1) as f32 / (OVERLAP + 1) as f32;
    let mut w = 1.0f32;
    if !first && f < OVERLAP {
        w = w.min(ramp(f));
    }
    if !last && len - 1 - f < OVERLAP {
        w = w.min(ramp(len - 1 - f));
    }
    w
}

fn read_frames(reader: &mut BufReader<File>, out: &mut Vec<f32>, frames: usize) -> Result<()> {
    let mut bytes = vec![0u8; frames * 8];
    let mut n = 0;
    while n < bytes.len() {
        match reader
            .read(&mut bytes[n..])
            .context("cannot read the audio")?
        {
            0 => break,
            k => n += k,
        }
    }
    out.extend(
        bytes[..n - n % 8]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b)),
    );
    Ok(())
}

fn write_frames(
    speech: &mut BufWriter<File>,
    background: &mut BufWriter<File>,
    mix: &[f32],
    sums: &[f32],
    weights: &[f32],
) -> Result<()> {
    let mut s_bytes = Vec::with_capacity(mix.len() * 4);
    let mut b_bytes = Vec::with_capacity(mix.len() * 4);
    for (i, (&m, &sum)) in mix.iter().zip(sums).enumerate() {
        let v = sum / weights[i / 2].max(1e-8);
        s_bytes.extend_from_slice(&v.to_le_bytes());
        b_bytes.extend_from_slice(&(m - v).to_le_bytes());
    }
    speech
        .write_all(&s_bytes)
        .context("cannot write the speech audio")?;
    background
        .write_all(&b_bytes)
        .context("cannot write the background audio")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake model that says the whole input is speech.
    struct Identity;

    impl Separator for Identity {
        fn vocals(&mut self, chunk: &[f32]) -> Result<Vec<f32>> {
            Ok(chunk.to_vec())
        }
    }

    fn read(path: &Path) -> Vec<f32> {
        std::fs::read(path)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect()
    }

    #[test]
    fn overlap_add_is_identity() {
        // 2.6 chunks: several borders, and a length that is not a chunk multiple.
        let frames = CHUNK * 2 + CHUNK * 3 / 5;
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let mix: Vec<f32> = (0..2 * frames)
            .map(|i| ((i as f32) * 0.001).sin() * 0.5)
            .collect();
        let dir = std::env::temp_dir();
        let path = dir.join("verbacue-separate-mix.f32");
        std::fs::write(
            &path,
            mix.iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
        )
        .unwrap();
        let input = RawAudio {
            path,
            rate: 44_100,
            channels: 2,
        };
        let (speech, background) = separate(
            &input,
            &mut Identity,
            &dir.join("verbacue-separate-speech.f32"),
            &dir.join("verbacue-separate-bg.f32"),
            &(),
        )
        .unwrap();
        let s = read(&speech.path);
        let b = read(&background.path);
        assert_eq!(s.len(), mix.len());
        for (i, (got, want)) in s.iter().zip(&mix).enumerate() {
            assert!(
                (got - want).abs() < 1e-5,
                "frame {}: {got} vs {want}",
                i / 2
            );
        }
        assert!(b.iter().all(|v| v.abs() < 1e-5));
    }

    /// 30 s of `say` speech over two quiet tones, as raw 44.1 kHz stereo.
    fn speech_over_music(dir: &Path) -> Option<RawAudio> {
        let speech = dir.join("verbacue-separate-say.aiff");
        let text = "The station was empty when I arrived. I waited for two hours, \
            but nobody came. Then the rain started, and I walked home alone.";
        let said = std::process::Command::new("say")
            .args(["-v", "Samantha", "-o"])
            .arg(&speech)
            .arg(text)
            .status()
            .is_ok_and(|s| s.success());
        let video = dir.join("verbacue-separate-clip.mkv");
        let made = said
            && std::process::Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error", "-i"])
                .arg(&speech)
                .args(["-f", "lavfi", "-i", "sine=frequency=220:duration=30"])
                .args(["-f", "lavfi", "-i", "sine=frequency=330:duration=30"])
                .args(["-filter_complex", "[0]apad[s];[1][2]amix=inputs=2,volume=0.3[m];[s][m]amix=inputs=2:duration=longest:normalize=0[a]"])
                .args(["-map", "[a]", "-t", "30", "-c:a", "pcm_s16le"])
                .arg(&video)
                .status()
                .is_ok_and(|s| s.success());
        if !made {
            eprintln!("skipped: needs the say and ffmpeg commands");
            return None;
        }
        let path = dir.join("verbacue-separate-clip.f32");
        verbacue_media::audio::decode_to_file(&video, 0, 44_100, &path, &()).unwrap();
        Some(RawAudio {
            path,
            rate: 44_100,
            channels: 2,
        })
    }

    fn rms_between(audio: &RawAudio, from_ms: u64, to_ms: u64) -> f32 {
        let part = crate::mix::read_mono(audio, from_ms, to_ms).unwrap();
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let n = part.len() as f32;
        (part.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn separates_voice() {
        let dir = std::env::temp_dir();
        let Some(mix) = speech_over_music(&dir) else {
            return;
        };
        let model = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/xtts-export/out/htdemucs_vocals.onnx");
        let start = std::time::Instant::now();
        let (speech, background) = separate(
            &mix,
            &mut Htdemucs::open(&model).unwrap(),
            &dir.join("verbacue-separate-real-speech.f32"),
            &dir.join("verbacue-separate-real-bg.f32"),
            &(),
        )
        .unwrap();
        eprintln!("separated 30 s in {:.1?}", start.elapsed());
        // The speech is in the first seconds; the tones go on to the end.
        let (mix_rms, bg_rms) = (
            rms_between(&mix, 1000, 4000),
            rms_between(&background, 1000, 4000),
        );
        let speech_rms = rms_between(&speech, 1000, 4000);
        eprintln!("mix {mix_rms:.3}, background {bg_rms:.3}, speech {speech_rms:.3}");
        assert!(
            bg_rms < 0.3 * mix_rms,
            "background {bg_rms} vs mix {mix_rms}"
        );
        // Without speech, the background keeps the tones.
        assert!(rms_between(&background, 25_000, 29_000) > 0.5 * rms_between(&mix, 25_000, 29_000));
    }

    #[test]
    fn fade_is_never_zero() {
        assert!((fade(0, CHUNK, true, false) - 1.0).abs() < 1e-6);
        assert!(fade(0, CHUNK, false, false) > 0.0);
        assert!(fade(CHUNK - 1, CHUNK, false, false) > 0.0);
        assert!((fade(CHUNK - 1, CHUNK, false, true) - 1.0).abs() < 1e-6);
    }
}
