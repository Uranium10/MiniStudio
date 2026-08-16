//! Stable processor instances shared by immutable realtime graphs.
//!
//! A structural project edit builds a replacement `AudioGraph` while the old
//! graph is still rendering. Reconstructing every VST3/CLAP instance during
//! that build is both slow and invalidates native editors. The registry keeps
//! one instance per stable project target and hands each graph a tiny proxy.
//! Only the audio callback dereferences the processor cell; graph swaps happen
//! between callbacks, so old and new proxies are never processed concurrently.

pub use ministudio_plugin::*;

use super::{
    dsp::{create_effect, AudioBuffer, DspEffect, PluginControl},
    instrument::{create_instrument, Instrument, NoteEvent},
    types::{EffectSpec, InstrumentSpec},
};
use std::{
    cell::UnsafeCell,
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

use ministudio_dsp::{RuntimeCapabilities, RuntimeTail};

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessorIdentity {
    format: String,
    path: String,
    uid: String,
    sample_rate_bits: u32,
}

impl ProcessorIdentity {
    fn effect(spec: &EffectSpec, sample_rate: f32) -> Self {
        let (format, path, uid) = spec.plugin.as_ref().map_or_else(
            || ("builtin".into(), String::new(), spec.kind.clone()),
            |plugin| {
                (
                    plugin.format.clone(),
                    plugin.path.clone(),
                    plugin.uid.clone(),
                )
            },
        );
        Self {
            format,
            path,
            uid,
            sample_rate_bits: sample_rate.to_bits(),
        }
    }

    fn instrument(spec: &InstrumentSpec, sample_rate: f32) -> Self {
        let (format, path, uid) = spec.plugin.as_ref().map_or_else(
            || ("builtin".into(), String::new(), spec.kind.clone()),
            |plugin| {
                (
                    plugin.format.clone(),
                    plugin.path.clone(),
                    plugin.uid.clone(),
                )
            },
        );
        Self {
            format,
            path,
            uid,
            sample_rate_bits: sample_rate.to_bits(),
        }
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

struct CapabilityCell {
    flags: AtomicU64,
    tail_samples: AtomicUsize,
}

impl CapabilityCell {
    fn new(value: RuntimeCapabilities) -> Self {
        let cell = Self {
            flags: AtomicU64::new(0),
            tail_samples: AtomicUsize::new(0),
        };
        cell.store(value);
        cell
    }

    #[inline]
    fn store(&self, value: RuntimeCapabilities) {
        let tail = match value.tail {
            RuntimeTail::None => 0,
            RuntimeTail::Finite(_) => 1,
            RuntimeTail::Infinite => 2,
            RuntimeTail::Unknown => 3,
        };
        let flags = tail
            | (u64::from(value.sleep_safe) << 2)
            | (u64::from(value.generator) << 3)
            | (u64::from(value.requires_continuous_time) << 4)
            | (u64::from(value.wakes_on_midi) << 5)
            | (u64::from(value.wakes_on_automation) << 6)
            | (u64::from(value.wakes_on_modulation) << 7)
            | (u64::from(value.wakes_on_transport) << 8)
            | (u64::from(value.wakes_on_sidechain) << 9);
        if let RuntimeTail::Finite(samples) = value.tail {
            self.tail_samples.store(samples, Ordering::Relaxed);
        }
        self.flags.store(flags, Ordering::Release);
    }

    #[inline]
    fn load(&self) -> RuntimeCapabilities {
        let flags = self.flags.load(Ordering::Acquire);
        let tail = match flags & 0b11 {
            0 => RuntimeTail::None,
            1 => RuntimeTail::Finite(self.tail_samples.load(Ordering::Relaxed)),
            2 => RuntimeTail::Infinite,
            _ => RuntimeTail::Unknown,
        };
        RuntimeCapabilities {
            tail,
            sleep_safe: flags & (1 << 2) != 0,
            generator: flags & (1 << 3) != 0,
            requires_continuous_time: flags & (1 << 4) != 0,
            wakes_on_midi: flags & (1 << 5) != 0,
            wakes_on_automation: flags & (1 << 6) != 0,
            wakes_on_modulation: flags & (1 << 7) != 0,
            wakes_on_transport: flags & (1 << 8) != 0,
            wakes_on_sidechain: flags & (1 << 9) != 0,
        }
    }
}

#[derive(Clone)]
struct SharedEffect {
    processor: Arc<RealtimeCell<Box<dyn DspEffect>>>,
    control: Option<Arc<dyn PluginControl>>,
    latency: usize,
    tail: usize,
    wants_midi: bool,
    capabilities: Arc<CapabilityCell>,
}

impl SharedEffect {
    fn new(effect: Box<dyn DspEffect>) -> Self {
        let control = effect.plugin_control();
        let latency = effect.latency_samples();
        let tail = effect.tail_samples();
        let wants_midi = effect.wants_midi();
        let capabilities = Arc::new(CapabilityCell::new(effect.runtime_capabilities()));
        Self {
            processor: Arc::new(RealtimeCell::new(effect)),
            control,
            latency,
            tail,
            wants_midi,
            capabilities,
        }
    }

    #[inline]
    fn processor(&mut self) -> &mut dyn DspEffect {
        // SAFETY: documented by `RealtimeCell`; all trait mutation is issued by
        // the single audio callback that owns the active graph.
        unsafe { self.processor.get_mut().as_mut() }
    }
}

impl DspEffect for SharedEffect {
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
        self.processor().set_param(id, value);
        let capabilities = self.processor().runtime_capabilities();
        self.capabilities.store(capabilities)
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.processor().set_tempo(bpm)
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

    fn runtime_capabilities(&self) -> RuntimeCapabilities {
        self.capabilities.load()
    }

    fn response(&self, points: usize) -> Option<ministudio_contracts::EqFrequencyResponse> {
        // SAFETY: graph metering/response reads happen on the same callback
        // owner as processing; the registry never dereferences this cell.
        unsafe { self.processor.get_ref().response(points) }
    }

    fn multiband_levels(&self) -> Option<[[f32; crate::audio::MAX_CHANNELS]; 3]> {
        unsafe { self.processor.get_ref().multiband_levels() }
    }

    fn effect_spectrum(&self) -> Option<[f32; ministudio_dsp::DISTORTION_SPECTRUM_BINS]> {
        unsafe { self.processor.get_ref().effect_spectrum() }
    }

    fn limiter_metrics(&self) -> Option<[f32; ministudio_dsp::LIMITER_METER_VALUES]> {
        unsafe { self.processor.get_ref().limiter_metrics() }
    }

    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        self.control.clone()
    }
}

#[derive(Clone)]
struct SharedInstrument {
    processor: Arc<RealtimeCell<Box<dyn Instrument>>>,
    control: Option<Arc<dyn PluginControl>>,
    tail: usize,
    capabilities: Arc<CapabilityCell>,
}

impl SharedInstrument {
    fn new(instrument: Box<dyn Instrument>) -> Self {
        let control = instrument.plugin_control();
        let tail = instrument.tail_samples();
        let capabilities = Arc::new(CapabilityCell::new(instrument.runtime_capabilities()));
        Self {
            processor: Arc::new(RealtimeCell::new(instrument)),
            control,
            tail,
            capabilities,
        }
    }

    #[inline]
    fn processor(&mut self) -> &mut dyn Instrument {
        // SAFETY: see `RealtimeCell`'s single-audio-thread invariant.
        unsafe { self.processor.get_mut().as_mut() }
    }
}

impl Instrument for SharedInstrument {
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize) {}

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize) {
        self.processor().process(events, out, frames)
    }

    fn set_param(&mut self, id: &str, value: f32) {
        self.processor().set_param(id, value);
        let capabilities = self.processor().runtime_capabilities();
        self.capabilities.store(capabilities)
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.processor().set_tempo(bpm)
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

    fn runtime_capabilities(&self) -> RuntimeCapabilities {
        self.capabilities.load()
    }

    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        self.control.clone()
    }
}

struct EffectEntry {
    identity: ProcessorIdentity,
    effect: SharedEffect,
}

struct InstrumentEntry {
    identity: ProcessorIdentity,
    instrument: SharedInstrument,
}

/// Control-thread registry of stable plug-in instances. It deliberately owns
/// no audio-thread locks and can be pruned immediately after a successful
/// graph build; the retired graph's `Arc` keeps removed instances alive until
/// the callback has completed its swap.
#[derive(Default)]
pub struct StableProcessorRegistry {
    effects: HashMap<String, EffectEntry>,
    instruments: HashMap<String, InstrumentEntry>,
}

impl StableProcessorRegistry {
    pub fn effect(
        &mut self,
        target_id: &str,
        spec: &EffectSpec,
        sample_rate: f32,
    ) -> Result<Box<dyn DspEffect>, String> {
        let identity = ProcessorIdentity::effect(spec, sample_rate);
        let needs_instance = self
            .effects
            .get(target_id)
            .is_none_or(|entry| entry.identity != identity);
        if needs_instance {
            let effect = create_effect(spec, sample_rate)
                .ok_or_else(|| format!("unsupported effect kind: {}", spec.kind))?;
            self.effects.insert(
                target_id.to_owned(),
                EffectEntry {
                    identity,
                    effect: SharedEffect::new(effect),
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
        let identity = ProcessorIdentity::instrument(spec, sample_rate);
        let needs_instance = self
            .instruments
            .get(target_id)
            .is_none_or(|entry| entry.identity != identity);
        if needs_instance {
            let instrument = create_instrument(spec, sample_rate)
                .ok_or_else(|| format!("unsupported instrument kind: {}", spec.kind))?;
            self.instruments.insert(
                target_id.to_owned(),
                InstrumentEntry {
                    identity,
                    instrument: SharedInstrument::new(instrument),
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
