//! Finds the speakers of a film and a voice sample for each one.
//!
//! The CAM++ speaker model gives a voiceprint for each 1.5 s window of each line. One
//! voiceprint per line is too noisy in a film: lines are short, emotional, and some
//! have two speakers. Average-linkage clustering on the cosine distance groups the
//! windows. Groups with little speech then join the nearest large group, and a few
//! centroid passes settle the groups. Each line gets the speaker of most of its windows.

use std::path::Path;

use ort::{session::Session, value::Tensor};
use verbacue_core::{Error, ErrorKind, Result};
use verbacue_media::audio::RawAudio;

use crate::{fbank, fit::Slot, mix, resample, run::Events, session, xtts::run_one};

/// The file of the CAM++ speaker model (3D-Speaker, VoxCeleb, Apache-2.0).
pub const MODEL_FILE: &str = "campplus.onnx";
const DIM: usize = 512;
/// The cosine distance at which two groups are different speakers.
pub const THRESHOLD: f32 = 0.7;
/// Lines shorter than this give no voiceprint, and take the speaker of a neighbour.
pub const MIN_LINE_MS: u64 = 400;
const WINDOW_MS: u64 = 1500;
const HOP_MS: u64 = 750;
/// A group needs this much speech to be an actor. Smaller groups join the nearest actor.
pub const MIN_ACTOR_MS: u64 = 8000;
const PASSES: usize = 3;
/// The longest voice sample.
pub const SAMPLE_MS: u64 = 30_000;
/// Shorter lines go into a sample only when the longer ones give too little speech.
const MIN_SAMPLE_LINE_MS: u64 = 1000;
const MIN_SAMPLE_MS: u64 = 6000;

/// The CAM++ speaker model.
pub struct SpeakerModel(Session);

impl SpeakerModel {
    /// Opens `campplus.onnx`.
    ///
    /// # Errors
    /// Returns an error if the file is not a valid ONNX graph.
    pub fn open(path: &Path) -> Result<SpeakerModel> {
        Ok(SpeakerModel(session::open(path)?))
    }

    /// The voiceprint of 16 kHz speech, with length 1. `None` for audio shorter than
    /// one 25 ms frame.
    ///
    /// # Errors
    /// Returns an error if the model fails.
    pub fn embed(&mut self, audio_16k: &[f32]) -> Result<Option<Vec<f32>>> {
        let (frames, data) = fbank::fbank(audio_16k);
        if frames == 0 {
            return Ok(None);
        }
        let input =
            Tensor::from_array(([1, frames, fbank::BINS], data)).map_err(session::model_error)?;
        let v = run_one(&mut self.0, "x", input, DIM)?;
        Ok(Some(normalised(v)))
    }
}

/// The speakers of the slots of a film.
#[derive(Debug, Clone, PartialEq)]
pub struct Speakers {
    /// The speaker of each slot, numbered from 0 in the order of their first slot.
    /// All are `None` only if no slot has a voiceprint.
    pub of_slot: Vec<Option<usize>>,
    /// How typical each slot is for its speaker: the cosine similarity of its voiceprint
    /// to the mean of the speaker. 0 for a slot without a voiceprint.
    pub typical: Vec<f32>,
    pub count: usize,
}

/// Finds the speaker of each slot in `speech`.
///
/// # Errors
/// Returns an error if the audio cannot be read, the model fails, or the user cancels.
pub fn find(
    model: &mut SpeakerModel,
    slots: &[Slot],
    speech: &RawAudio,
    events: &dyn Events,
) -> Result<Speakers> {
    let mut windows: Vec<(usize, u64)> = Vec::new();
    let mut vectors: Vec<Vec<f32>> = Vec::new();
    for (n, slot) in slots.iter().enumerate() {
        if events.cancelled() {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        }
        events.progress(n as u64, slots.len() as u64);
        if slot.end_ms - slot.start_ms < MIN_LINE_MS {
            continue;
        }
        let audio = mix::read_mono(speech, slot.start_ms, slot.end_ms)?;
        let audio = resample::resample(&audio, speech.rate, fbank::SAMPLE_RATE);
        let per_ms = fbank::SAMPLE_RATE as usize / 1000;
        for (from, to) in windows_of(slot.end_ms - slot.start_ms) {
            let part = &audio[(from as usize * per_ms).min(audio.len())
                ..(to as usize * per_ms).min(audio.len())];
            if let Some(v) = model.embed(part)? {
                windows.push((n, to - from));
                vectors.push(v);
            }
        }
    }
    let ms: Vec<u64> = windows.iter().map(|w| w.1).collect();
    let labels = assign(&vectors, &ms, THRESHOLD);
    Ok(by_slot(slots.len(), &windows, &vectors, &labels))
}

/// The windows of a line of `len` ms, as (start, end) in the line. A short line is one
/// window; the last window ends at the end of the line.
fn windows_of(len: u64) -> Vec<(u64, u64)> {
    if len <= WINDOW_MS {
        return vec![(0, len)];
    }
    let mut out: Vec<(u64, u64)> = (0..)
        .map(|k| k * HOP_MS)
        .take_while(|&s| s + WINDOW_MS <= len)
        .map(|s| (s, s + WINDOW_MS))
        .collect();
    if out.last().is_some_and(|&(_, e)| len - e > HOP_MS / 2) {
        out.push((len - WINDOW_MS, len));
    }
    out
}

/// Groups the voiceprints. `ms` is the speech time of each one. Returns the actor of
/// each voiceprint, numbered from 0.
fn assign(vectors: &[Vec<f32>], ms: &[u64], threshold: f32) -> Vec<usize> {
    if vectors.is_empty() {
        return Vec::new();
    }
    let clusters = cluster(vectors, threshold);
    let mut speech = vec![0u64; vectors.len()];
    for (&c, &m) in clusters.iter().zip(ms) {
        speech[c] += m;
    }
    let mut actors: Vec<usize> = (0..vectors.len())
        .filter(|&c| speech[c] >= MIN_ACTOR_MS)
        .collect();
    if actors.is_empty() {
        actors = (0..vectors.len())
            .max_by_key(|&c| speech[c])
            .into_iter()
            .collect();
    }
    // Each voiceprint starts with its own group if that is an actor.
    let mut labels: Vec<Option<usize>> = clusters
        .iter()
        .map(|c| actors.iter().position(|a| a == c))
        .collect();
    for _ in 0..PASSES {
        let mut sums = vec![vec![0.0f32; DIM]; actors.len()];
        for (v, l) in vectors.iter().zip(&labels) {
            if let Some(l) = *l {
                sums[l].iter_mut().zip(v).for_each(|(s, x)| *s += x);
            }
        }
        let centroids: Vec<Vec<f32>> = sums.into_iter().map(normalised).collect();
        labels = vectors
            .iter()
            .map(|v| nearest(v, &centroids).map(|(k, _)| k))
            .collect();
    }
    labels.into_iter().map(|l| l.unwrap_or(0)).collect()
}

/// The index and similarity of the centroid nearest to `v`.
fn nearest(v: &[f32], centroids: &[Vec<f32>]) -> Option<(usize, f32)> {
    centroids
        .iter()
        .map(|c| dot(v, c))
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The speaker of each slot by a vote of its windows, the slots without a voiceprint
/// filled from their neighbours, and how typical each slot is.
fn by_slot(
    slots: usize,
    windows: &[(usize, u64)],
    vectors: &[Vec<f32>],
    labels: &[usize],
) -> Speakers {
    let count = labels.iter().max().map_or(0, |m| m + 1);
    let mut votes = vec![vec![0u64; count]; slots];
    let mut means = vec![vec![0.0f32; DIM]; slots];
    let mut speaker_sums = vec![vec![0.0f32; DIM]; count];
    for (((slot, ms), v), &l) in windows.iter().zip(vectors).zip(labels) {
        votes[*slot][l] += ms;
        means[*slot].iter_mut().zip(v).for_each(|(s, x)| *s += x);
        speaker_sums[l].iter_mut().zip(v).for_each(|(s, x)| *s += x);
    }
    let centroids: Vec<Vec<f32>> = speaker_sums.into_iter().map(normalised).collect();
    let mut labels: Vec<Option<usize>> = votes
        .iter()
        .map(|v| {
            (0..count)
                .filter(|&k| v[k] > 0)
                .max_by_key(|&k| (v[k], std::cmp::Reverse(k)))
        })
        .collect();
    let typical = labels
        .iter()
        .zip(means)
        .map(|(l, m)| l.map_or(0.0, |l| dot(&normalised(m), &centroids[l])))
        .collect();
    labels = fill(&labels);

    // Number the speakers in the order of their first slot.
    let mut names: Vec<Option<usize>> = vec![None; count];
    let mut next = 0;
    let of_slot: Vec<Option<usize>> = labels
        .iter()
        .map(|l| {
            l.map(|l| {
                *names[l].get_or_insert_with(|| {
                    next += 1;
                    next - 1
                })
            })
        })
        .collect();
    Speakers {
        of_slot,
        typical,
        count: next,
    }
}

/// Slots without a speaker take the speaker of the nearest slot, the earlier one on a tie.
fn fill(labels: &[Option<usize>]) -> Vec<Option<usize>> {
    (0..labels.len())
        .map(|i| {
            labels[i].or_else(|| {
                let before = (0..i).rev().find(|&j| labels[j].is_some());
                let after = (i + 1..labels.len()).find(|&j| labels[j].is_some());
                match (before, after) {
                    (Some(b), Some(a)) if a - i < i - b => labels[a],
                    (Some(b), _) => labels[b],
                    (None, Some(a)) => labels[a],
                    (None, None) => None,
                }
            })
        })
        .collect()
}

fn normalised(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

/// Average-linkage clustering with the nearest-neighbour chain. It merges only groups
/// closer than `threshold`. Returns the group index of each vector.
fn cluster(vectors: &[Vec<f32>], threshold: f32) -> Vec<usize> {
    let n = vectors.len();
    let mut dist = vec![0.0f32; n * n];
    for i in 0..n {
        for j in i + 1..n {
            let cos: f32 = vectors[i].iter().zip(&vectors[j]).map(|(a, b)| a * b).sum();
            dist[i * n + j] = 1.0 - cos;
            dist[j * n + i] = 1.0 - cos;
        }
    }
    let mut size = vec![1usize; n];
    let mut active = vec![true; n];
    // A finished group is at least `threshold` from all others. Average linkage never
    // brings two groups closer than their parts, so it stays finished.
    let mut finished = vec![false; n];
    let mut parent: Vec<usize> = (0..n).collect();
    let mut chain: Vec<usize> = Vec::new();
    loop {
        if chain.is_empty() {
            match (0..n).find(|&i| active[i] && !finished[i]) {
                Some(i) => chain.push(i),
                None => break,
            }
        }
        let a = chain[chain.len() - 1];
        let previous = chain.len().checked_sub(2).map(|k| chain[k]);
        // The nearest other group; the previous chain link wins a tie.
        let mut best: Option<(f32, usize)> = previous.map(|p| (dist[a * n + p], p));
        for k in 0..n {
            if k != a && active[k] && !finished[k] && best.is_none_or(|(d, _)| dist[a * n + k] < d)
            {
                best = Some((dist[a * n + k], k));
            }
        }
        match best {
            Some((d, b)) if d < threshold => {
                if Some(b) == previous {
                    chain.truncate(chain.len() - 2);
                    // Merge b into a (Lance-Williams update for average linkage).
                    let (sa, sb) = (size[a] as f32, size[b] as f32);
                    for k in 0..n {
                        if active[k] && k != a && k != b {
                            let v = (sa * dist[a * n + k] + sb * dist[b * n + k]) / (sa + sb);
                            dist[a * n + k] = v;
                            dist[k * n + a] = v;
                        }
                    }
                    size[a] += size[b];
                    active[b] = false;
                    parent[b] = a;
                } else {
                    chain.push(b);
                }
            }
            _ => {
                finished[a] = true;
                chain.pop();
            }
        }
    }
    (0..n)
        .map(|mut i| {
            while parent[i] != i {
                i = parent[i];
            }
            i
        })
        .collect()
}

/// A voice sample of one speaker from its `lines` (`(start_ms, end_ms, typical)`), as
/// 24 kHz mono. The most typical lines come first, up to `max_ms`. Lines shorter than
/// 1 s count only when the longer lines give less than 6 s.
///
/// # Errors
/// Returns an error if the audio cannot be read.
pub fn sample(lines: &[(u64, u64, f32)], speech: &RawAudio, max_ms: u64) -> Result<Vec<f32>> {
    let mut order: Vec<&(u64, u64, f32)> = lines.iter().collect();
    order.sort_by(|a, b| b.2.total_cmp(&a.2));
    let long_ms: u64 = order
        .iter()
        .filter(|l| l.1 - l.0 >= MIN_SAMPLE_LINE_MS)
        .map(|l| l.1 - l.0)
        .sum();
    if long_ms >= MIN_SAMPLE_MS {
        order.retain(|l| l.1 - l.0 >= MIN_SAMPLE_LINE_MS);
    }
    let max = usize::try_from(max_ms * u64::from(speech.rate) / 1000).unwrap_or(usize::MAX);
    let mut out = Vec::with_capacity(max);
    for &&(start, end, _) in &order {
        let audio = mix::read_mono(speech, start, end)?;
        let room = max - out.len();
        out.extend_from_slice(&audio[..audio.len().min(room)]);
        if out.len() >= max {
            break;
        }
    }
    Ok(resample::resample(
        &out,
        speech.rate,
        crate::xtts::SAMPLE_RATE,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector along axis `a`, turned a little towards axis `b`.
    fn vector(a: usize, b: usize, amount: f32) -> Vec<f32> {
        let mut v = vec![0.0; DIM];
        v[a] = 1.0;
        v[b] = amount;
        normalised(v)
    }

    #[test]
    fn windows_cover_a_line() {
        assert_eq!(windows_of(900), [(0, 900)]);
        assert_eq!(windows_of(1500), [(0, 1500)]);
        assert_eq!(windows_of(3000), [(0, 1500), (750, 2250), (1500, 3000)]);
        assert_eq!(windows_of(2700), [(0, 1500), (750, 2250), (1200, 2700)]);
        assert_eq!(windows_of(2400), [(0, 1500), (750, 2250)]);
    }

    #[test]
    fn groups_two_speakers() {
        let v = [
            vector(0, 5, 0.1),
            vector(1, 6, 0.1),
            vector(0, 7, 0.2),
            vector(1, 8, 0.15),
        ];
        let l = assign(&v, &[9000; 4], THRESHOLD);
        assert!(l[0] == l[2] && l[1] == l[3] && l[0] != l[1], "{l:?}");
    }

    #[test]
    fn a_small_group_joins_the_nearest_actor() {
        // Speaker 0 has 16 s, speaker 1 has 16 s, and a third voice only 1.5 s.
        let v = [
            vector(0, 5, 0.1),
            vector(0, 6, 0.1),
            vector(1, 7, 0.1),
            vector(1, 8, 0.1),
            vector(2, 1, 0.9),
        ];
        let ms = [8000, 8000, 8000, 8000, 1500];
        let l = assign(&v, &ms, THRESHOLD);
        assert!(l[0] == l[1] && l[2] == l[3] && l[0] != l[2], "{l:?}");
        assert_eq!(l[4], l[2]);
    }

    #[test]
    fn without_a_large_group_the_largest_is_the_actor() {
        let v = [vector(0, 5, 0.1), vector(1, 6, 0.1), vector(1, 7, 0.1)];
        assert_eq!(assign(&v, &[1000; 3], THRESHOLD), [0, 0, 0]);
    }

    #[test]
    fn slots_vote_and_short_slots_take_a_neighbour() {
        // Slot 0: two windows of speaker 1 and one of speaker 0. Slot 1 has no window.
        let windows = [(0, 1500), (0, 1500), (0, 1000), (2, 1500)];
        let vectors = [
            vector(1, 5, 0.1),
            vector(1, 6, 0.1),
            vector(0, 7, 0.1),
            vector(0, 8, 0.1),
        ];
        let speakers = by_slot(3, &windows, &vectors, &[1, 1, 0, 0]);
        assert_eq!(speakers.of_slot, [Some(0), Some(0), Some(1)]);
        assert_eq!(speakers.count, 2);
        assert_eq!(speakers.typical[1], 0.0);
        assert!(speakers.typical[0] > 0.8);
        assert_eq!(by_slot(2, &[], &[], &[]).of_slot, [None, None]);
    }

    #[test]
    fn a_sample_starts_with_the_most_typical_long_lines() {
        // 1 kHz stereo; each line has its own constant value.
        let path = std::env::temp_dir().join("verbacue-speakers-typical.f32");
        let mut bytes = Vec::new();
        for (value, ms) in [(0.1f32, 4000), (0.2, 4000), (0.3, 500), (0.4, 4000)] {
            bytes.extend(std::iter::repeat_n(value.to_le_bytes(), 2 * ms).flatten());
        }
        std::fs::write(&path, bytes).unwrap();
        let speech = RawAudio {
            path,
            rate: 1000,
            channels: 2,
        };
        let lines = [
            (0, 4000, 0.5),
            (4000, 8000, 0.9),
            (8000, 8500, 0.99),
            (8500, 12_500, 0.7),
        ];
        let audio = sample(&lines, &speech, 9000).unwrap();
        // 9 s at 24 kHz: the 0.9 line, the 0.7 line, then 1 s of the 0.5 line. The
        // short 0.99 line is left out.
        assert_eq!(audio.len(), 9 * 24_000);
        assert!((audio[24_000] - 0.2).abs() < 0.01);
        assert!((audio[6 * 24_000] - 0.4).abs() < 0.01);
        assert!((audio[8 * 24_000 + 12_000] - 0.1).abs() < 0.01);
    }

    /// Speech from the `say` command at 24 kHz, or `None` without it.
    fn say(voice: &str, text: &str) -> Option<Vec<f32>> {
        let path = std::env::temp_dir().join(format!("verbacue-speakers-{voice}.wav"));
        let ok = std::process::Command::new("say")
            .args(["-v", voice, "--data-format=LEI16@24000", "-o"])
            .arg(&path)
            .arg(text)
            .status()
            .is_ok_and(|s| s.success());
        ok.then(|| crate::wav::read(&path).unwrap().0)
    }

    #[test]
    #[ignore = "needs tools/xtts-export/out from the export scripts, with campplus.onnx"]
    fn clones_two_voices() {
        let lines = [
            ("Samantha", "I waited for you at the station for two hours."),
            (
                "Daniel",
                "I am sorry, the train was late again this morning.",
            ),
            (
                "Samantha",
                "You could have called me. I was worried about you.",
            ),
            (
                "Daniel",
                "My phone was dead, and I had no money for a taxi.",
            ),
            (
                "Samantha",
                "Fine. Let us go home now, it is getting cold here.",
            ),
            (
                "Daniel",
                "Yes, and tomorrow I will buy a new phone, I promise.",
            ),
        ];
        // The lines one after the other, with 300 ms of silence, as 44.1 kHz stereo.
        let mut audio = Vec::new();
        let mut slots = Vec::new();
        for (voice, text) in lines {
            let Some(speech) = say(voice, text) else {
                eprintln!("skipped: needs the say command");
                return;
            };
            let start = audio.len() as u64 * 1000 / u64::from(crate::xtts::SAMPLE_RATE);
            audio.extend(speech);
            let end = audio.len() as u64 * 1000 / u64::from(crate::xtts::SAMPLE_RATE);
            slots.push(Slot {
                cue: slots.len(),
                start_ms: start,
                end_ms: end,
                next_start_ms: None,
            });
            audio.extend(std::iter::repeat_n(
                0.0,
                crate::xtts::SAMPLE_RATE as usize * 3 / 10,
            ));
        }
        let stereo: Vec<u8> = resample::resample(&audio, crate::xtts::SAMPLE_RATE, 44_100)
            .iter()
            .flat_map(|v| [v.to_le_bytes(), v.to_le_bytes()])
            .flatten()
            .collect();
        let path = std::env::temp_dir().join("verbacue-speakers-two.f32");
        std::fs::write(&path, stereo).unwrap();
        let speech = RawAudio {
            path,
            rate: 44_100,
            channels: 2,
        };

        let out =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/xtts-export/out");
        let mut model = SpeakerModel::open(&out.join(MODEL_FILE)).unwrap();
        let speakers = find(&mut model, &slots, &speech, &()).unwrap();
        assert_eq!(
            speakers.of_slot,
            [Some(0), Some(1), Some(0), Some(1), Some(0), Some(1)]
        );

        // Each sample clones a voice.
        let mine: Vec<(u64, u64, f32)> = slots
            .iter()
            .zip(&speakers.typical)
            .step_by(2)
            .map(|(s, &t)| (s.start_ms, s.end_ms, t))
            .collect();
        let clip = sample(&mine, &speech, SAMPLE_MS).unwrap();
        let mut xtts = crate::xtts::Xtts::load(&out, true).unwrap();
        assert!(xtts.clone_voice(&clip).is_ok());
    }
}
