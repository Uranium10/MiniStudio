// Sample-accurate clip, routing, effect-chain, metering, and PDC graph.
use super::{
    asset::AudioAsset,
    command::{ChainKind, EffectRef},
    dsp::{
        clipper_curve, AudioBuffer, DspEffect, PluginControl, Smoother, DISTORTION_SPECTRUM_BINS,
        LIMITER_METER_VALUES,
    },
    instrument::{Instrument, NoteEvent, NoteEventKind, TempoMap},
    plugin::StableProcessorRegistry,
    runtime::{
        BlockActivity, NodeRuntime, ProcessDecision, SchedulerSnapshot, SleepPolicy, WakeReason,
    },
    types::{db_to_gain, sec_to_samples, GraphSnapshot, Level},
    MAX_BLOCK_SIZE, MAX_CHANNELS, MAX_EFFECT_METERS, MAX_TRACKS,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Master-output safety knee: nothing below -0.18 dBFS is touched, so normal mixing is
/// untouched, but a plugin note that spikes past 0 dBFS (resonant filters, unison stacks,
/// stray gain-stage overs from a hot instrument patch) gets bent toward the ceiling instead
/// of being handed to the device's raw float->int conversion, which has no clipping
/// protection of its own and turns an over into harsh, "tearing" digital distortion.
const MASTER_SAFETY_KNEE: f32 = 0.02;

fn effect_has_spectrum(kind: &str) -> bool {
    matches!(
        kind,
        "builtin:eq"
            | "builtin:eq8"
            | "builtin:distortion"
            | "builtin:disperser"
            | "builtin:clipper"
            | "builtin:roboter"
            | "builtin:compressor"
            | "builtin:upward-compressor"
    )
}

fn effect_has_limiter_metrics(kind: &str) -> bool {
    kind == "builtin:mastering-limiter"
}

fn resolve_sidechain_source(spec: &GraphSnapshot, source: &str) -> Option<SidechainSource> {
    if let Some(id) = source.strip_prefix("bus:") {
        return spec
            .buses
            .iter()
            .position(|bus| bus.id == id)
            .map(SidechainSource::Bus);
    }
    let id = source.strip_prefix("track:").unwrap_or(source);
    spec.tracks
        .iter()
        .position(|track| track.id == id)
        .map(SidechainSource::Track)
}

fn sidechain_from_taps<'a>(
    source: SidechainSource,
    tracks: &'a [AudioBuffer],
    buses: &'a [AudioBuffer],
) -> Option<&'a AudioBuffer> {
    match source {
        SidechainSource::Track(index) => tracks.get(index),
        SidechainSource::Bus(index) => buses.get(index),
    }
}

pub struct Bindings {
    pub tracks: HashMap<String, usize>,
    pub buses: HashMap<String, usize>,
    pub sends: HashMap<String, (usize, usize)>,
    pub effects: HashMap<String, EffectRef>,
    pub plugin_controls: HashMap<String, Arc<dyn PluginControl>>,
    pub track_ids: Vec<String>,
    pub multiband_meter_ids: Vec<String>,
    pub distortion_meter_ids: Vec<String>,
    pub limiter_meter_ids: Vec<String>,
}
struct ClipEvent {
    asset: Arc<AudioAsset>,
    start: u64,
    end: u64,
    offset: u64,
    gain: f32,
    fade_in: u64,
    fade_out: u64,
    source_step: f64,
    reversed: bool,
    fade_in_curve: f32,
    fade_out_curve: f32,
    gain_segments: Vec<ClipGainSegment>,
}
#[derive(Clone, Copy)]
struct ClipGainSegment {
    end: u64,
    gain: f32,
    control: f32,
}
#[derive(Clone, Copy)]
struct ScheduledNoteEvent {
    sample: u64,
    kind: NoteEventKind,
}
struct SendRoute {
    bus: usize,
    gain: Smoother,
    pre: bool,
}

struct ScheduledEffect {
    processor: Box<dyn DspEffect>,
    runtime: NodeRuntime,
}

impl ScheduledEffect {
    fn new(processor: Box<dyn DspEffect>) -> Self {
        let runtime = NodeRuntime::new(processor.runtime_capabilities());
        Self { processor, runtime }
    }

    #[inline]
    fn process(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        activity: BlockActivity,
        frames: usize,
        scheduler_enabled: bool,
        metrics: &mut SchedulerSnapshot,
    ) -> bool {
        let decision = self
            .runtime
            .begin_block(activity, scheduler_enabled, frames);
        if decision == ProcessDecision::SkipAndClear {
            buffer.clear(frames);
        } else {
            self.processor
                .process_with_sidechain(&[], buffer, sidechain, frames);
        }
        metrics.observe(&self.runtime, decision);
        decision == ProcessDecision::Process
    }

    fn set_param(&mut self, id: &str, value: f32) {
        self.processor.set_param(id, value);
        self.runtime
            .update_capabilities(self.processor.runtime_capabilities());
        self.runtime.wake(WakeReason::Parameter);
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.processor.set_bypassed(bypassed);
        self.runtime.wake(WakeReason::Bypass);
    }

    fn reset(&mut self) {
        self.processor.reset();
        self.runtime.reset();
    }
}

struct ScheduledInstrument {
    processor: Box<dyn Instrument>,
    runtime: NodeRuntime,
}

impl ScheduledInstrument {
    fn new(processor: Box<dyn Instrument>) -> Self {
        let runtime = NodeRuntime::new(processor.runtime_capabilities());
        Self { processor, runtime }
    }

    #[inline]
    fn process(
        &mut self,
        events: &[NoteEvent],
        out: &mut AudioBuffer,
        frames: usize,
        scheduler_enabled: bool,
        metrics: &mut SchedulerSnapshot,
    ) -> bool {
        let activity = BlockActivity {
            main_input: self.processor.active_voice_count() > 0,
            midi: !events.is_empty(),
            ..BlockActivity::default()
        };
        let decision = self
            .runtime
            .begin_block(activity, scheduler_enabled, frames);
        if decision == ProcessDecision::SkipAndClear {
            out.clear(frames);
        } else {
            self.processor.process(events, out, frames);
        }
        metrics.observe(&self.runtime, decision);
        decision == ProcessDecision::Process
    }

    fn set_param(&mut self, id: &str, value: f32) {
        self.processor.set_param(id, value);
        self.runtime
            .update_capabilities(self.processor.runtime_capabilities());
        self.runtime.wake(WakeReason::Parameter);
    }

    fn reset(&mut self) {
        self.processor.reset();
        self.runtime.reset();
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SidechainSource {
    Track(usize),
    Bus(usize),
}
struct DelayComp {
    data: [Vec<f32>; MAX_CHANNELS],
    index: usize,
    delay: usize,
    activity_remaining: usize,
}
impl DelayComp {
    fn new(delay: usize) -> Self {
        let len = delay.max(1) + MAX_BLOCK_SIZE;
        Self {
            data: [vec![0.0; len], vec![0.0; len]],
            index: 0,
            delay,
            activity_remaining: 0,
        }
    }
    fn process(&mut self, b: &mut AudioBuffer, n: usize, input_active: bool) -> bool {
        if self.delay == 0 {
            return input_active;
        }
        let len = self.data[0].len();
        for i in 0..n {
            for ch in 0..2 {
                let read = (self.index + len - self.delay) % len;
                let out = self.data[ch][read];
                self.data[ch][self.index] = b.channels[ch][i];
                b.channels[ch][i] = out
            }
            self.index = (self.index + 1) % len
        }
        if input_active {
            self.activity_remaining = self.delay.saturating_add(n);
        } else {
            self.activity_remaining = self.activity_remaining.saturating_sub(n);
        }
        input_active || self.activity_remaining > 0
    }
    fn reset(&mut self) {
        for channel in &mut self.data {
            channel.fill(0.0);
        }
        self.index = 0;
        self.activity_remaining = 0;
    }
}
struct TrackNode {
    clips: Vec<ClipEvent>,
    cursor: usize,
    midi_events: Vec<ScheduledNoteEvent>,
    midi_cursor: usize,
    live_events: Vec<NoteEvent>,
    event_buffer: Vec<NoteEvent>,
    instrument: Option<ScheduledInstrument>,
    effects: Vec<ScheduledEffect>,
    effect_sidechains: Vec<Option<SidechainSource>>,
    buffer: AudioBuffer,
    gain: Smoother,
    pan: Smoother,
    muted: bool,
    solo: bool,
    sends: Vec<SendRoute>,
    output_bus: Option<usize>,
    pdc: DelayComp,
}
struct BusNode {
    effects: Vec<ScheduledEffect>,
    effect_sidechains: Vec<Option<SidechainSource>>,
    buffer: AudioBuffer,
    gain: Smoother,
}
pub struct AudioGraph {
    sample_rate: u32,
    tempo: TempoMap,
    tracks: Vec<TrackNode>,
    buses: Vec<BusNode>,
    master_effects: Vec<ScheduledEffect>,
    master_effect_sidechains: Vec<Option<SidechainSource>>,
    master_buffer: AudioBuffer,
    master_gain: Smoother,
    loop_enabled: bool,
    loop_start: u64,
    loop_end: u64,
    max_latency: usize,
    sidechain_taps: Vec<AudioBuffer>,
    next_sidechain_taps: Vec<AudioBuffer>,
    bus_sidechain_taps: Vec<AudioBuffer>,
    next_bus_sidechain_taps: Vec<AudioBuffer>,
    sidechain_activity: Vec<bool>,
    next_sidechain_activity: Vec<bool>,
    bus_sidechain_activity: Vec<bool>,
    next_bus_sidechain_activity: Vec<bool>,
    bus_input_activity: Vec<bool>,
    scheduler_enabled: bool,
    scheduler_snapshot: SchedulerSnapshot,
}

impl AudioGraph {
    pub fn build(
        spec: &GraphSnapshot,
        assets: &HashMap<String, Arc<AudioAsset>>,
        sample_rate: u32,
    ) -> Result<(Box<Self>, Bindings), String> {
        let mut plugins = StableProcessorRegistry::default();
        Self::build_with_plugins(spec, assets, sample_rate, &mut plugins)
    }

    pub fn build_with_plugins(
        spec: &GraphSnapshot,
        assets: &HashMap<String, Arc<AudioAsset>>,
        sample_rate: u32,
        plugins: &mut StableProcessorRegistry,
    ) -> Result<(Box<Self>, Bindings), String> {
        let sr = sample_rate as f32;
        let mut effects = HashMap::new();
        let mut plugin_controls = HashMap::new();
        let mut tracks_map = HashMap::new();
        let mut sends_map = HashMap::new();
        let mut buses_map = HashMap::new();
        let mut track_ids = Vec::new();
        let mut multiband_meter_ids = Vec::new();
        let mut distortion_meter_ids = Vec::new();
        let mut limiter_meter_ids = Vec::new();
        let mut latencies = Vec::new();
        let mut tracks = Vec::new();
        let mut live_effect_ids = HashSet::new();
        let mut live_instrument_ids = HashSet::new();
        let tempo = if spec.transport.tempo_points.is_empty() {
            TempoMap::new(spec.transport.bpm, sample_rate)
        } else {
            TempoMap::from_specs(
                &spec.transport.tempo_points,
                &spec.transport.time_signatures,
                sample_rate,
            )
        };
        let mut next_note_id = 1_i32;
        for (ts_idx, ts) in spec.tracks.iter().take(MAX_TRACKS).enumerate() {
            tracks_map.insert(ts.id.clone(), ts_idx);
            track_ids.push(ts.id.clone());
            let mut chain = Vec::new();
            let mut effect_sidechains = Vec::new();
            for (es_idx, es) in ts.effects.iter().enumerate() {
                live_effect_ids.insert(es.id.clone());
                let fx = Some(plugins.effect(&es.id, es, sr).map_err(|error| {
                    format!("failed to load effect '{}' ({}): {error}", es.id, es.kind)
                })?);
                if let Some(fx) = fx {
                    if let Some(control) = fx.plugin_control() {
                        plugin_controls.insert(es.id.clone(), control);
                    }
                    effect_sidechains.push(es.sidechain.as_ref().and_then(|sidechain| {
                        if !sidechain.enabled {
                            return None;
                        }
                        let source =
                            resolve_sidechain_source(spec, sidechain.source_track_id.as_deref()?)?;
                        (source != SidechainSource::Track(ts_idx)).then_some(source)
                    }));
                    if es.kind == "builtin:multiband-compressor"
                        && multiband_meter_ids.len() < MAX_EFFECT_METERS
                    {
                        multiband_meter_ids.push(es.id.clone())
                    }
                    if effect_has_spectrum(&es.kind)
                        && distortion_meter_ids.len() < MAX_EFFECT_METERS
                    {
                        distortion_meter_ids.push(es.id.clone())
                    }
                    if effect_has_limiter_metrics(&es.kind)
                        && limiter_meter_ids.len() < MAX_EFFECT_METERS
                    {
                        limiter_meter_ids.push(es.id.clone())
                    }
                    effects.insert(
                        es.id.clone(),
                        EffectRef {
                            chain: ChainKind::Track,
                            owner: ts_idx,
                            effect: chain.len(),
                        },
                    );
                    let _ = es_idx;
                    chain.push(ScheduledEffect::new(fx))
                }
            }
            let latency = chain
                .iter()
                .map(|effect| effect.processor.latency_samples())
                .sum();
            latencies.push(latency);
            let mut clips = Vec::new();
            for c in &ts.clips {
                if c.muted {
                    continue;
                }
                if let Some(asset) = assets.get(&c.asset_id) {
                    clips.push(ClipEvent {
                        asset: Arc::clone(asset),
                        start: sec_to_samples(c.start_sec, sample_rate),
                        end: sec_to_samples(c.start_sec + c.duration_sec, sample_rate),
                        offset: sec_to_samples(c.offset_sec, sample_rate),
                        gain: db_to_gain(c.gain_db),
                        fade_in: sec_to_samples(c.fade_in_sec, sample_rate),
                        fade_out: sec_to_samples(c.fade_out_sec, sample_rate),
                        source_step: c.playback_rate.clamp(0.05, 8.0)
                            * 2.0_f64.powf(
                                (c.pitch_semitones.clamp(-48.0, 48.0)
                                    + c.fine_cents.clamp(-100.0, 100.0) / 100.0)
                                    / 12.0,
                            ),
                        reversed: c.reversed,
                        fade_in_curve: c.fade_in_curve.clamp(-1.0, 1.0),
                        fade_out_curve: c.fade_out_curve.clamp(-1.0, 1.0),
                        gain_segments: {
                            let mut points: Vec<_> = c
                                .gain_points
                                .iter()
                                .map(|point| {
                                    (
                                        sec_to_samples(
                                            point.time_sec.max(0.0).min(c.duration_sec),
                                            sample_rate,
                                        ),
                                        point.value_db.clamp(-60.0, 12.0),
                                        point.curve.clamp(-1.0, 1.0),
                                    )
                                })
                                .collect();
                            points.sort_by_key(|point| point.0);
                            let mut previous = (0_u64, c.gain_db.clamp(-60.0, 12.0), 0.0_f32);
                            let mut segments = Vec::with_capacity(points.len() + 1);
                            for point in points {
                                let end_gain = db_to_gain(point.1);
                                segments.push(ClipGainSegment {
                                    end: point.0,
                                    gain: end_gain,
                                    control: clip_gain_control(previous.1, point.1, previous.2),
                                });
                                previous = point;
                            }
                            if !segments.is_empty() {
                                let end = sec_to_samples(c.duration_sec, sample_rate);
                                let end_db = c.gain_db.clamp(-60.0, 12.0);
                                let end_gain = db_to_gain(end_db);
                                segments.push(ClipGainSegment {
                                    end,
                                    gain: end_gain,
                                    control: clip_gain_control(previous.1, end_db, previous.2),
                                });
                            }
                            segments
                        },
                    })
                }
            }
            clips.sort_by_key(|v| v.start);
            let mut midi_events = Vec::new();
            for clip in ts.midi_clips.iter().filter(|clip| !clip.muted) {
                let clip_start = sec_to_samples(clip.start_sec, sample_rate);
                let clip_start_tick = tempo.samples_to_ticks(clip_start);
                let clip_tick_sample = tempo.ticks_to_samples(clip_start_tick);
                let clip_length = sec_to_samples(clip.duration_sec, sample_rate);
                let loop_length = clip.loop_length_ticks.max(1);
                // Controller points are scheduled before notes so a point placed on
                // the same tick as a note affects that note deterministically.
                for lane in &clip.cc_lanes {
                    for point in &lane.points {
                        let mut cycle = 0_u64;
                        loop {
                            let tick = if clip.loop_enabled {
                                point.ticks.saturating_sub(clip.loop_start_ticks)
                                    + cycle * loop_length
                            } else {
                                point.ticks
                            };
                            let relative = tempo
                                .ticks_to_samples(clip_start_tick.saturating_add(tick))
                                .saturating_sub(clip_tick_sample);
                            if relative >= clip_length {
                                break;
                            }
                            let kind = if lane.cc == -1 {
                                NoteEventKind::PitchBend {
                                    value: (f32::from(point.value) / 8192.0).clamp(-1.0, 1.0),
                                }
                            } else if (0..=127).contains(&lane.cc) {
                                NoteEventKind::Controller {
                                    cc: lane.cc as u8,
                                    value: (f32::from(point.value) / 127.0).clamp(0.0, 1.0),
                                }
                            } else {
                                break;
                            };
                            midi_events.push(ScheduledNoteEvent {
                                sample: clip_start + relative,
                                kind,
                            });
                            if !clip.loop_enabled {
                                break;
                            }
                            cycle += 1;
                        }
                    }
                }
                for note in clip.notes.iter().filter(|note| !note.muted) {
                    let mut cycle = 0_u64;
                    loop {
                        let tick = if clip.loop_enabled {
                            note.start_ticks.saturating_sub(clip.loop_start_ticks)
                                + cycle * loop_length
                        } else {
                            note.start_ticks
                        };
                        let event_tick = clip_start_tick.saturating_add(tick);
                        let relative = tempo
                            .ticks_to_samples(event_tick)
                            .saturating_sub(clip_tick_sample);
                        if relative >= clip_length {
                            break;
                        }
                        let pitch =
                            (i16::from(note.pitch) + clip.transpose_semitones).clamp(0, 127) as u8;
                        let velocity = (f32::from(note.velocity) / 127.0 * clip.velocity_scale)
                            .clamp(0.0, 1.0);
                        let note_id = next_note_id;
                        next_note_id = next_note_id.wrapping_add(1).max(1);
                        midi_events.push(ScheduledNoteEvent {
                            sample: clip_start + relative,
                            kind: NoteEventKind::NoteOn {
                                note_id,
                                pitch,
                                velocity,
                                tuning_cents: 0.0,
                            },
                        });
                        let off = tempo
                            .ticks_to_samples(event_tick.saturating_add(note.length_ticks))
                            .saturating_sub(clip_tick_sample)
                            .min(clip_length);
                        midi_events.push(ScheduledNoteEvent {
                            sample: clip_start + off,
                            kind: NoteEventKind::NoteOff {
                                note_id,
                                pitch,
                                velocity: f32::from(note.release_velocity) / 127.0,
                            },
                        });
                        if !clip.loop_enabled {
                            break;
                        }
                        cycle += 1;
                    }
                }
            }
            midi_events.sort_by_key(|event| event.sample);
            let instrument = match ts.instrument.as_ref() {
                Some(instrument) => {
                    live_instrument_ids.insert(ts.id.clone());
                    plugins
                        .instrument(&ts.id, instrument, sr)
                        .map_err(|error| {
                            format!(
                                "failed to load instrument for track '{}' ({}): {error}",
                                ts.id, instrument.kind
                            )
                        })?
                        .map(ScheduledInstrument::new)
                }
                None => None,
            };
            if let Some(control) = instrument
                .as_ref()
                .and_then(|instrument| instrument.processor.plugin_control())
            {
                plugin_controls.insert(ts.id.clone(), control);
            }
            let sends = ts
                .sends
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let bus = spec.buses.iter().position(|b| b.id == s.target_bus_id)?;
                    sends_map.insert(s.id.clone(), (ts_idx, i));
                    Some(SendRoute {
                        bus,
                        gain: Smoother::new(db_to_gain(s.gain_db), sr, 0.015),
                        pre: s.pre_fader,
                    })
                })
                .collect();
            let output_bus = ts
                .output_bus_id
                .as_deref()
                .and_then(|id| spec.buses.iter().position(|bus| bus.id == id));
            tracks.push(TrackNode {
                clips,
                cursor: 0,
                midi_events,
                midi_cursor: 0,
                live_events: Vec::with_capacity(128),
                event_buffer: Vec::with_capacity(1024),
                instrument,
                effects: chain,
                effect_sidechains,
                buffer: AudioBuffer::new(),
                gain: Smoother::new(db_to_gain(ts.volume_db), sr, 0.015),
                pan: Smoother::new(ts.pan, sr, 0.015),
                muted: ts.muted,
                solo: ts.solo,
                sends,
                output_bus,
                pdc: DelayComp::new(0),
            })
        }
        let max_latency = latencies.iter().copied().max().unwrap_or(0);
        for (i, t) in tracks.iter_mut().enumerate() {
            t.pdc = DelayComp::new(max_latency - latencies[i])
        }
        let mut buses = Vec::new();
        for (bs_idx, bs) in spec.buses.iter().enumerate() {
            buses_map.insert(bs.id.clone(), bs_idx);
            let mut chain = Vec::new();
            let mut effect_sidechains = Vec::new();
            for es in &bs.effects {
                live_effect_ids.insert(es.id.clone());
                let fx = Some(plugins.effect(&es.id, es, sr).map_err(|error| {
                    format!(
                        "failed to load bus effect '{}' ({}): {error}",
                        es.id, es.kind
                    )
                })?);
                if let Some(fx) = fx {
                    if let Some(control) = fx.plugin_control() {
                        plugin_controls.insert(es.id.clone(), control);
                    }
                    effect_sidechains.push(es.sidechain.as_ref().and_then(|sidechain| {
                        if !sidechain.enabled {
                            return None;
                        }
                        let source =
                            resolve_sidechain_source(spec, sidechain.source_track_id.as_deref()?)?;
                        (source != SidechainSource::Bus(bs_idx)).then_some(source)
                    }));
                    if es.kind == "builtin:multiband-compressor"
                        && multiband_meter_ids.len() < MAX_EFFECT_METERS
                    {
                        multiband_meter_ids.push(es.id.clone())
                    }
                    if effect_has_spectrum(&es.kind)
                        && distortion_meter_ids.len() < MAX_EFFECT_METERS
                    {
                        distortion_meter_ids.push(es.id.clone())
                    }
                    if effect_has_limiter_metrics(&es.kind)
                        && limiter_meter_ids.len() < MAX_EFFECT_METERS
                    {
                        limiter_meter_ids.push(es.id.clone())
                    }
                    effects.insert(
                        es.id.clone(),
                        EffectRef {
                            chain: ChainKind::Bus,
                            owner: bs_idx,
                            effect: chain.len(),
                        },
                    );
                    chain.push(ScheduledEffect::new(fx))
                }
            }
            buses.push(BusNode {
                effects: chain,
                effect_sidechains,
                buffer: AudioBuffer::new(),
                gain: Smoother::new(db_to_gain(bs.volume_db), sr, 0.015),
            })
        }
        let mut master_effects = Vec::new();
        let mut master_effect_sidechains = Vec::new();
        for es in &spec.master.effects {
            live_effect_ids.insert(es.id.clone());
            let fx = Some(plugins.effect(&es.id, es, sr).map_err(|error| {
                format!(
                    "failed to load master effect '{}' ({}): {error}",
                    es.id, es.kind
                )
            })?);
            if let Some(fx) = fx {
                if let Some(control) = fx.plugin_control() {
                    plugin_controls.insert(es.id.clone(), control);
                }
                master_effect_sidechains.push(es.sidechain.as_ref().and_then(|sidechain| {
                    if !sidechain.enabled {
                        return None;
                    }
                    resolve_sidechain_source(spec, sidechain.source_track_id.as_deref()?)
                }));
                if es.kind == "builtin:multiband-compressor"
                    && multiband_meter_ids.len() < MAX_EFFECT_METERS
                {
                    multiband_meter_ids.push(es.id.clone())
                }
                if effect_has_spectrum(&es.kind) && distortion_meter_ids.len() < MAX_EFFECT_METERS {
                    distortion_meter_ids.push(es.id.clone())
                }
                if effect_has_limiter_metrics(&es.kind)
                    && limiter_meter_ids.len() < MAX_EFFECT_METERS
                {
                    limiter_meter_ids.push(es.id.clone())
                }
                effects.insert(
                    es.id.clone(),
                    EffectRef {
                        chain: ChainKind::Master,
                        owner: 0,
                        effect: master_effects.len(),
                    },
                );
                master_effects.push(ScheduledEffect::new(fx))
            }
        }
        let track_count = tracks.len();
        let bus_count = buses.len();
        plugins.retain(&live_effect_ids, &live_instrument_ids);
        Ok((
            Box::new(Self {
                sample_rate,
                tempo,
                tracks,
                buses,
                master_effects,
                master_effect_sidechains,
                master_buffer: AudioBuffer::new(),
                master_gain: Smoother::new(db_to_gain(spec.master.volume_db), sr, 0.015),
                loop_enabled: spec.transport.loop_.enabled,
                loop_start: sec_to_samples(spec.transport.loop_.start_sec, sample_rate),
                loop_end: sec_to_samples(spec.transport.loop_.end_sec, sample_rate),
                max_latency,
                sidechain_taps: (0..track_count).map(|_| AudioBuffer::new()).collect(),
                next_sidechain_taps: (0..track_count).map(|_| AudioBuffer::new()).collect(),
                bus_sidechain_taps: (0..bus_count).map(|_| AudioBuffer::new()).collect(),
                next_bus_sidechain_taps: (0..bus_count).map(|_| AudioBuffer::new()).collect(),
                sidechain_activity: vec![false; track_count],
                next_sidechain_activity: vec![false; track_count],
                bus_sidechain_activity: vec![false; bus_count],
                next_bus_sidechain_activity: vec![false; bus_count],
                bus_input_activity: vec![false; bus_count],
                scheduler_enabled: true,
                scheduler_snapshot: SchedulerSnapshot::default(),
            }),
            Bindings {
                tracks: tracks_map,
                buses: buses_map,
                sends: sends_map,
                effects,
                plugin_controls,
                track_ids,
                multiband_meter_ids,
                distortion_meter_ids,
                limiter_meter_ids,
            },
        ))
    }
    pub fn empty(sample_rate: u32) -> Box<Self> {
        let s = GraphSnapshot {
            tracks: Vec::new(),
            buses: Vec::new(),
            master: super::types::MasterSpec {
                volume_db: 0.0,
                effects: Vec::new(),
            },
            transport: super::types::TransportSpec {
                bpm: 120.0,
                tempo_points: Vec::new(),
                time_signatures: Vec::new(),
                playhead_sec: 0.0,
                is_playing: false,
                loop_: super::types::LoopSpec {
                    enabled: false,
                    start_sec: 0.0,
                    end_sec: 0.0,
                },
            },
        };
        Self::build(&s, &HashMap::new(), sample_rate)
            .expect("empty graph construction cannot fail")
            .0
    }
    pub fn process(
        &mut self,
        position: u64,
        frames: usize,
        out: &mut [f32],
        levels: &mut [Level; MAX_TRACKS],
        master: &mut Level,
        timeline: bool,
    ) {
        let mut scheduler = SchedulerSnapshot {
            skipped_process_calls: self.scheduler_snapshot.skipped_process_calls,
            wake_count: self.scheduler_snapshot.wake_count,
            sleep_count: self.scheduler_snapshot.sleep_count,
            ..SchedulerSnapshot::default()
        };
        let tempo_bpm = self
            .tempo
            .bpm_at_tick(self.tempo.samples_to_ticks(position));
        for track in &mut self.tracks {
            if let Some(instrument) = &mut track.instrument {
                instrument.processor.set_tempo(tempo_bpm);
            }
            for effect in &mut track.effects {
                effect.processor.set_tempo(tempo_bpm);
            }
        }
        for bus in &mut self.buses {
            for effect in &mut bus.effects {
                effect.processor.set_tempo(tempo_bpm);
            }
        }
        for effect in &mut self.master_effects {
            effect.processor.set_tempo(tempo_bpm);
        }
        self.master_buffer.clear(frames);
        for b in &mut self.buses {
            b.buffer.clear(frames)
        }
        let has_solo = self.tracks.iter().any(|t| t.solo);
        for tap in &mut self.next_sidechain_taps {
            tap.clear(frames)
        }
        for tap in &mut self.next_bus_sidechain_taps {
            tap.clear(frames)
        }
        self.next_sidechain_activity.fill(false);
        self.next_bus_sidechain_activity.fill(false);
        self.bus_input_activity.fill(false);
        let mut master_activity = false;
        let sidechain_taps = &self.sidechain_taps;
        let bus_sidechain_taps = &self.bus_sidechain_taps;
        let sidechain_activity = &self.sidechain_activity;
        let bus_sidechain_activity = &self.bus_sidechain_activity;
        for (index, track) in self.tracks.iter_mut().enumerate() {
            track.buffer.clear(frames);
            let mut track_activity = false;
            if timeline {
                while track.cursor < track.clips.len() && track.clips[track.cursor].end <= position
                {
                    track.cursor += 1
                }
                for clip in track.clips.iter().skip(track.cursor) {
                    if clip.start >= position + frames as u64 {
                        break;
                    }
                    render_clip(clip, position, frames, &mut track.buffer);
                    track_activity = true;
                }
            }
            if let Some(instrument) = &mut track.instrument {
                track.event_buffer.clear();
                if timeline {
                    while track.midi_cursor < track.midi_events.len()
                        && track.midi_events[track.midi_cursor].sample < position
                    {
                        track.midi_cursor += 1;
                    }
                    let mut cursor = track.midi_cursor;
                    while cursor < track.midi_events.len() {
                        let event = track.midi_events[cursor];
                        if event.sample >= position + frames as u64 {
                            break;
                        }
                        if track.event_buffer.len() < 1024 {
                            track.event_buffer.push(NoteEvent {
                                sample_offset: (event.sample - position) as u32,
                                kind: event.kind,
                            });
                        }
                        cursor += 1;
                    }
                    track.midi_cursor = cursor;
                }
                let live_capacity = track
                    .event_buffer
                    .capacity()
                    .saturating_sub(track.event_buffer.len())
                    .min(128);
                track
                    .event_buffer
                    .extend(track.live_events.drain(..).take(live_capacity));
                // The timeline prefix is sorted and live packets are bounded. An
                // unstable in-place sort cannot allocate, unlike slice::sort.
                track
                    .event_buffer
                    .sort_unstable_by_key(|event| event.sample_offset);
                track_activity |= instrument.process(
                    &track.event_buffer,
                    &mut track.buffer,
                    frames,
                    self.scheduler_enabled,
                    &mut scheduler,
                );
            } else {
                track.live_events.clear();
            }
            for (effect_index, fx) in track.effects.iter_mut().enumerate() {
                let source = track.effect_sidechains.get(effect_index).copied().flatten();
                let sidechain = source.and_then(|source| {
                    sidechain_from_taps(source, sidechain_taps, bus_sidechain_taps)
                });
                let sidechain_active = source.is_some_and(|source| match source {
                    SidechainSource::Track(index) => {
                        sidechain_activity.get(index).copied().unwrap_or(false)
                    }
                    SidechainSource::Bus(index) => {
                        bus_sidechain_activity.get(index).copied().unwrap_or(false)
                    }
                });
                // Effect-note routing is intentionally not connected yet. The
                // event-aware DSP contract is live, but inserts receive an
                // allocation-free empty slice until a routing source exists.
                // Querying the capability keeps graph construction ready for
                // a routed source without implicitly borrowing instrument MIDI.
                let _awaiting_midi_route = fx.processor.wants_midi();
                track_activity = fx.process(
                    &mut track.buffer,
                    sidechain,
                    BlockActivity {
                        main_input: track_activity,
                        sidechain: sidechain_active,
                        ..BlockActivity::default()
                    },
                    frames,
                    self.scheduler_enabled,
                    &mut scheduler,
                );
            }
            let silent = track.muted || (has_solo && !track.solo);
            let mut peak = 0.0_f32;
            let mut sum = 0.0;
            for i in 0..frames {
                let gain = if silent { 0.0 } else { track.gain.next() };
                let pan = track.pan.next().clamp(-1.0, 1.0);
                let gl = gain * ((1.0 - pan) * 0.5).sqrt();
                let gr = gain * ((1.0 + pan) * 0.5).sqrt();
                let pre = [track.buffer.channels[0][i], track.buffer.channels[1][i]];
                track.buffer.channels[0][i] *= gl;
                track.buffer.channels[1][i] *= gr;
                let post = [track.buffer.channels[0][i], track.buffer.channels[1][i]];
                peak = peak.max(post[0].abs().max(post[1].abs()));
                sum += post[0] * post[0] + post[1] * post[1];
                for send in &mut track.sends {
                    if let Some(bus) = self.buses.get_mut(send.bus) {
                        let src = if send.pre { pre } else { post };
                        let g = send.gain.next();
                        bus.buffer.channels[0][i] += src[0] * g;
                        bus.buffer.channels[1][i] += src[1] * g
                    }
                }
            }
            for send in &track.sends {
                if let Some(active) = self.bus_input_activity.get_mut(send.bus) {
                    *active |= track_activity && (send.pre || !silent);
                }
            }
            let track_output_activity =
                track
                    .pdc
                    .process(&mut track.buffer, frames, track_activity && !silent);
            for channel in 0..MAX_CHANNELS {
                self.next_sidechain_taps[index].channels[channel][..frames]
                    .copy_from_slice(&track.buffer.channels[channel][..frames])
            }
            self.next_sidechain_activity[index] = track_output_activity;
            if let Some(bus) = track.output_bus.and_then(|index| self.buses.get_mut(index)) {
                for ch in 0..2 {
                    for i in 0..frames {
                        bus.buffer.channels[ch][i] += track.buffer.channels[ch][i]
                    }
                }
                if let Some(active) = track
                    .output_bus
                    .and_then(|index| self.bus_input_activity.get_mut(index))
                {
                    *active |= track_output_activity;
                }
            } else {
                for ch in 0..2 {
                    for i in 0..frames {
                        self.master_buffer.channels[ch][i] += track.buffer.channels[ch][i]
                    }
                }
                master_activity |= track_output_activity;
            }
            if let Some(m) = levels.get_mut(index) {
                m.peak = peak;
                m.rms = (sum / (frames.max(1) * 2) as f32).sqrt()
            }
        }
        for (bus_index, bus) in self.buses.iter_mut().enumerate() {
            let mut bus_activity = self.bus_input_activity[bus_index];
            for (effect_index, fx) in bus.effects.iter_mut().enumerate() {
                let source = bus.effect_sidechains.get(effect_index).copied().flatten();
                let sidechain = source.and_then(|source| {
                    sidechain_from_taps(source, sidechain_taps, bus_sidechain_taps)
                });
                let sidechain_active = source.is_some_and(|source| match source {
                    SidechainSource::Track(index) => {
                        sidechain_activity.get(index).copied().unwrap_or(false)
                    }
                    SidechainSource::Bus(index) => {
                        bus_sidechain_activity.get(index).copied().unwrap_or(false)
                    }
                });
                bus_activity = fx.process(
                    &mut bus.buffer,
                    sidechain,
                    BlockActivity {
                        main_input: bus_activity,
                        sidechain: sidechain_active,
                        ..BlockActivity::default()
                    },
                    frames,
                    self.scheduler_enabled,
                    &mut scheduler,
                );
            }
            for i in 0..frames {
                let g = bus.gain.next();
                let left = bus.buffer.channels[0][i] * g;
                let right = bus.buffer.channels[1][i] * g;
                self.next_bus_sidechain_taps[bus_index].channels[0][i] = left;
                self.next_bus_sidechain_taps[bus_index].channels[1][i] = right;
                self.master_buffer.channels[0][i] += left;
                self.master_buffer.channels[1][i] += right
            }
            self.next_bus_sidechain_activity[bus_index] = bus_activity;
            master_activity |= bus_activity;
        }
        for (effect_index, fx) in self.master_effects.iter_mut().enumerate() {
            let source = self
                .master_effect_sidechains
                .get(effect_index)
                .copied()
                .flatten();
            let sidechain = source
                .and_then(|source| sidechain_from_taps(source, sidechain_taps, bus_sidechain_taps));
            let sidechain_active = source.is_some_and(|source| match source {
                SidechainSource::Track(index) => {
                    sidechain_activity.get(index).copied().unwrap_or(false)
                }
                SidechainSource::Bus(index) => {
                    bus_sidechain_activity.get(index).copied().unwrap_or(false)
                }
            });
            master_activity = fx.process(
                &mut self.master_buffer,
                sidechain,
                BlockActivity {
                    main_input: master_activity,
                    sidechain: sidechain_active,
                    ..BlockActivity::default()
                },
                frames,
                self.scheduler_enabled,
                &mut scheduler,
            );
        }
        let (mut peak, mut sum) = (0.0_f32, 0.0_f32);
        for (i, frame) in out.chunks_exact_mut(2).take(frames).enumerate() {
            let g = self.master_gain.next();
            let l = clipper_curve(self.master_buffer.channels[0][i] * g, MASTER_SAFETY_KNEE);
            let r = clipper_curve(self.master_buffer.channels[1][i] * g, MASTER_SAFETY_KNEE);
            frame[0] = l;
            frame[1] = r;
            peak = peak.max(l.abs().max(r.abs()));
            sum += l * l + r * r
        }
        master.peak = peak;
        master.rms = (sum / (frames.max(1) * 2) as f32).sqrt();
        std::mem::swap(&mut self.sidechain_taps, &mut self.next_sidechain_taps);
        std::mem::swap(
            &mut self.bus_sidechain_taps,
            &mut self.next_bus_sidechain_taps,
        );
        std::mem::swap(
            &mut self.sidechain_activity,
            &mut self.next_sidechain_activity,
        );
        std::mem::swap(
            &mut self.bus_sidechain_activity,
            &mut self.next_bus_sidechain_activity,
        );
        self.scheduler_snapshot = scheduler;
    }
    /// Positions a newly built graph at the current transport sample without
    /// resetting stable processors shared with the retiring graph. Structural
    /// edits use this path so held MIDI voices and effect tails survive the
    /// callback-boundary swap.
    pub fn activate(&mut self, position: u64) {
        self.reposition(position, false)
    }

    /// Destructive transport reset used by seek/stop/loop. Unlike `activate`,
    /// this intentionally releases voices and clears processor history.
    pub fn reset(&mut self, position: u64) {
        self.reposition(position, true)
    }

    fn reposition(&mut self, position: u64, reset_processors: bool) {
        for tap in self
            .sidechain_taps
            .iter_mut()
            .chain(self.next_sidechain_taps.iter_mut())
        {
            tap.clear(MAX_BLOCK_SIZE)
        }
        self.sidechain_activity.fill(false);
        self.next_sidechain_activity.fill(false);
        self.bus_sidechain_activity.fill(false);
        self.next_bus_sidechain_activity.fill(false);
        self.bus_input_activity.fill(false);
        for tap in self
            .bus_sidechain_taps
            .iter_mut()
            .chain(self.next_bus_sidechain_taps.iter_mut())
        {
            tap.clear(MAX_BLOCK_SIZE)
        }
        for t in &mut self.tracks {
            t.cursor = t.clips.partition_point(|c| c.end <= position);
            t.midi_cursor = t
                .midi_events
                .partition_point(|event| event.sample < position);
            t.live_events.clear();
            if reset_processors {
                if let Some(instrument) = &mut t.instrument {
                    instrument.reset();
                }
                for fx in &mut t.effects {
                    fx.reset()
                }
            }
            t.pdc.reset();
        }
        if reset_processors {
            for b in &mut self.buses {
                for fx in &mut b.effects {
                    fx.reset()
                }
            }
            for fx in &mut self.master_effects {
                fx.reset()
            }
        }
    }
    pub fn set_effect(&mut self, target: EffectRef, param: &str, value: f32) {
        let fx: Option<&mut ScheduledEffect> = match target.chain {
            ChainKind::Track => self
                .tracks
                .get_mut(target.owner)
                .and_then(|v| v.effects.get_mut(target.effect)),
            ChainKind::Bus => self
                .buses
                .get_mut(target.owner)
                .and_then(|v| v.effects.get_mut(target.effect)),
            ChainKind::Master => self.master_effects.get_mut(target.effect),
        };
        if let Some(v) = fx {
            v.set_param(param, value)
        }
    }
    pub fn set_effect_bypassed(&mut self, target: EffectRef, bypassed: bool) {
        let effect: Option<&mut ScheduledEffect> = match target.chain {
            ChainKind::Track => self
                .tracks
                .get_mut(target.owner)
                .and_then(|track| track.effects.get_mut(target.effect)),
            ChainKind::Bus => self
                .buses
                .get_mut(target.owner)
                .and_then(|bus| bus.effects.get_mut(target.effect)),
            ChainKind::Master => self.master_effects.get_mut(target.effect),
        };
        if let Some(effect) = effect {
            effect.set_bypassed(bypassed)
        }
    }
    pub fn push_live_event(&mut self, track: usize, event: NoteEvent) {
        if let Some(node) = self.tracks.get_mut(track) {
            if node.live_events.len() < 128 {
                node.live_events.push(event);
            }
        }
    }
    pub fn all_notes_off(&mut self) {
        for track in &mut self.tracks {
            if track.instrument.is_some() && track.live_events.len() < 128 {
                track.live_events.push(NoteEvent {
                    sample_offset: 0,
                    kind: NoteEventKind::AllNotesOff,
                })
            }
        }
    }
    pub fn wake_all(&mut self, reason: WakeReason) {
        for track in &mut self.tracks {
            if let Some(instrument) = &mut track.instrument {
                instrument.runtime.wake(reason);
            }
            for effect in &mut track.effects {
                effect.runtime.wake(reason);
            }
        }
        for bus in &mut self.buses {
            for effect in &mut bus.effects {
                effect.runtime.wake(reason);
            }
        }
        for effect in &mut self.master_effects {
            effect.runtime.wake(reason);
        }
    }

    pub fn set_scheduler_enabled(&mut self, enabled: bool) {
        self.scheduler_enabled = enabled;
        if !enabled {
            self.wake_all(WakeReason::Routing);
        }
    }

    pub fn set_effect_sleep_policy(&mut self, target: EffectRef, policy: SleepPolicy) {
        let effect = match target.chain {
            ChainKind::Track => self
                .tracks
                .get_mut(target.owner)
                .and_then(|track| track.effects.get_mut(target.effect)),
            ChainKind::Bus => self
                .buses
                .get_mut(target.owner)
                .and_then(|bus| bus.effects.get_mut(target.effect)),
            ChainKind::Master => self.master_effects.get_mut(target.effect),
        };
        if let Some(effect) = effect {
            effect.runtime.set_policy(policy);
        }
    }
    pub fn write_multiband_levels(
        &self,
        output: &mut [[[f32; MAX_CHANNELS]; 3]; MAX_EFFECT_METERS],
    ) {
        output.fill([[0.0; MAX_CHANNELS]; 3]);
        let effects = self
            .tracks
            .iter()
            .flat_map(|track| track.effects.iter())
            .chain(self.buses.iter().flat_map(|bus| bus.effects.iter()))
            .chain(self.master_effects.iter());
        let mut index = 0;
        for effect in effects {
            if let Some(levels) = effect.processor.multiband_levels() {
                if index >= MAX_EFFECT_METERS {
                    break;
                }
                output[index] = levels;
                index += 1
            }
        }
    }
    pub fn write_distortion_spectra(
        &self,
        output: &mut [[f32; DISTORTION_SPECTRUM_BINS]; MAX_EFFECT_METERS],
    ) {
        output.fill([0.0; DISTORTION_SPECTRUM_BINS]);
        let effects = self
            .tracks
            .iter()
            .flat_map(|track| track.effects.iter())
            .chain(self.buses.iter().flat_map(|bus| bus.effects.iter()))
            .chain(self.master_effects.iter());
        let mut index = 0;
        for effect in effects {
            if let Some(spectrum) = effect.processor.effect_spectrum() {
                if index >= MAX_EFFECT_METERS {
                    break;
                }
                output[index] = spectrum;
                index += 1
            }
        }
    }
    pub fn write_limiter_metrics(
        &self,
        output: &mut [[f32; LIMITER_METER_VALUES]; MAX_EFFECT_METERS],
    ) {
        output.fill([-120.0, -120.0, 0.0, -120.0, -120.0, -120.0, -120.0]);
        let effects = self
            .tracks
            .iter()
            .flat_map(|track| track.effects.iter())
            .chain(self.buses.iter().flat_map(|bus| bus.effects.iter()))
            .chain(self.master_effects.iter());
        let mut index = 0;
        for effect in effects {
            if let Some(metrics) = effect.processor.limiter_metrics() {
                if index >= MAX_EFFECT_METERS {
                    break;
                }
                output[index] = metrics;
                index += 1
            }
        }
    }
    pub fn set_instrument_param(&mut self, track: usize, param: &str, value: f32) {
        if let Some(instrument) = self
            .tracks
            .get_mut(track)
            .and_then(|node| node.instrument.as_mut())
        {
            instrument.set_param(param, value);
        }
    }
    pub fn write_active_voice_counts(&self, output: &mut [u32; MAX_TRACKS]) {
        output.fill(0);
        for (index, track) in self.tracks.iter().enumerate() {
            output[index] = track.instrument.as_ref().map_or(0, |instrument| {
                instrument.processor.active_voice_count() as u32
            });
        }
    }
    pub fn track_mut(
        &mut self,
        index: usize,
    ) -> Option<(&mut Smoother, &mut Smoother, &mut bool, &mut bool)> {
        self.tracks
            .get_mut(index)
            .map(|t| (&mut t.gain, &mut t.pan, &mut t.muted, &mut t.solo))
    }
    pub fn send_gain(&mut self, t: usize, s: usize) -> Option<&mut Smoother> {
        self.tracks
            .get_mut(t)?
            .sends
            .get_mut(s)
            .map(|v| &mut v.gain)
    }
    pub fn bus_gain(&mut self, index: usize) -> Option<&mut Smoother> {
        self.buses.get_mut(index).map(|bus| &mut bus.gain)
    }
    pub fn master_gain(&mut self) -> &mut Smoother {
        &mut self.master_gain
    }
    pub fn loop_range(&self) -> Option<(u64, u64)> {
        if self.loop_enabled && self.loop_end > self.loop_start {
            Some((self.loop_start, self.loop_end))
        } else {
            None
        }
    }
    pub fn max_latency(&self) -> usize {
        self.max_latency
    }
    pub fn tail_samples(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| track.effects.iter())
            .chain(self.buses.iter().flat_map(|bus| bus.effects.iter()))
            .chain(self.master_effects.iter())
            .map(|effect| effect.processor.tail_samples())
            .chain(self.tracks.iter().filter_map(|track| {
                track
                    .instrument
                    .as_ref()
                    .map(|instrument| instrument.processor.tail_samples())
            }))
            .max()
            .unwrap_or(0)
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn tempo_map(&self) -> &TempoMap {
        &self.tempo
    }

    pub fn scheduler_snapshot(&self) -> SchedulerSnapshot {
        self.scheduler_snapshot
    }
}
fn render_clip(c: &ClipEvent, pos: u64, n: usize, b: &mut AudioBuffer) {
    let from = pos.max(c.start);
    let to = (pos + n as u64).min(c.end);
    for timeline in from..to {
        let dst = (timeline - pos) as usize;
        let rel = timeline - c.start;
        let source_rel = if c.reversed {
            c.end
                .saturating_sub(c.start)
                .saturating_sub(1)
                .saturating_sub(rel)
        } else {
            rel
        };
        let src_pos = c.offset as f64 + source_rel as f64 * c.source_step;
        let src = src_pos.floor() as usize;
        if src >= c.asset.frames {
            if c.reversed {
                continue;
            }
            break;
        }
        let fraction = (src_pos - src as f64) as f32;
        let remaining = c.end - timeline;
        let fade_in = if c.fade_in > 0 {
            fade_shape((rel as f32 / c.fade_in as f32).min(1.0), c.fade_in_curve)
        } else {
            1.0
        };
        let fade_out = if c.fade_out > 0 {
            fade_shape(
                (remaining as f32 / c.fade_out as f32).min(1.0),
                c.fade_out_curve,
            )
        } else {
            1.0
        };
        let envelope_gain = clip_gain(c, rel) * fade_in * fade_out;
        for ch in 0..2 {
            let source = c
                .asset
                .channels
                .get(ch)
                .or_else(|| c.asset.channels.first())
                .map(|v| {
                    let first = v.get(src).copied().unwrap_or(0.0);
                    let second = v
                        .get((src + 1).min(c.asset.frames.saturating_sub(1)))
                        .copied()
                        .unwrap_or(first);
                    first + (second - first) * fraction
                })
                .unwrap_or(0.0);
            b.channels[ch][dst] += source * envelope_gain
        }
    }
}

fn fade_shape(progress: f32, curve: f32) -> f32 {
    progress
        .clamp(0.0, 1.0)
        .powf(2.0_f32.powf(curve.clamp(-1.0, 1.0) * 2.0))
}

fn clip_gain(c: &ClipEvent, rel: u64) -> f32 {
    if c.gain_segments.is_empty() {
        return c.gain;
    }
    let mut previous = (0_u64, c.gain);
    for segment in &c.gain_segments {
        if rel <= segment.end {
            let span = segment.end.saturating_sub(previous.0).max(1) as f32;
            let mix = rel.saturating_sub(previous.0) as f32 / span;
            let mix = mix.clamp(0.0, 1.0);
            let inverse = 1.0 - mix;
            return (inverse * inverse * previous.1
                + 2.0 * inverse * mix * segment.control
                + mix * mix * segment.gain)
                .max(0.0);
        }
        previous = (segment.end, segment.gain);
    }
    c.gain
}

fn clip_gain_control(from_db: f32, to_db: f32, curve: f32) -> f32 {
    let from = db_to_gain(from_db);
    let to = db_to_gain(to_db);
    let midpoint = db_to_gain((from_db + to_db) * 0.5 + curve.clamp(-1.0, 1.0) * 9.0);
    2.0 * midpoint - 0.5 * (from + to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::types::{
        BusSpec, EffectSpec, InstrumentSpec, LoopSpec, MasterSpec, MidiCcLaneSpec, MidiCcPointSpec,
        MidiClipSpec, MidiNoteSpec, SendSpec, SidechainSpec, TrackSpec, TransportSpec,
    };

    #[test]
    fn clip_gain_curve_control_reaches_the_requested_midpoint() {
        for curve in [-1.0, 1.0] {
            let from_db = -12.0;
            let to_db = -6.0;
            let control = clip_gain_control(from_db, to_db, curve);
            let midpoint = 0.25 * db_to_gain(from_db) + 0.5 * control + 0.25 * db_to_gain(to_db);
            let requested = db_to_gain((from_db + to_db) * 0.5 + curve * 9.0);
            assert!((midpoint - requested).abs() < 1.0e-5);
        }
    }

    fn effect(id: &str, kind: &str, params: &[(&str, f32)]) -> EffectSpec {
        EffectSpec {
            id: id.into(),
            kind: kind.into(),
            bypassed: false,
            plugin: None,
            sidechain: None,
            params: params
                .iter()
                .map(|(key, value)| ((*key).into(), *value))
                .collect(),
        }
    }

    fn midi_snapshot() -> GraphSnapshot {
        GraphSnapshot {
            tracks: vec![TrackSpec {
                id: "instrument-1".into(),
                kind: "instrument".into(),
                name: "Test Tone".into(),
                clips: Vec::new(),
                midi_clips: vec![MidiClipSpec {
                    id: "clip-1".into(),
                    name: "Test Pattern".into(),
                    start_sec: 0.0,
                    duration_sec: 1.0,
                    loop_enabled: false,
                    loop_start_ticks: 0,
                    loop_length_ticks: 1_920,
                    notes: vec![MidiNoteSpec {
                        id: "note-1".into(),
                        pitch: 69,
                        velocity: 127,
                        start_ticks: 120,
                        length_ticks: 480,
                        release_velocity: 64,
                        muted: false,
                    }],
                    cc_lanes: Vec::new(),
                    transpose_semitones: 0,
                    velocity_scale: 1.0,
                    muted: false,
                }],
                instrument: Some(InstrumentSpec {
                    id: "synth-1".into(),
                    kind: "builtin:testtone".into(),
                    plugin: None,
                    params: HashMap::new(),
                    bypassed: false,
                }),
                volume_db: 0.0,
                pan: 0.0,
                muted: false,
                solo: false,
                effects: Vec::new(),
                sends: Vec::new(),
                output_bus_id: None,
            }],
            buses: Vec::new(),
            master: MasterSpec {
                volume_db: 0.0,
                effects: Vec::new(),
            },
            transport: TransportSpec {
                bpm: 120.0,
                tempo_points: Vec::new(),
                time_signatures: Vec::new(),
                playhead_sec: 0.0,
                is_playing: true,
                loop_: LoopSpec {
                    enabled: false,
                    start_sec: 0.0,
                    end_sec: 0.0,
                },
            },
        }
    }

    #[test]
    fn pdc_delays_shorter_track_paths_to_the_longest_native_chain() {
        let mut snapshot = midi_snapshot();
        let mut dry = snapshot.tracks[0].clone();
        dry.id = "instrument-dry".into();
        dry.midi_clips.clear();
        snapshot.tracks[0].effects = vec![effect("limiter", "builtin:mastering-limiter", &[])];
        snapshot.tracks.push(dry);
        let (graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        assert_eq!(graph.max_latency(), 240);
        assert_eq!(graph.tracks[0].pdc.delay, 0);
        assert_eq!(graph.tracks[1].pdc.delay, 240);
    }

    fn render_midi(block_size: usize) -> Vec<f32> {
        render_midi_snapshot(&midi_snapshot(), block_size)
    }

    fn render_midi_snapshot(snapshot: &GraphSnapshot, block_size: usize) -> Vec<f32> {
        let (mut graph, _) = AudioGraph::build(snapshot, &HashMap::new(), 48_000).unwrap();
        let mut rendered = Vec::with_capacity(48_000 * 2);
        let mut position = 0_u64;
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        while position < 48_000 {
            let frames = block_size.min(48_000 - position as usize);
            let mut out = vec![0.0; frames * 2];
            graph.process(position, frames, &mut out, &mut levels, &mut master, true);
            rendered.extend(out);
            position += frames as u64;
        }
        rendered
    }

    fn active_voice_count(graph: &AudioGraph, track: usize) -> u32 {
        let mut counts = [0; MAX_TRACKS];
        graph.write_active_voice_counts(&mut counts);
        counts[track]
    }

    #[test]
    fn inserting_an_effect_preserves_a_held_live_midi_voice() {
        let first = midi_snapshot();
        let mut registry = StableProcessorRegistry::default();
        let (mut old_graph, _) =
            AudioGraph::build_with_plugins(&first, &HashMap::new(), 48_000, &mut registry).unwrap();
        old_graph.push_live_event(
            0,
            NoteEvent {
                sample_offset: 0,
                kind: NoteEventKind::NoteOn {
                    note_id: 77,
                    pitch: 60,
                    velocity: 1.0,
                    tuning_cents: 0.0,
                },
            },
        );
        let mut first_output = vec![0.0; 256 * 2];
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        old_graph.process(0, 256, &mut first_output, &mut levels, &mut master, false);
        assert_eq!(active_voice_count(&old_graph, 0), 1);

        let mut second = first;
        second.tracks[0]
            .effects
            .push(effect("inserted-compressor", "builtin:compressor", &[]));
        let (mut new_graph, _) =
            AudioGraph::build_with_plugins(&second, &HashMap::new(), 48_000, &mut registry)
                .unwrap();
        // Mirrors AudioCommand::SwapGraph: position the replacement, but do
        // not issue the destructive transport reset used by seek/stop.
        new_graph.activate(256);
        drop(old_graph);

        let mut second_output = vec![0.0; 256 * 2];
        new_graph.process(
            256,
            256,
            &mut second_output,
            &mut levels,
            &mut master,
            false,
        );
        assert_eq!(active_voice_count(&new_graph, 0), 1);
        assert!(
            second_output.iter().any(|sample| sample.abs() > 0.001),
            "the held note became silent during the structural graph swap"
        );
    }

    /// Manual regression for the commercial-instrument failure that motivated
    /// the stable registry: keep editor A open, add instrument B through a new
    /// graph, then open B. The A control pointer must survive the rebuild.
    #[test]
    #[ignore = "opens two installed vendor VST3 editors"]
    fn unchanged_vst3_survives_graph_rebuild_while_second_editor_opens() {
        fn external_instrument(
            id: &str,
            path_var: &str,
            uid_var: &str,
        ) -> ministudio_contracts::InstrumentSpec {
            let path = std::env::var(path_var).unwrap_or_else(|_| panic!("{path_var} is required"));
            let uid = std::env::var(uid_var).unwrap_or_else(|_| panic!("{uid_var} is required"));
            ministudio_contracts::InstrumentSpec {
                id: id.into(),
                kind: format!("vst3:{uid}"),
                params: HashMap::new(),
                bypassed: false,
                plugin: Some(ministudio_contracts::ExternalPluginRef {
                    format: "vst3".into(),
                    uid,
                    name: id.into(),
                    vendor: String::new(),
                    path,
                    audio_input_buses: 0,
                    audio_output_buses: 0,
                    supports_sidechain: false,
                    has_editor: true,
                    state: Vec::new(),
                }),
            }
        }

        let mut first = midi_snapshot();
        first.tracks[0].id = "stable-a".into();
        first.tracks[0].instrument = Some(external_instrument(
            "instrument-a",
            "MINISTUDIO_VST3_SMOKE_PATH_A",
            "MINISTUDIO_VST3_SMOKE_UID_A",
        ));
        let mut registry = StableProcessorRegistry::default();
        let (first_graph, first_bindings) =
            AudioGraph::build_with_plugins(&first, &HashMap::new(), 48_000, &mut registry)
                .expect("first VST3 graph should build");
        let first_control = first_bindings.plugin_controls["stable-a"].clone();
        first_control.open_editor().expect("editor A should open");

        let mut second = first.clone();
        let mut track_b = second.tracks[0].clone();
        track_b.id = "new-b".into();
        track_b.instrument = Some(external_instrument(
            "instrument-b",
            "MINISTUDIO_VST3_SMOKE_PATH_B",
            "MINISTUDIO_VST3_SMOKE_UID_B",
        ));
        second.tracks.push(track_b);
        let (second_graph, second_bindings) =
            AudioGraph::build_with_plugins(&second, &HashMap::new(), 48_000, &mut registry)
                .expect("second VST3 graph should build without rebuilding A");
        assert!(Arc::ptr_eq(
            &first_control,
            &second_bindings.plugin_controls["stable-a"]
        ));
        let second_control = second_bindings.plugin_controls["new-b"].clone();
        second_control.open_editor().expect("editor B should open");
        assert!(first_control.is_editor_open());
        assert!(second_control.is_editor_open());
        std::thread::sleep(std::time::Duration::from_millis(750));
        second_control
            .close_editor()
            .expect("editor B should close");
        first_control.close_editor().expect("editor A should close");
        drop(second_graph);
        drop(first_graph);
    }

    #[test]
    fn scheduled_midi_is_audible_and_block_size_invariant() {
        let small = render_midi(32);
        let large = render_midi(1_024);
        assert!(small.iter().any(|sample| sample.abs() > 0.001));
        assert_eq!(small.len(), large.len());
        assert!(small
            .iter()
            .zip(&large)
            .all(|(left, right)| left.to_bits() == right.to_bits()));
    }

    #[test]
    fn variable_tempo_schedules_notes_at_the_analytic_sample() {
        use ministudio_contracts::{TempoCurveSpec, TempoPointSpec};
        let mut snapshot = midi_snapshot();
        snapshot.transport.tempo_points = vec![
            TempoPointSpec {
                tick: 0,
                bpm: 60.0,
                curve: TempoCurveSpec::Linear,
            },
            TempoPointSpec {
                tick: 1_920,
                bpm: 180.0,
                curve: TempoCurveSpec::Jump,
            },
        ];
        snapshot.tracks[0].midi_clips[0].notes[0].start_ticks = 960;
        let expected = TempoMap::from_specs(&snapshot.transport.tempo_points, &[], 48_000)
            .ticks_to_samples(960);
        let (graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let note_on = graph.tracks[0]
            .midi_events
            .iter()
            .find(|event| matches!(event.kind, NoteEventKind::NoteOn { .. }))
            .unwrap();
        assert_eq!(note_on.sample, expected);
        let small = render_midi_snapshot(&snapshot, 32);
        let large = render_midi_snapshot(&snapshot, 1_024);
        assert!(small
            .iter()
            .zip(&large)
            .all(|(left, right)| left.to_bits() == right.to_bits()));
    }

    #[test]
    fn clip_controller_and_pitch_bend_points_are_scheduled() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].cc_lanes = vec![
            MidiCcLaneSpec {
                cc: -1,
                points: vec![MidiCcPointSpec {
                    ticks: 0,
                    value: 4_096,
                }],
            },
            MidiCcLaneSpec {
                cc: 64,
                points: vec![MidiCcPointSpec {
                    ticks: 60,
                    value: 127,
                }],
            },
        ];
        let (graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        assert!(matches!(
            graph.tracks[0].midi_events[0].kind,
            NoteEventKind::PitchBend { value } if (value - 0.5).abs() < 0.0001
        ));
        assert!(graph.tracks[0].midi_events.iter().any(|event| matches!(
            event.kind,
            NoteEventKind::Controller { cc: 64, value } if (value - 1.0).abs() < 0.0001
        )));
    }

    #[test]
    fn transport_all_notes_off_releases_a_sustained_voice() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].notes[0].length_ticks = 20_000;
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![0.0; MAX_BLOCK_SIZE * 2];
        graph.process(
            0,
            MAX_BLOCK_SIZE,
            &mut output,
            &mut levels,
            &mut master,
            true,
        );
        graph.process(
            MAX_BLOCK_SIZE as u64,
            MAX_BLOCK_SIZE,
            &mut output,
            &mut levels,
            &mut master,
            true,
        );
        assert_eq!(active_voice_count(&graph, 0), 1);
        graph.all_notes_off();
        // The synth release parameter is an exponential time constant, so
        // allow enough blocks to fall below its -86 dB idle threshold.
        for _ in 0..600 {
            graph.process(0, 256, &mut output[..512], &mut levels, &mut master, false)
        }
        assert_eq!(active_voice_count(&graph, 0), 0)
    }

    #[test]
    fn looping_midi_keeps_producing_audio_beyond_the_first_second() {
        let mut snapshot = midi_snapshot();
        let clip = &mut snapshot.tracks[0].midi_clips[0];
        clip.duration_sec = 4.0;
        clip.loop_enabled = true;
        clip.loop_length_ticks = 960;
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let mut energy = [0.0_f32; 4];
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut position = 0_usize;
        while position < 48_000 * 4 {
            let frames = 256.min(48_000 * 4 - position);
            let mut out = vec![0.0; frames * 2];
            graph.process(
                position as u64,
                frames,
                &mut out,
                &mut levels,
                &mut master,
                true,
            );
            for (index, sample) in out.into_iter().enumerate() {
                let timeline_sample = position + index / 2;
                energy[(timeline_sample / 48_000).min(3)] += sample.abs();
            }
            position += frames;
        }
        assert!(
            energy.iter().all(|value| *value > 1.0),
            "audio energy ran out: {energy:?}"
        );
    }

    #[test]
    fn demo_style_fx_chain_stays_finite_and_audible() {
        let mut snapshot = midi_snapshot();
        let clip = &mut snapshot.tracks[0].midi_clips[0];
        clip.duration_sec = 4.0;
        clip.loop_enabled = true;
        clip.loop_length_ticks = 960;
        snapshot.tracks[0].sends = vec![
            SendSpec {
                id: "send-a".into(),
                target_bus_id: "bus-a".into(),
                gain_db: -18.0,
                pre_fader: false,
            },
            SendSpec {
                id: "send-b".into(),
                target_bus_id: "bus-b".into(),
                gain_db: -24.0,
                pre_fader: false,
            },
        ];
        snapshot.buses = vec![
            BusSpec {
                id: "bus-a".into(),
                name: "Space".into(),
                effects: vec![effect(
                    "reverb",
                    "builtin:reverb",
                    &[
                        ("decaySec", 2.8),
                        ("damping", 0.42),
                        ("width", 0.85),
                        ("mix", 0.3),
                    ],
                )],
                volume_db: -5.0,
            },
            BusSpec {
                id: "bus-b".into(),
                name: "Echo".into(),
                effects: vec![effect(
                    "delay",
                    "builtin:delay",
                    &[("time", 0.375), ("feedback", 0.42), ("mix", 0.3)],
                )],
                volume_db: -7.0,
            },
        ];
        snapshot.master.effects = vec![
            effect(
                "master-eq",
                "builtin:eq",
                &[("lowGain", 0.5), ("midGain", -0.8), ("midFreq", 340.0)],
            ),
            effect(
                "master-comp",
                "builtin:compressor",
                &[
                    ("threshold", -8.0),
                    ("ratio", 2.0),
                    ("attack", 0.03),
                    ("release", 0.25),
                ],
            ),
            effect(
                "master-shaper",
                "builtin:waveshaper",
                &[("driveDb", 3.0), ("mix", 0.18), ("oversample", 4.0)],
            ),
        ];

        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut energy = [0.0_f32; 4];
        for position in (0..48_000 * 4).step_by(256) {
            let frames = 256.min(48_000 * 4 - position);
            let mut out = vec![0.0; frames * 2];
            graph.process(
                position as u64,
                frames,
                &mut out,
                &mut levels,
                &mut master,
                true,
            );
            assert!(out.iter().all(|sample| sample.is_finite()));
            for (index, sample) in out.into_iter().enumerate() {
                energy[((position + index / 2) / 48_000).min(3)] += sample.abs();
            }
        }
        assert!(
            energy.iter().all(|value| *value > 1.0),
            "FX chain became silent: {energy:?}"
        );
    }

    #[test]
    fn track_and_return_sidechains_resolve_to_distinct_taps() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].notes[0].start_ticks = 0;
        snapshot.tracks[0].sends = vec![SendSpec {
            id: "send-sidechain-return".into(),
            target_bus_id: "sidechain-return".into(),
            gain_db: 0.0,
            pre_fader: false,
        }];
        let mut return_compressor = effect(
            "return-compressor",
            "builtin:compressor",
            &[("threshold", -30.0), ("ratio", 4.0)],
        );
        return_compressor.sidechain = Some(SidechainSpec {
            enabled: true,
            source_track_id: Some("instrument-1".into()),
        });
        snapshot.buses = vec![BusSpec {
            id: "sidechain-return".into(),
            name: "Sidechain Return".into(),
            effects: vec![return_compressor],
            volume_db: 0.0,
        }];
        let mut master_compressor = effect(
            "master-sidechain",
            "builtin:compressor",
            &[("threshold", -30.0), ("ratio", 4.0)],
        );
        master_compressor.sidechain = Some(SidechainSpec {
            enabled: true,
            source_track_id: Some("bus:sidechain-return".into()),
        });
        snapshot.master.effects = vec![master_compressor];

        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        assert!(matches!(
            graph.buses[0].effect_sidechains[0],
            Some(SidechainSource::Track(0))
        ));
        assert!(matches!(
            graph.master_effect_sidechains[0],
            Some(SidechainSource::Bus(0))
        ));

        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![0.0; 512];
        graph.process(0, 256, &mut output, &mut levels, &mut master, true);
        assert!(graph.sidechain_taps[0].channels[0][..256]
            .iter()
            .any(|sample| sample.abs() > 0.0001));
        assert!(graph.bus_sidechain_taps[0].channels[0][..256]
            .iter()
            .any(|sample| sample.abs() > 0.0001));
    }

    #[test]
    fn track_main_output_can_route_exclusively_through_a_bus() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].notes[0].start_ticks = 0;
        snapshot.tracks[0].output_bus_id = Some("group".into());
        snapshot.buses.push(BusSpec {
            id: "group".into(),
            name: "Group".into(),
            volume_db: -144.0,
            effects: Vec::new(),
        });
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        assert_eq!(graph.tracks[0].output_bus, Some(0));
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![0.0; 512];
        graph.process(0, 256, &mut output, &mut levels, &mut master, true);
        assert!(graph.sidechain_taps[0].channels[0][..256]
            .iter()
            .any(|sample| sample.abs() > 0.0001));
        assert!(output.iter().all(|sample| sample.abs() < 0.0001));
    }

    #[test]
    fn mass_native_no_tail_nodes_sleep_without_leaving_garbage() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].instrument = None;
        snapshot.tracks[0].midi_clips.clear();
        snapshot.tracks[0].effects = (0..500)
            .map(|index| effect(&format!("utility-{index}"), "builtin:utility", &[]))
            .collect();
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![1.0; 512];
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        let metrics = graph.scheduler_snapshot();
        assert_eq!(metrics.total_nodes, 500);
        assert_eq!(metrics.sleeping_nodes, 500);
        assert!(metrics.skipped_process_calls >= 500);
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn scheduler_on_and_off_render_identically_for_safe_native_chain() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].notes[0].start_ticks = 0;
        snapshot.tracks[0].effects = vec![
            effect("utility", "builtin:utility", &[]),
            effect("clipper", "builtin:clipper", &[("inputDb", 0.0)]),
        ];
        let (mut enabled, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let (mut disabled, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        disabled.set_scheduler_enabled(false);
        let mut enabled_out = vec![0.0_f32; 8_192];
        let mut disabled_out = vec![0.0_f32; 8_192];
        let mut enabled_levels = [Level::default(); MAX_TRACKS];
        let mut disabled_levels = [Level::default(); MAX_TRACKS];
        let mut enabled_master = Level::default();
        let mut disabled_master = Level::default();
        for position in (0_usize..4_096).step_by(256) {
            enabled.process(
                position as u64,
                256,
                &mut enabled_out[position * 2..position * 2 + 512],
                &mut enabled_levels,
                &mut enabled_master,
                true,
            );
            disabled.process(
                position as u64,
                256,
                &mut disabled_out[position * 2..position * 2 + 512],
                &mut disabled_levels,
                &mut disabled_master,
                true,
            );
        }
        assert!(enabled_out
            .iter()
            .zip(&disabled_out)
            .all(|(left, right)| left.to_bits() == right.to_bits()));
    }

    #[test]
    fn sleeping_never_changes_pdc_and_seek_wakes_nodes() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].instrument = None;
        snapshot.tracks[0].midi_clips.clear();
        snapshot.tracks[0].effects = vec![effect("clipper", "builtin:clipper", &[])];
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let latency = graph.max_latency();
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![0.0; 512];
        for _ in 0..4 {
            graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        }
        assert_eq!(graph.scheduler_snapshot().sleeping_nodes, 1);
        assert_eq!(graph.max_latency(), latency);
        graph.reset(0);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        assert_eq!(graph.scheduler_snapshot().running_nodes, 1);
        assert_eq!(graph.max_latency(), latency);
    }

    #[test]
    fn parameter_automation_and_modulation_wake_a_sleeping_node() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].instrument = None;
        snapshot.tracks[0].midi_clips.clear();
        snapshot.tracks[0].effects = vec![effect("utility", "builtin:utility", &[])];
        let (mut graph, bindings) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let target = bindings.effects["utility"];
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![0.0; 512];
        for _ in 0..3 {
            graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        }
        assert_eq!(graph.scheduler_snapshot().sleeping_nodes, 1);
        graph.set_effect(target, "gainDb", -3.0);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        assert_eq!(graph.scheduler_snapshot().running_nodes, 1);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        graph.wake_all(WakeReason::Automation);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        assert_eq!(graph.scheduler_snapshot().running_nodes, 1);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        graph.wake_all(WakeReason::Modulation);
        graph.process(0, 256, &mut output, &mut levels, &mut master, false);
        assert_eq!(graph.scheduler_snapshot().running_nodes, 1);
    }

    #[test]
    fn return_reverb_tail_survives_after_the_source_becomes_quiet() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].notes[0].start_ticks = 0;
        snapshot.tracks[0].midi_clips[0].notes[0].length_ticks = 120;
        snapshot.tracks[0].sends = vec![SendSpec {
            id: "tail-send".into(),
            target_bus_id: "tail-return".into(),
            gain_db: 0.0,
            pre_fader: false,
        }];
        snapshot.buses = vec![BusSpec {
            id: "tail-return".into(),
            name: "Tail Return".into(),
            effects: vec![effect(
                "tail-reverb",
                "builtin:reverb",
                &[("decaySec", 2.5), ("mix", 1.0)],
            )],
            volume_db: 0.0,
        }];
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000).unwrap();
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        let mut output = vec![0.0; 512];
        let mut late_energy = 0.0_f32;
        for position in (0..72_000).step_by(256) {
            graph.process(
                position as u64,
                256,
                &mut output,
                &mut levels,
                &mut master,
                true,
            );
            if position >= 48_000 {
                late_energy += output.iter().map(|sample| sample.abs()).sum::<f32>();
            }
        }
        assert!(
            late_energy > 0.01,
            "return tail was truncated: {late_energy}"
        );
    }
}
