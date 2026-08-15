//! Allocation-free native DSP shared by realtime and offline renderers.

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::{collections::HashMap, f32::consts::PI, sync::Arc};

pub use ministudio_contracts::EqFrequencyResponse;

pub const MAX_CHANNELS: usize = 2;
pub const MAX_BLOCK_SIZE: usize = 2048;
pub const DISTORTION_SPECTRUM_BINS: usize = 48;
pub const LIMITER_METER_VALUES: usize = 7;

/// Main-thread/control-plane access to an external plug-in instance.
///
/// The realtime graph only keeps an `Arc` to this interface. Implementations
/// must never make the audio callback wait for editor or state operations.
pub trait PluginControl: Send + Sync {
    fn has_editor(&self) -> bool;
    fn open_editor(&self) -> Result<(), String>;
    fn close_editor(&self) -> Result<(), String>;
    fn is_editor_open(&self) -> bool;
    fn save_state(&self) -> Result<Vec<u8>, String>;
    fn load_state(&self, state: Vec<u8>) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug)]
pub struct NoteEvent {
    pub sample_offset: u32,
    pub kind: NoteEventKind,
}

#[derive(Clone, Copy, Debug)]
pub enum NoteEventKind {
    NoteOn {
        note_id: i32,
        pitch: u8,
        velocity: f32,
        tuning_cents: f32,
    },
    NoteOff {
        note_id: i32,
        pitch: u8,
        velocity: f32,
    },
    PolyPressure {
        note_id: i32,
        pitch: u8,
        pressure: f32,
    },
    Controller {
        cc: u8,
        value: f32,
    },
    PitchBend {
        value: f32,
    },
    AllNotesOff,
}

pub trait Instrument: Send {
    fn prepare(&mut self, sample_rate: f32, max_block: usize);
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize);
    fn set_param(&mut self, id: &str, value: f32);
    fn reset(&mut self);
    fn tail_samples(&self) -> usize;
    fn active_voice_count(&self) -> usize;
    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        None
    }
}

#[inline(always)]
fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

include!("common/core.rs");
include!("common/biquad.rs");
include!("common/helpers.rs");
include!("effects/mod.rs");

pub fn create_builtin_effect(
    kind: &str,
    params: &HashMap<String, f32>,
    bypassed: bool,
    sample_rate: f32,
) -> Option<Box<dyn DspEffect>> {
    let mut effect: Box<dyn DspEffect> = match kind {
        "builtin:eq" => Box::new(ParametricEq::new()),
        "builtin:eq8" => Box::new(ParametricEq::new_eight()),
        "builtin:compressor" => Box::new(Compressor::new()),
        "builtin:multiband-compressor" => Box::new(MultibandCompressor::new()),
        "builtin:utility" => Box::new(Utility::new()),
        "builtin:delay" => Box::new(Delay::new()),
        "builtin:reverb" => Box::new(Reverb::new()),
        "builtin:waveshaper" => Box::new(Waveshaper::new()),
        "builtin:distortion" => Box::new(Distortion::new()),
        "builtin:disperser" => Box::new(Disperser::new()),
        "builtin:mastering-limiter" => Box::new(MasteringLimiter::new()),
        "builtin:vocoder" => Box::new(Vocoder::new()),
        "builtin:lfo-tremolo" => Box::new(LfoTremolo::new()),
        "builtin:clipper" => Box::new(Clipper::new()),
        "builtin:upward-compressor" => Box::new(UpwardCompressor::new()),
        "builtin:roboter" => Box::new(Roboter::new()),
        "builtin:resonator" => Box::new(Colorizer::new()),
        _ => return None,
    };
    effect.prepare(sample_rate, MAX_BLOCK_SIZE, MAX_CHANNELS);
    for (id, value) in params {
        effect.set_param(id, *value);
    }
    effect.set_bypassed(bypassed);
    Some(effect)
}

#[cfg(test)]
mod tests;
