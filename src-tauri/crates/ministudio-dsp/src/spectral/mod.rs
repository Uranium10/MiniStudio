//! Reusable, allocation-free spectral processing primitives.
//!
//! Effects own their musical policy. This module owns only transform planning,
//! streaming/WOLA mechanics, phase helpers, and reconstruction guarantees.

pub(crate) mod analysis;
mod hpcp;
mod peaks;
pub(crate) mod pitch_map;
pub(crate) mod stft;

#[inline(always)]
pub(crate) fn wrap_phase(phase: f32) -> f32 {
    (phase + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI
}
