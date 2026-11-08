//! Canvas drawings that move: the grill, the skewer and the confetti.

pub mod confetti;
pub mod grill;
pub mod skewer;

use std::f32::consts::TAU;

/// A smooth 0..1..0 wave with period `period` seconds, shifted by `delay` seconds.
pub fn wave(t: f32, period: f32, delay: f32) -> f32 {
    0.5 - 0.5 * (TAU * (t - delay) / period).cos()
}

/// The 0..1 position in a repeating cycle of `period` seconds.
pub fn cycle(t: f32, period: f32, delay: f32) -> f32 {
    ((t - delay) / period).rem_euclid(1.0)
}
