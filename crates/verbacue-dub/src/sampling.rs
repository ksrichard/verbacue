//! Picks the next audio code from the GPT logits, as the Hugging Face `generate` of
//! coqui does: repetition penalty, then temperature, top-k and top-p.

/// A small seeded random generator (xorshift64*). The same seed gives the same speech.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // xorshift must not start at 0.
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    /// A number in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        #[expect(clippy::cast_precision_loss, reason = "53 bits")]
        let v = (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
        v
    }
}

/// The sampling settings. [`Default`] gives the XTTS-v2 values.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub temperature: f32,
    pub top_k: usize,
    pub top_p: f32,
    pub repetition_penalty: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            temperature: 0.75,
            top_k: 50,
            top_p: 0.85,
            repetition_penalty: 10.0,
        }
    }
}

/// Picks a code from `logits`. `seen` are the codes that get the repetition penalty.
pub fn sample(logits: &mut [f32], seen: &[usize], s: Settings, rng: &mut Rng) -> usize {
    for &t in seen {
        if let Some(l) = logits.get_mut(t) {
            *l = if *l < 0.0 {
                *l * s.repetition_penalty
            } else {
                *l / s.repetition_penalty
            };
        }
    }
    // Sorted from the most likely code down.
    let mut order: Vec<usize> = (0..logits.len()).collect();
    order.sort_by(|&a, &b| logits[b].total_cmp(&logits[a]));
    order.truncate(s.top_k.max(1));

    let max = logits[order[0]];
    let mut probs: Vec<f64> = order
        .iter()
        .map(|&i| f64::from((logits[i] - max) / s.temperature).exp())
        .collect();
    let total: f64 = probs.iter().sum();
    probs.iter_mut().for_each(|p| *p /= total);

    // Top-p as Hugging Face: drop the least likely codes while their sum stays at or
    // below 1 - top_p. The most likely code always stays.
    let mut dropped = 0.0;
    let mut keep = probs.len();
    while keep > 1 && dropped + probs[keep - 1] <= 1.0 - f64::from(s.top_p) {
        dropped += probs[keep - 1];
        keep -= 1;
    }
    let total: f64 = probs[..keep].iter().sum();
    let mut r = rng.next_f64() * total;
    for (i, p) in probs[..keep].iter().enumerate() {
        if r < *p {
            return order[i];
        }
        r -= p;
    }
    order[keep - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_same_numbers() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            let x = a.next_f64();
            assert!((0.0..1.0).contains(&x));
            assert_eq!(x.to_bits(), b.next_f64().to_bits());
        }
        assert_ne!(
            Rng::new(1).next_f64().to_bits(),
            Rng::new(2).next_f64().to_bits()
        );
    }

    #[test]
    fn top_p_keeps_only_the_likely_codes() {
        // Code 2 has almost all of the probability, so top-p leaves only code 2.
        let mut rng = Rng::new(1);
        for _ in 0..50 {
            let mut logits = [0.0, 1.0, 10.0, 2.0];
            assert_eq!(sample(&mut logits, &[], Settings::default(), &mut rng), 2);
        }
    }

    #[test]
    fn repetition_penalty_moves_away_from_seen_codes() {
        let mut rng = Rng::new(1);
        let mut logits = [5.0, 4.0, -1.0];
        let s = Settings {
            top_k: 1,
            ..Settings::default()
        };
        // 5 / 10 = 0.5 is below 4, so code 1 wins.
        assert_eq!(sample(&mut logits, &[0], s, &mut rng), 1);
        assert!((logits[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn top_k_never_picks_a_code_outside_the_k_best() {
        let mut rng = Rng::new(3);
        let s = Settings {
            top_k: 2,
            top_p: 1.0,
            temperature: 100.0,
            ..Settings::default()
        };
        for _ in 0..200 {
            let mut logits = [3.0, 2.9, 2.8, 2.7];
            assert!(sample(&mut logits, &[], s, &mut rng) < 2);
        }
    }
}
