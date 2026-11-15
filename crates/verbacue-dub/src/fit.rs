//! Fits each spoken line into the time of its subtitle cue.
//!
//! A line plays from the start of its cue, or from where the actor starts to speak in
//! the cue. When the speech is longer than the cue, the line speeds up to fit the cue.
//! If that is too fast, it also uses the silence before the next cue. When the speech
//! is shorter than the actor's speech, the line slows down a little, so that it ends
//! with the actor. A line never plays over the next line.

use verbacue_core::subtitle::Cue;

/// The shortest slot for a cue, so that a very short cue still gets some time.
const MIN_SLOT_MS: u64 = 200;
/// The silence that stays before the next line when a line uses the gap.
const GAP_MS: u64 = 100;
/// The highest speed. Above it, the mixer cuts the line at the next line.
pub const MAX_FACTOR: f32 = 1.5;
/// The lowest speed: a line slows down by at most 15 % to end with the actor.
pub const MIN_FACTOR: f32 = 0.85;
/// The frames of the actor's speech, and their level against the loudest frame.
const FRAME_MS: u64 = 20;
const SPEECH_DB: f32 = -30.0;

/// The time of one cue for its speech.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The index of the cue in the input of [`slots`].
    pub cue: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    /// The start of the next slot, or `None` for the last one.
    pub next_start_ms: Option<u64>,
}

/// The slots of the cues, sorted by start. Each slot is at least 200 ms long, but
/// never goes past the start of the next slot. Cues with the same start get starts
/// 1 ms apart, so that no two slots overlap.
pub fn slots(cues: &[Cue]) -> Vec<Slot> {
    let mut order: Vec<usize> = (0..cues.len()).collect();
    order.sort_by_key(|&i| (cues[i].start_ms, i));
    let mut starts: Vec<u64> = Vec::with_capacity(order.len());
    for &i in &order {
        let start = match starts.last() {
            Some(&prev) => cues[i].start_ms.max(prev + 1),
            None => cues[i].start_ms,
        };
        starts.push(start);
    }
    order
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            let start_ms = starts[n];
            let next_start_ms = starts.get(n + 1).copied();
            let end_ms = cues[i].end_ms.max(start_ms + MIN_SLOT_MS);
            let end_ms = next_start_ms.map_or(end_ms, |next| end_ms.min(next));
            Slot {
                cue: i,
                start_ms,
                end_ms,
                next_start_ms,
            }
        })
        .collect()
}

/// The speed of one line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// 1.0 is the natural speed, 1.3 is 30 % faster. The latents get
    /// `stretch(1.0 / factor)`.
    pub factor: f32,
    /// True if the speed is above the limit, or the line still does not fit.
    pub warn: bool,
}

/// The speed for speech of `natural_ms` in `slot`. `limit` is the highest speed
/// without a warning (`--dub-speed-limit`). `actor_ms` is how long the actor speaks,
/// if known: a shorter line slows down towards it.
pub fn fit(natural_ms: u64, slot: &Slot, limit: f32, actor_ms: Option<u64>) -> Fit {
    let limit = limit.clamp(1.0, MAX_FACTOR);
    let slot_ms = slot.end_ms - slot.start_ms;
    // The time up to shortly before the next line, but never less than the slot.
    let gap_ms = slot.next_start_ms.map_or(u64::MAX, |next| {
        next.saturating_sub(GAP_MS).max(slot.end_ms) - slot.start_ms
    });
    #[expect(clippy::cast_precision_loss, reason = "durations in ms")]
    let need = |room: u64| natural_ms as f32 / room.max(1) as f32;

    if natural_ms <= slot_ms {
        let target = actor_ms.unwrap_or(0).min(slot_ms);
        #[expect(clippy::cast_precision_loss, reason = "durations in ms")]
        let factor = if natural_ms > 0 && natural_ms < target {
            (natural_ms as f32 / target as f32).max(MIN_FACTOR)
        } else {
            1.0
        };
        return Fit {
            factor,
            warn: false,
        };
    }
    if need(slot_ms) <= limit {
        return Fit {
            factor: need(slot_ms),
            warn: false,
        };
    }
    let factor = need(gap_ms).max(1.0);
    if factor <= limit {
        return Fit {
            factor,
            warn: false,
        };
    }
    Fit {
        factor: factor.min(MAX_FACTOR),
        warn: true,
    }
}

/// Where the actor speaks in `audio` (clean speech of one cue, mono at `rate`), as
/// (start, end) in ms from the start of `audio`: the first and the last 20 ms frame
/// within 30 dB of the loudest one. `None` for silence.
pub fn speech_span(audio: &[f32], rate: u32) -> Option<(u64, u64)> {
    let frame = (rate as usize * FRAME_MS as usize / 1000).max(1);
    let levels: Vec<f32> = audio
        .chunks(frame)
        .map(|c| c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32)
        .collect();
    let peak = levels.iter().copied().fold(0.0f32, f32::max);
    if peak <= 1e-8 {
        return None;
    }
    let floor = peak * 10.0f32.powf(SPEECH_DB / 10.0);
    let first = levels.iter().position(|&l| l >= floor)?;
    let last = levels.iter().rposition(|&l| l >= floor)?;
    Some((first as u64 * FRAME_MS, (last as u64 + 1) * FRAME_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(len: u64, next: Option<u64>) -> Slot {
        Slot {
            cue: 0,
            start_ms: 10_000,
            end_ms: 10_000 + len,
            next_start_ms: next.map(|n| 10_000 + n),
        }
    }

    #[test]
    fn a_short_line_slows_down_towards_the_actor() {
        let f = fit(1800, &slot(3000, None), 1.3, Some(2000));
        assert!((f.factor - 0.9).abs() < 1e-3, "{f:?}");
        // At most 15 % slower.
        assert!((fit(1000, &slot(3000, None), 1.3, Some(2000)).factor - MIN_FACTOR).abs() < 1e-3);
        // Longer than the actor, but inside the cue: natural speed.
        assert!((fit(2500, &slot(3000, None), 1.3, Some(2000)).factor - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_speech_span_skips_the_silence() {
        // 1 kHz: 300 ms silence, 500 ms speech, 200 ms of very quiet noise.
        let mut audio = vec![0.0f32; 300];
        audio.extend(std::iter::repeat_n(0.5, 500));
        audio.extend(std::iter::repeat_n(0.001, 200));
        assert_eq!(speech_span(&audio, 1000), Some((300, 800)));
        assert_eq!(speech_span(&[0.0; 100], 1000), None);
    }

    #[test]
    fn fits_without_change() {
        assert_eq!(
            fit(1000, &slot(1500, None), 1.3, None),
            Fit {
                factor: 1.0,
                warn: false
            }
        );
    }

    #[test]
    fn speeds_up_to_limit() {
        let f = fit(1300, &slot(1000, Some(1000)), 1.3, None);
        assert!((f.factor - 1.3).abs() < 1e-6 && !f.warn, "{f:?}");
    }

    #[test]
    fn uses_the_gap() {
        assert_eq!(
            fit(2000, &slot(1000, Some(3000)), 1.3, None),
            Fit {
                factor: 1.0,
                warn: false
            }
        );
        // 2600 ms in the 2900 ms up to the next line, at natural speed.
        assert_eq!(
            fit(2600, &slot(1000, Some(3000)), 1.3, None),
            Fit {
                factor: 1.0,
                warn: false
            }
        );
        // 3480 ms needs 1.2 to fit the gap.
        let f = fit(3480, &slot(1000, Some(3000)), 1.3, None);
        assert!((f.factor - 1.2).abs() < 1e-6 && !f.warn, "{f:?}");
    }

    #[test]
    fn warns_above_limit() {
        assert_eq!(
            fit(2000, &slot(1000, Some(1100)), 1.3, None),
            Fit {
                factor: 1.5,
                warn: true
            }
        );
        // 1.4 is above the limit 1.3 and below 1.5.
        let f = fit(1400, &slot(1000, Some(1100)), 1.3, None);
        assert!((f.factor - 1.4).abs() < 1e-6 && f.warn, "{f:?}");
    }

    #[test]
    fn the_last_line_has_all_the_time_it_needs() {
        assert_eq!(
            fit(9000, &slot(1000, None), 1.3, None),
            Fit {
                factor: 1.0,
                warn: false
            }
        );
    }

    #[test]
    fn slots_never_overlap() {
        let cues = [
            Cue::new(5000, 4000, "end before start"),
            Cue::new(0, 3000, "overlaps the next"),
            Cue::new(2000, 2500, "inside the first"),
            Cue::new(2500, 2600, "touches the one before"),
            Cue::new(2600, 2600, "zero length"),
            Cue::new(2600, 2700, "same start"),
            Cue::new(9000, 9000, "last"),
        ];
        let slots = slots(&cues);
        assert_eq!(slots.len(), cues.len());
        for w in slots.windows(2) {
            assert!(w[0].end_ms <= w[1].start_ms, "{w:?}");
            assert_eq!(w[0].next_start_ms, Some(w[1].start_ms));
        }
        for s in &slots {
            assert!(s.end_ms > s.start_ms, "{s:?}");
        }
        assert_eq!(slots[0].cue, 1);
        assert_eq!(slots.last().unwrap().end_ms, 9200);
        let mut seen: Vec<usize> = slots.iter().map(|s| s.cue).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..cues.len()).collect::<Vec<_>>());
    }
}
