//! Runs XTTS-v2: text to speech in a studio voice or a cloned voice.
//!
//! The steps for one line:
//! 1. Clean the text and split it into parts that fit the text limit ([`text`]).
//! 2. For each part, the GPT makes audio codes one by one, until the stop code. Its
//!    hidden states ("latents") for these codes describe the speech.
//! 3. The vocoder turns the latents into 24 kHz audio in the voice of the speaker.

use std::{collections::BTreeMap, path::Path, time::Instant};

use ort::{
    session::{Session, SessionInputValue},
    value::{DynValue, Tensor},
};
use verbacue_core::{Context as _, Error, ErrorKind, Result};

use crate::{
    bpe::Tokenizer,
    mel, resample,
    safetensors::{self, Tensor as Table},
    sampling::{self, Rng, Settings},
    session::{self, model_error},
    text,
};

/// The sample rate of the speech.
pub const SAMPLE_RATE: u32 = 24_000;
/// The studio voice when the user chooses none.
pub const DEFAULT_STUDIO_VOICE: &str = "Ana Florence";

// The values in config.json.
const START_TEXT: usize = 261;
const STOP_TEXT: usize = 0;
const START_AUDIO: usize = 1024;
const STOP_AUDIO: usize = 1025;
const MAX_CODES: usize = 602;
const MAX_TEXT_TOKENS: usize = 402;
const LAYERS: usize = 30;
const HEADS: usize = 16;
const DIM: usize = 1024;
const COND_LEN: usize = 32;
const SPEAKER_DIM: usize = 512;

/// A voice: the GPT conditioning and the speaker embedding of the vocoder.
#[derive(Clone, Debug)]
pub struct Voice {
    /// `[32, 1024]` in row-major order.
    pub cond_latents: Vec<f32>,
    /// 512 values.
    pub speaker_emb: Vec<f32>,
}

/// The GPT latents of one line: one frame for each audio code. One frame is about
/// 43 ms of speech.
#[derive(Clone, Debug, Default)]
pub struct Latents {
    pub frames: Vec<[f32; DIM]>,
}

impl Latents {
    /// The latents with `factor` times the frames, by linear interpolation. A factor
    /// below 1 makes the speech shorter. The first and the last frames stay.
    #[must_use]
    pub fn stretch(&self, factor: f32) -> Latents {
        let n = self.frames.len();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "frame counts"
        )]
        #[expect(clippy::cast_precision_loss, reason = "frame counts")]
        let len = ((n as f32 * factor).round() as usize).max(usize::from(n > 0));
        if n < 2 || len < 2 {
            return Latents {
                frames: self.frames.iter().take(len).copied().collect(),
            };
        }
        #[expect(clippy::cast_precision_loss, reason = "frame counts")]
        let frames = (0..len)
            .map(|i| {
                let pos = i as f32 * (n - 1) as f32 / (len - 1) as f32;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "index"
                )]
                let j = (pos as usize).min(n - 2);
                let w = pos - j as f32;
                let (a, b) = (&self.frames[j], &self.frames[j + 1]);
                std::array::from_fn(|k| a[k] + (b[k] - a[k]) * w)
            })
            .collect();
        Latents { frames }
    }
}

/// The names of the studio voices in a model folder, sorted, without loading the model.
///
/// # Errors
/// Returns an error if `speakers.safetensors` cannot be read.
pub fn studio_voices_in(dir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = safetensors::load(&dir.join("speakers.safetensors"))?
        .into_keys()
        .filter_map(|k| k.strip_suffix(".cond_latents").map(str::to_string))
        .collect();
    names.sort();
    Ok(names)
}

/// XTTS-v2, loaded from the files of the `xtts-v2` or `xtts-v2-int8` model.
pub struct Xtts {
    conditioning: Session,
    speaker: Session,
    gpt: Session,
    vocoder: Session,
    text_emb: Table,
    text_pos: Table,
    mel_emb: Table,
    mel_pos: Table,
    voices: BTreeMap<String, Voice>,
    tokenizer: Tokenizer,
    mel_stats: Vec<f32>,
    sampling: Settings,
}

impl Xtts {
    /// Loads the model from `dir`. With `int8`, it uses `gpt_int8.onnx`, else `gpt.onnx`.
    ///
    /// # Errors
    /// Returns an error if a file is missing or broken.
    pub fn load(dir: &Path, int8: bool) -> Result<Xtts> {
        let start = Instant::now();
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("config.json"))
                .with_context(|| format!("cannot read {}", dir.join("config.json").display()))?,
        )
        .context("config.json is not JSON")?;
        #[expect(clippy::cast_possible_truncation, reason = "f32 model values")]
        let mel_stats: Vec<f32> = config["cond_mel"]["mel_stats"]
            .as_array()
            .context("config.json has no mel_stats")?
            .iter()
            .filter_map(|v| v.as_f64().map(|v| v as f32))
            .collect();
        if mel_stats.len() != 80 {
            return Err(Error::new(
                ErrorKind::Model,
                "config.json needs 80 mel_stats",
            ));
        }

        let mut emb = safetensors::load(&dir.join("embeddings.safetensors"))?;
        let mut table = |name: &str| {
            emb.remove(name)
                .filter(|t| t.shape.len() == 2 && t.shape[1] == DIM)
                .with_context(|| format!("embeddings.safetensors has no {name}"))
        };
        let (text_emb, text_pos) = (table("text_embedding")?, table("text_pos_embedding")?);
        let (mel_emb, mel_pos) = (table("mel_embedding")?, table("mel_pos_embedding")?);

        let mut voices = BTreeMap::new();
        let mut speakers = safetensors::load(&dir.join("speakers.safetensors"))?;
        let names: Vec<String> = speakers
            .keys()
            .filter_map(|k| k.strip_suffix(".cond_latents").map(str::to_string))
            .collect();
        for name in names {
            let cond = speakers.remove(&format!("{name}.cond_latents"));
            let spk = speakers.remove(&format!("{name}.speaker_emb"));
            if let (Some(c), Some(s)) = (cond, spk)
                && c.data.len() == COND_LEN * DIM
                && s.data.len() == SPEAKER_DIM
            {
                voices.insert(
                    name,
                    Voice {
                        cond_latents: c.data,
                        speaker_emb: s.data,
                    },
                );
            }
        }

        let gpt = if int8 { "gpt_int8.onnx" } else { "gpt.onnx" };
        // XTTS-v2 runs on the CPU on macOS. CoreML gives `inf` in the conditioning
        // encoder, refuses the KV cache of the GPT, and is slower for the vocoder.
        let xtts = Xtts {
            conditioning: session::open(&dir.join("conditioning.onnx"), false)?,
            speaker: session::open(&dir.join("speaker.onnx"), false)?,
            gpt: session::open(&dir.join(gpt), false)?,
            vocoder: session::open(&dir.join("vocoder.onnx"), false)?,
            text_emb,
            text_pos,
            mel_emb,
            mel_pos,
            voices,
            tokenizer: Tokenizer::load(&dir.join("vocab.json"))?,
            mel_stats,
            sampling: Settings::default(),
        };
        tracing::debug!(
            "loaded XTTS-v2 from {} in {:.1?}",
            dir.display(),
            start.elapsed()
        );
        Ok(xtts)
    }

    /// The names of the studio voices, sorted.
    pub fn studio_voices(&self) -> Vec<&str> {
        self.voices.keys().map(String::as_str).collect()
    }

    /// A studio voice by name.
    pub fn studio_voice(&self, name: &str) -> Option<Voice> {
        self.voices.get(name).cloned()
    }

    /// Makes a voice from a sample of clean speech at 24 kHz. The GPT conditioning uses
    /// the first 6 seconds and the speaker embedding the first 30 seconds.
    ///
    /// # Errors
    /// Returns an error if the sample is shorter than 0.33 seconds or a model fails.
    pub fn clone_voice(&mut self, audio_24k: &[f32]) -> Result<Voice> {
        let a22 = resample::resample(
            &audio_24k[..audio_24k.len().min(24_000 * 6)],
            24_000,
            22_050,
        );
        if a22.len() < 22_050 / 3 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "the voice sample is shorter than 0.33 s",
            ));
        }
        let m = mel::cond_mel(&a22, &self.mel_stats);
        let cond_latents = run_one(
            &mut self.conditioning,
            "mel",
            Tensor::from_array(([1, m.n_mels, m.frames], m.data)).map_err(model_error)?,
            COND_LEN * DIM,
        )?;
        let a16 = resample::resample(
            &audio_24k[..audio_24k.len().min(24_000 * 30)],
            24_000,
            16_000,
        );
        let m = mel::speaker_mel(&a16);
        let speaker_emb = run_one(
            &mut self.speaker,
            "mel",
            Tensor::from_array(([1, m.n_mels, m.frames], m.data)).map_err(model_error)?,
            SPEAKER_DIM,
        )?;
        Ok(Voice {
            cond_latents,
            speaker_emb,
        })
    }

    /// The latents for one subtitle line. The line is cleaned and split into parts that
    /// fit the text limit, and the latents of the parts are joined. A line with nothing
    /// to speak gives no frames and does not run the GPT.
    ///
    /// # Errors
    /// Returns an error if the GPT fails, or with [`ErrorKind::Other`] if a part
    /// reaches the code limit without the stop code.
    pub fn latents(&mut self, line: &str, lang: &str, voice: &Voice, seed: u64) -> Result<Latents> {
        let Some(clean) = text::clean(line, lang) else {
            return Ok(Latents::default());
        };
        let mut rng = Rng::new(seed);
        let mut frames = Vec::new();
        for part in text::split(&clean, lang) {
            frames.extend(self.generate(&part, lang, voice, &mut rng)?);
        }
        Ok(Latents { frames })
    }

    /// The GPT loop for one clean part of a line.
    fn generate(
        &mut self,
        part: &str,
        lang: &str,
        voice: &Voice,
        rng: &mut Rng,
    ) -> Result<Vec<[f32; DIM]>> {
        let start = Instant::now();
        let tokens = self.tokenizer.encode(part, lang);
        if tokens.len() + 2 > MAX_TEXT_TOKENS {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("the text has too many tokens: {part}"),
            ));
        }
        // The prefix: the voice, the text between its start and stop tokens, and the
        // start audio code. The token and position embeddings are added here.
        let mut embeds = voice.cond_latents.clone();
        let text_ids = std::iter::once(START_TEXT)
            .chain(tokens.iter().map(|&t| usize::try_from(t).unwrap_or(1)))
            .chain(std::iter::once(STOP_TEXT));
        for (pos, id) in text_ids.enumerate() {
            embeds.extend(add(row(&self.text_emb, id), row(&self.text_pos, pos)));
        }
        embeds.extend(add(row(&self.mel_emb, START_AUDIO), row(&self.mel_pos, 0)));

        let names: Vec<String> = (0..LAYERS)
            .flat_map(|i| [format!("k_{i}"), format!("v_{i}")])
            .collect();
        let mut past: Vec<DynValue> = names
            .iter()
            .map(|_| {
                Tensor::<f32>::from_array(([1, HEADS, 0, DIM / HEADS], Vec::new()))
                    .map(Tensor::into_dyn)
            })
            .collect::<std::result::Result<_, _>>()
            .map_err(model_error)?;
        // Hugging Face sees a prefix of id 1 and the start code as the codes so far.
        let mut seen = vec![1, START_AUDIO];
        let mut frames = Vec::new();
        for step in 0..MAX_CODES {
            let rows = embeds.len() / DIM;
            let mut inputs: Vec<(String, SessionInputValue)> = Vec::with_capacity(1 + past.len());
            inputs.push((
                "embeds".into(),
                Tensor::from_array(([1, rows, DIM], std::mem::take(&mut embeds)))
                    .map_err(model_error)?
                    .into(),
            ));
            for (name, value) in names.iter().zip(past.drain(..)) {
                inputs.push((format!("past_{name}"), value.into()));
            }
            let mut out = self.gpt.run(inputs).map_err(model_error)?;
            let (_, logits) = out["logits"]
                .try_extract_tensor::<f32>()
                .map_err(model_error)?;
            let mut logits = logits.to_vec();
            let (_, latents) = out["latents"]
                .try_extract_tensor::<f32>()
                .map_err(model_error)?;
            let last: [f32; DIM] = latents[latents.len() - DIM..]
                .try_into()
                .ok()
                .context("the GPT gave no latents")?;
            for name in &names {
                past.push(
                    out.remove(format!("present_{name}"))
                        .context("the GPT has no KV output")?,
                );
            }
            drop(out);

            frames.push(last);
            let code = sampling::sample(&mut logits, &seen, self.sampling, rng);
            if code == STOP_AUDIO {
                tracing::debug!(
                    "{} codes for {part:?} in {:.1?}",
                    frames.len(),
                    start.elapsed()
                );
                return Ok(frames);
            }
            if !seen.contains(&code) {
                seen.push(code);
            }
            embeds = add(row(&self.mel_emb, code), row(&self.mel_pos, step + 1)).collect();
        }
        Err(Error::new(
            ErrorKind::Other,
            format!("no stop code after {MAX_CODES} codes: {part}"),
        ))
    }

    /// Turns latents into speech at [`SAMPLE_RATE`].
    ///
    /// # Errors
    /// Returns an error if the vocoder fails.
    pub fn vocode(&mut self, latents: &Latents, voice: &Voice) -> Result<Vec<f32>> {
        if latents.frames.is_empty() {
            return Ok(Vec::new());
        }
        let data: Vec<f32> = latents.frames.iter().flatten().copied().collect();
        let inputs: Vec<(&str, SessionInputValue)> = vec![
            (
                "latents",
                Tensor::from_array(([1, latents.frames.len(), DIM], data))
                    .map_err(model_error)?
                    .into(),
            ),
            (
                "speaker_emb",
                Tensor::from_array(([1, SPEAKER_DIM, 1], voice.speaker_emb.clone()))
                    .map_err(model_error)?
                    .into(),
            ),
        ];
        let out = self.vocoder.run(inputs).map_err(model_error)?;
        let (_, wav) = out["wav"]
            .try_extract_tensor::<f32>()
            .map_err(model_error)?;
        Ok(wav.to_vec())
    }
}

/// Runs a graph with one input and returns its one output, which must have `len` values.
fn run_one(session: &mut Session, input: &str, value: Tensor<f32>, len: usize) -> Result<Vec<f32>> {
    let out = session
        .run(vec![(input, SessionInputValue::from(value))])
        .map_err(model_error)?;
    let (_, data) = out[0].try_extract_tensor::<f32>().map_err(model_error)?;
    if data.len() != len {
        return Err(Error::new(
            ErrorKind::Model,
            format!("expected {len} values, got {}", data.len()),
        ));
    }
    Ok(data.to_vec())
}

/// Row `i` of an embedding table. An id outside the table gives row 0.
fn row(table: &Table, i: usize) -> &[f32] {
    let i = if i < table.shape[0] { i } else { 0 };
    &table.data[i * DIM..(i + 1) * DIM]
}

fn add<'a>(a: &'a [f32], b: &'a [f32]) -> impl Iterator<Item = f32> + 'a {
    a.iter().zip(b).map(|(x, y)| x + y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize) -> Latents {
        #[expect(clippy::cast_precision_loss, reason = "test")]
        Latents {
            frames: (0..n).map(|i| [i as f32; DIM]).collect(),
        }
    }

    #[test]
    fn stretch_halves_frames() {
        let input = frames(100);
        let out = input.stretch(0.5);
        assert_eq!(out.frames.len(), 50);
        assert_eq!(out.frames[0], input.frames[0]);
        assert_eq!(out.frames[49], input.frames[99]);
        // The values in between are on the line from the first to the last frame.
        assert!((out.frames[25][0] - 25.0 * 99.0 / 49.0).abs() < 1e-4);
    }

    #[test]
    fn stretch_keeps_short_inputs() {
        assert!(frames(0).stretch(0.5).frames.is_empty());
        assert_eq!(frames(1).stretch(2.0).frames.len(), 1);
        assert_eq!(frames(3).stretch(0.1).frames.len(), 1);
        assert_eq!(frames(10).stretch(1.5).frames.len(), 15);
    }

    fn export_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out")
    }

    fn rms(audio: &[f32]) -> f32 {
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let n = audio.len() as f32;
        (audio.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn speaks_hungarian() {
        let mut xtts = Xtts::load(&export_dir(), false).unwrap();
        let voice = xtts.studio_voice(DEFAULT_STUDIO_VOICE).unwrap();
        let latents = xtts
            .latents("Jó napot kívánok, hogy van?", "hu", &voice, 1)
            .unwrap();
        let audio = xtts.vocode(&latents, &voice).unwrap();
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let seconds = audio.len() as f32 / SAMPLE_RATE as f32;
        assert!((1.0..4.0).contains(&seconds), "{seconds} s");
        assert!(rms(&audio) > 0.01, "rms {}", rms(&audio));
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn a_long_line_is_split_and_joined() {
        let mut xtts = Xtts::load(&export_dir(), true).unwrap();
        let voice = xtts.studio_voice(DEFAULT_STUDIO_VOICE).unwrap();
        let line = "Tegnap reggel korán keltem, mert sok dolgom volt a városban. Először a \
            postára mentem, aztán a piacra, ahol friss zöldséget és gyümölcsöt vettem. \
            Délben találkoztam egy régi barátommal, és együtt ebédeltünk egy kis \
            étteremben a folyó partján, ahol nagyon finom halászlét főztek.";
        assert!(line.chars().count() > text::char_limit("hu"));
        let latents = xtts.latents(line, "hu", &voice, 1).unwrap();
        // About 13 characters per second; a cut text would be much shorter.
        #[expect(clippy::cast_precision_loss, reason = "test")]
        let seconds = latents.frames.len() as f32 * 1024.0 / SAMPLE_RATE as f32;
        assert!(seconds > 12.0, "{seconds} s");
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn a_line_without_speech_gives_no_frames() {
        let mut xtts = Xtts::load(&export_dir(), true).unwrap();
        let voice = xtts.studio_voice(DEFAULT_STUDIO_VOICE).unwrap();
        for line in ["[zene]", "♪ la la la ♪", "..."] {
            assert!(
                xtts.latents(line, "hu", &voice, 1)
                    .unwrap()
                    .frames
                    .is_empty(),
                "{line}"
            );
        }
        assert!(xtts.vocode(&Latents::default(), &voice).unwrap().is_empty());
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn clone_matches_python() {
        let dir = export_dir();
        let fx = safetensors::load(&dir.join("fixtures/clone.safetensors")).unwrap();
        let (audio, rate) = crate::wav::read(&dir.join("fixtures/reference.wav")).unwrap();
        assert_eq!(rate, SAMPLE_RATE);
        let mut xtts = Xtts::load(&dir, true).unwrap();
        let voice = xtts.clone_voice(&audio).unwrap();
        for (got, want, name) in [
            (
                &voice.cond_latents,
                &fx["cond_latents"].data,
                "cond_latents",
            ),
            (&voice.speaker_emb, &fx["speaker_emb"].data, "speaker_emb"),
        ] {
            let diff = got
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(diff < 1e-2, "{name}: max diff {diff}");
        }
        assert!(xtts.clone_voice(&audio[..1000]).is_err());
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
    fn studio_voices_are_sorted() {
        let xtts = Xtts::load(&export_dir(), true).unwrap();
        let names = xtts.studio_voices();
        assert!(names.len() > 10 && names.is_sorted(), "{names:?}");
        assert!(names.contains(&DEFAULT_STUDIO_VOICE));
    }
}
