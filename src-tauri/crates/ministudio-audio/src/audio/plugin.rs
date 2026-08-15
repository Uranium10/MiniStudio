//! Stable external plug-in instances shared by immutable realtime graphs.
//!
//! A structural project edit builds a replacement `AudioGraph` while the old
//! graph is still rendering. Reconstructing every VST3/CLAP instance during
//! that build is both slow and invalidates native editors. The registry keeps
//! one instance per stable project target and hands each graph a tiny proxy.
//! Only the audio callback dereferences the processor cell; graph swaps happen
//! between callbacks, so old and new proxies are never processed concurrently.

pub use ministudio_plugin::*;

use super::{
    dsp::{AudioBuffer, DspEffect, PluginControl},
    instrument::{Instrument, NoteEvent},
    types::{EffectSpec, InstrumentSpec},
};
use std::{
    cell::UnsafeCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PluginIdentity {
    format: String,
    path: String,
    uid: String,
    sample_rate_bits: u32,
}

impl PluginIdentity {
    fn effect(spec: &EffectSpec, sample_rate: f32) -> Result<Self, String> {
        let plugin = spec
            .plugin
            .as_ref()
            .ok_or("external effect is missing its plugin reference")?;
        Ok(Self {
            format: plugin.format.clone(),
            path: plugin.path.clone(),
            uid: plugin.uid.clone(),
            sample_rate_bits: sample_rate.to_bits(),
        })
    }

    fn instrument(spec: &InstrumentSpec, sample_rate: f32) -> Result<Self, String> {
        let plugin = spec
            .plugin
            .as_ref()
            .ok_or("external instrument is missing its plugin reference")?;
        Ok(Self {
            format: plugin.format.clone(),
            path: plugin.path.clone(),
            uid: plugin.uid.clone(),
            sample_rate_bits: sample_rate.to_bits(),
        })
    }
}

/// Single-writer storage for a native processor.
///
/// # Safety invariant
/// `AudioGraph` is consumed by exactly one audio callback. Replacement graphs
/// are swapped only at a callback boundary; a retired graph is never rendered
/// again. Registry code only clones/drops `Arc`s and never dereferences this
/// cell. Consequently every `get_mut`/`get_ref` below runs on that one audio
/// thread and cannot overlap another dereference, while vendor GUI/state calls
/// use the plug-in format's separate control handle.
struct RealtimeCell<T>(UnsafeCell<T>);

unsafe impl<T: Send> Send for RealtimeCell<T> {}
unsafe impl<T: Send> Sync for RealtimeCell<T> {}

impl<T> RealtimeCell<T> {
    fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    #[inline]
    unsafe fn get_mut(&self) -> &mut T {
        &mut *self.0.get()
    }

    #[inline]
    unsafe fn get_ref(&self) -> &T {
        &*self.0.get()
    }
}

#[derive(Clone)]
struct SharedExternalEffect {
    processor: Arc<RealtimeCell<Box<dyn DspEffect>>>,
    control: Option<Arc<dyn PluginControl>>,
    latency: usize,
    tail: usize,
    wants_midi: bool,
}

impl SharedExternalEffect {
    fn new(effect: Box<dyn DspEffect>) -> Self {
        let control = effect.plugin_control();
        let latency = effect.latency_samples();
        let tail = effect.tail_samples();
        let wants_midi = effect.wants_midi();
        Self {
            processor: Arc::new(RealtimeCell::new(effect)),
            control,
            latency,
            tail,
            wants_midi,
        }
    }

    #[inline]
    fn processor(&mut self) -> &mut dyn DspEffect {
        // SAFETY: documented by `RealtimeCell`; all trait mutation is issued by
        // the single audio callback that owns the active graph.
        unsafe { self.processor.get_mut().as_mut() }
    }
}

impl DspEffect for SharedExternalEffect {
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {
        // The native instance was prepared once when inserted into the
        // registry. A graph proxy must not reactivate it during every rebuild.
    }

    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        self.processor().process(events, buffer, frames)
    }

    fn process_with_sidechain(
        &mut self,
        events: &[NoteEvent],
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        self.processor()
            .process_with_sidechain(events, buffer, sidechain, frames)
    }

    fn set_param(&mut self, id: &str, value: f32) {
        self.processor().set_param(id, value)
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.processor().set_bypassed(bypassed)
    }

    fn reset(&mut self) {
        self.processor().reset()
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn wants_midi(&self) -> bool {
        self.wants_midi
    }

    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        self.control.clone()
    }
}

#[derive(Clone)]
struct SharedExternalInstrument {
    processor: Arc<RealtimeCell<Box<dyn Instrument>>>,
    control: Option<Arc<dyn PluginControl>>,
    tail: usize,
}

impl SharedExternalInstrument {
    fn new(instrument: Box<dyn Instrument>) -> Self {
        let control = instrument.plugin_control();
        let tail = instrument.tail_samples();
        Self {
            processor: Arc::new(RealtimeCell::new(instrument)),
            control,
            tail,
        }
    }

    #[inline]
    fn processor(&mut self) -> &mut dyn Instrument {
        // SAFETY: see `RealtimeCell`'s single-audio-thread invariant.
        unsafe { self.processor.get_mut().as_mut() }
    }
}

impl Instrument for SharedExternalInstrument {
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize) {}

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize) {
        self.processor().process(events, out, frames)
    }

    fn set_param(&mut self, id: &str, value: f32) {
        self.processor().set_param(id, value)
    }

    fn reset(&mut self) {
        self.processor().reset()
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    fn active_voice_count(&self) -> usize {
        // SAFETY: this query is made by the same callback that processes the
        // graph and never by registry/control code.
        unsafe { self.processor.get_ref().active_voice_count() }
    }

    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        self.control.clone()
    }
}

struct EffectEntry {
    identity: PluginIdentity,
    effect: SharedExternalEffect,
}

struct InstrumentEntry {
    identity: PluginIdentity,
    instrument: SharedExternalInstrument,
}

/// Control-thread registry of stable plug-in instances. It deliberately owns
/// no audio-thread locks and can be pruned immediately after a successful
/// graph build; the retired graph's `Arc` keeps removed instances alive until
/// the callback has completed its swap.
#[derive(Default)]
pub struct ExternalPluginRegistry {
    effects: HashMap<String, EffectEntry>,
    instruments: HashMap<String, InstrumentEntry>,
}

impl ExternalPluginRegistry {
    pub fn effect(
        &mut self,
        target_id: &str,
        spec: &EffectSpec,
        sample_rate: f32,
    ) -> Result<Box<dyn DspEffect>, String> {
        let identity = PluginIdentity::effect(spec, sample_rate)?;
        let needs_instance = self
            .effects
            .get(target_id)
            .is_none_or(|entry| entry.identity != identity);
        if needs_instance {
            let effect = try_create_external_effect(spec, sample_rate)?;
            self.effects.insert(
                target_id.to_owned(),
                EffectEntry {
                    identity,
                    effect: SharedExternalEffect::new(effect),
                },
            );
        }
        Ok(Box::new(
            self.effects
                .get(target_id)
                .expect("effect inserted above")
                .effect
                .clone(),
        ))
    }

    pub fn instrument(
        &mut self,
        target_id: &str,
        spec: &InstrumentSpec,
        sample_rate: f32,
    ) -> Result<Option<Box<dyn Instrument>>, String> {
        if spec.bypassed {
            return Ok(None);
        }
        let identity = PluginIdentity::instrument(spec, sample_rate)?;
        let needs_instance = self
            .instruments
            .get(target_id)
            .is_none_or(|entry| entry.identity != identity);
        if needs_instance {
            let instrument = try_create_external_instrument(spec, sample_rate)?;
            self.instruments.insert(
                target_id.to_owned(),
                InstrumentEntry {
                    identity,
                    instrument: SharedExternalInstrument::new(instrument),
                },
            );
        }
        Ok(Some(Box::new(
            self.instruments
                .get(target_id)
                .expect("instrument inserted above")
                .instrument
                .clone(),
        )))
    }

    pub fn retain(&mut self, effect_ids: &HashSet<String>, instrument_ids: &HashSet<String>) {
        self.effects.retain(|id, _| effect_ids.contains(id));
        self.instruments.retain(|id, _| instrument_ids.contains(id));
    }

    pub fn clear(&mut self) {
        self.effects.clear();
        self.instruments.clear();
    }
}
