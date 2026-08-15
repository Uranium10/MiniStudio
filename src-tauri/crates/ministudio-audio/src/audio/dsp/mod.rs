//! App-level effect factory. Native DSP lives in the Tauri-free DSP crate.

use super::types::EffectSpec;

pub use ministudio_dsp::*;

pub fn create_effect(spec: &EffectSpec, sample_rate: f32) -> Option<Box<dyn DspEffect>> {
    if spec.kind.starts_with("vst3:") || spec.kind.starts_with("clap:") {
        return super::plugin::create_external_effect(spec, sample_rate);
    }
    create_builtin_effect(&spec.kind, &spec.params, spec.bypassed, sample_rate)
}
