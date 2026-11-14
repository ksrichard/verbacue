//! Finds the speakers of a film and a voice sample for each one.
//!
//! Each line gets a speaker embedding from the XTTS-v2 speaker encoder. Lines with
//! similar embeddings form one speaker. The grouping is agglomerative clustering with
//! average linkage on the cosine distance. It stops when the two nearest groups are
//! [`THRESHOLD`] or more apart.

use verbacue_core::Result;
use verbacue_media::audio::RawAudio;

use crate::{
    fit::Slot,
    mix, resample,
    run::Events,
    xtts::{SAMPLE_RATE, SPEAKER_DIM, Xtts},
};

/// The cosine distance at which two groups are different speakers. Two halves of one
/// recording are about 0.27 apart; different speakers are 0.5 to 0.65 apart.
pub const THRESHOLD: f32 = 0.35;
/// Lines shorter than this give no reliable embedding.
pub const MIN_LINE_MS: u64 = 600;
/// The length of a voice sample.
pub const SAMPLE_MS: u64 = 10_000;

/// The speaker of each line, numbered from 0 in the order of their first line. A line
/// without an embedding gets the speaker of the nearest line, the earlier one on a tie.
/// All lines are `None` only if no line has an embedding.
pub fn group(embeddings: &[Option<[f32; SPEAKER_DIM]>], threshold: f32) -> Vec<Option<usize>> {
    let known: Vec<usize> = (0..embeddings.len())
        .filter(|&i| embeddings[i].is_some())
        .collect();
    let vectors: Vec<[f32; SPEAKER_DIM]> = known
        .iter()
        .filter_map(|&i| embeddings[i].map(normalised))
        .collect();
    let clusters = cluster(&vectors, threshold);

    // Number the groups in the order of their first line.
    let mut names: Vec<Option<usize>> = vec![None; vectors.len()];
    let mut next = 0;
    let mut labels: Vec<Option<usize>> = vec![None; embeddings.len()];
    for (k, &line) in known.iter().enumerate() {
        let c = clusters[k];
        let name = *names[c].get_or_insert_with(|| {
            next += 1;
            next - 1
        });
        labels[line] = Some(name);
    }
    // Lines without an embedding take the speaker of the nearest line.
    let filled: Vec<Option<usize>> = (0..labels.len())
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
        .collect();
    filled
}

fn normalised(v: [f32; SPEAKER_DIM]) -> [f32; SPEAKER_DIM] {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.map(|x| x / norm)
}

/// Average-linkage clustering with the nearest-neighbour chain. It merges only groups
/// closer than `threshold`. Returns the group index of each vector.
fn cluster(vectors: &[[f32; SPEAKER_DIM]], threshold: f32) -> Vec<usize> {
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

/// The speaker of each slot. Slots of [`MIN_LINE_MS`] or longer get an embedding from
/// `speech`.
///
/// # Errors
/// Returns an error if the audio cannot be read or the speaker encoder fails.
pub fn find(
    xtts: &mut Xtts,
    slots: &[Slot],
    speech: &RawAudio,
    events: &dyn Events,
) -> Result<Vec<Option<usize>>> {
    let mut embeddings = Vec::with_capacity(slots.len());
    for (n, slot) in slots.iter().enumerate() {
        if events.cancelled() {
            return Err(verbacue_core::Error::new(
                verbacue_core::ErrorKind::Cancelled,
                "cancelled",
            ));
        }
        events.progress(n as u64, slots.len() as u64);
        if slot.end_ms - slot.start_ms < MIN_LINE_MS {
            embeddings.push(None);
            continue;
        }
        let audio = mix::read_mono(speech, slot.start_ms, slot.end_ms)?;
        let audio = resample::resample(&audio, speech.rate, SAMPLE_RATE);
        embeddings.push(Some(xtts.speaker_embedding(&audio)?));
    }
    Ok(group(&embeddings, THRESHOLD))
}

/// A voice sample from `lines` (`(start_ms, end_ms)`): the loudest lines, up to
/// `max_ms` in total, as 24 kHz mono.
///
/// # Errors
/// Returns an error if the audio cannot be read.
pub fn sample(lines: &[(u64, u64)], speech: &RawAudio, max_ms: u64) -> Result<Vec<f32>> {
    let mut parts: Vec<(f32, Vec<f32>)> = Vec::with_capacity(lines.len());
    for &(start, end) in lines {
        let audio = mix::read_mono(speech, start, end)?;
        #[expect(clippy::cast_precision_loss, reason = "a sample count")]
        let rms = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32).sqrt();
        parts.push((rms, audio));
    }
    parts.sort_by(|a, b| b.0.total_cmp(&a.0));
    let max = usize::try_from(max_ms * u64::from(speech.rate) / 1000).unwrap_or(usize::MAX);
    let mut out = Vec::with_capacity(max);
    for (_, audio) in parts {
        let room = max - out.len();
        out.extend_from_slice(&audio[..audio.len().min(room)]);
        if out.len() >= max {
            break;
        }
    }
    Ok(resample::resample(&out, speech.rate, SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector along axis `a`, turned a little towards axis `b`.
    fn vector(a: usize, b: usize, amount: f32) -> Option<[f32; SPEAKER_DIM]> {
        let mut v = [0.0; SPEAKER_DIM];
        v[a] = 1.0;
        v[b] = amount;
        Some(v)
    }

    #[test]
    fn groups_two_speakers() {
        let lines = [
            vector(0, 5, 0.1),
            vector(1, 6, 0.1),
            vector(0, 7, 0.2),
            vector(1, 8, 0.15),
            vector(0, 9, 0.05),
        ];
        assert_eq!(
            group(&lines, THRESHOLD),
            [Some(0), Some(1), Some(0), Some(1), Some(0)]
        );
    }

    #[test]
    fn one_speaker_stays_one_group() {
        let lines: Vec<_> = (0..20).map(|i| vector(0, 1 + i, 0.2)).collect();
        assert!(group(&lines, THRESHOLD).iter().all(|&g| g == Some(0)));
    }

    #[test]
    fn short_lines_join_neighbour() {
        let lines = [
            vector(0, 5, 0.1),
            None,
            vector(0, 6, 0.1),
            vector(1, 7, 0.1),
            None,
        ];
        assert_eq!(
            group(&lines, THRESHOLD),
            [Some(0), Some(0), Some(0), Some(1), Some(1)]
        );
        assert_eq!(group(&[None, None], THRESHOLD), [None, None]);
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
    #[ignore = "needs tools/xtts-export/out from the export scripts"]
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
            let start = audio.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
            audio.extend(speech);
            let end = audio.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
            slots.push(Slot {
                cue: slots.len(),
                start_ms: start,
                end_ms: end,
                next_start_ms: None,
            });
            audio.extend(std::iter::repeat_n(0.0, SAMPLE_RATE as usize * 3 / 10));
        }
        let stereo: Vec<u8> = resample::resample(&audio, SAMPLE_RATE, 44_100)
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
        let mut xtts = Xtts::load(&out, true).unwrap();
        let groups = find(&mut xtts, &slots, &speech, &()).unwrap();
        assert_eq!(
            groups,
            [Some(0), Some(1), Some(0), Some(1), Some(0), Some(1)]
        );

        // Each sample clones a voice near its speaker.
        let mine: Vec<(u64, u64)> = slots
            .iter()
            .step_by(2)
            .map(|s| (s.start_ms, s.end_ms))
            .collect();
        let clip = sample(&mine, &speech, SAMPLE_MS).unwrap();
        assert!(xtts.clone_voice(&clip).is_ok());
    }

    #[test]
    fn sample_caps_length() {
        // 20 s of a constant, at 1000 Hz stereo, with 4 lines of 5 s.
        let path = std::env::temp_dir().join("verbacue-speakers-sample.f32");
        let bytes: Vec<u8> = std::iter::repeat_n(0.25f32.to_le_bytes(), 2 * 20_000)
            .flatten()
            .collect();
        std::fs::write(&path, bytes).unwrap();
        let speech = RawAudio {
            path,
            rate: 1000,
            channels: 2,
        };
        let lines = [
            (0, 5000),
            (5000, 10_000),
            (10_000, 15_000),
            (15_000, 20_000),
        ];
        let audio = sample(&lines, &speech, SAMPLE_MS).unwrap();
        assert_eq!(audio.len(), 10 * SAMPLE_RATE as usize);
    }
}
