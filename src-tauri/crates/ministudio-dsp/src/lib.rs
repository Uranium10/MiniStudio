//! Allocation-free native DSP shared by realtime and offline renderers.

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::{collections::HashMap, f32::consts::PI, sync::Arc};

pub use ministudio_contracts::EqFrequencyResponse;

pub const MAX_CHANNELS: usize = 2;
pub const MAX_BLOCK_SIZE: usize = 2048;
pub const DISTORTION_SPECTRUM_BINS: usize = 48;
pub const LIMITER_METER_VALUES: usize = 7;

/// A host-side handle for a native plug-in view. The actual thread-affine GUI
/// object may live on a dedicated message-pumped UI worker; this Send handle
/// only forwards lifecycle and coalesced resize requests to that owner.
pub trait EmbeddedPluginEditor: Send {
    fn set_rect(&mut self, x: f32, y: f32, width: f32, height: f32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEditorAction {
    Closed,
    TogglePower,
    TogglePin,
    ToggleBypass,
    SavePreset,
    LoadPreset,
    ShowSidechain,
    AutomationOff,
    AutomationWrite,
    AutomationRead,
    AutomationLatch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginEditorState {
    pub bypassed: bool,
    pub automation: u8,
}

/// Main-thread/control-plane access to an external plug-in instance.
///
/// The realtime graph only keeps an `Arc` to this interface. Implementations
/// must never make the audio callback wait for editor or state operations.
pub trait PluginControl: Send + Sync {
    fn has_editor(&self) -> bool;
    fn open_editor(&self) -> Result<(), String>;
    /// Opens the native editor inside a host-owned top-level window. The
    /// coordinates are logical pixels so GUI work remains independent of the
    /// audio callback and of monitor DPI.
    fn open_editor_embedded(
        &self,
        _parent: usize,
        _x: f32,
        _y: f32,
        _width: f32,
        _height: f32,
    ) -> Result<Box<dyn EmbeddedPluginEditor>, String> {
        Err("embedded plug-in editors are not supported by this host".into())
    }
    fn close_editor(&self) -> Result<(), String>;
    fn is_editor_open(&self) -> bool;
    fn save_state(&self) -> Result<Vec<u8>, String>;
    fn load_state(&self, state: Vec<u8>) -> Result<(), String>;
    /// Drain human-rate actions emitted by host-owned native editor controls.
    fn take_editor_actions(&self) -> Vec<PluginEditorAction> {
        Vec::new()
    }
    fn set_editor_state(&self, _state: PluginEditorState) {}
    /// Hints that this instance's track has been silent for a while (`true`) or has started
    /// producing audio again (`false`). This is only a future graph-suspension seam: a host
    /// must not trim or page out a helper that is still servicing realtime deadlines. Callers
    /// never depend on the hint taking effect, and current implementations leave it as a no-op.
    fn set_idle(&self, _idle: bool) {}
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
    /// Musical tempo at the start of the current processing segment.
    fn set_tempo(&mut self, _bpm: f64) {}
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
        "builtin:transient-shaper" => Box::new(TransientShaper::new()),
        "builtin:roboter" => Box::new(Roboter::new()),
        "builtin:resonator" => Box::new(Colorizer::new()),
        "builtin:formant-shifter" => Box::new(FormantShifter::new()),
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
