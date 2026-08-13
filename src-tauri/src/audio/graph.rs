// Sample-accurate clip, routing, effect-chain, metering, and PDC graph.
use super::{
    asset::AudioAsset,
    command::{ChainKind, EffectRef},
    dsp::{create_effect, AudioBuffer, DspEffect, Smoother, DISTORTION_SPECTRUM_BINS},
    instrument::{create_instrument, Instrument, NoteEvent, NoteEventKind, TempoMap},
    types::{db_to_gain, sec_to_samples, GraphSnapshot, Level},
    MAX_BLOCK_SIZE, MAX_CHANNELS, MAX_EFFECT_METERS, MAX_TRACKS,
};
use std::{collections::HashMap, sync::Arc};

fn effect_has_spectrum(kind: &str) -> bool {
    matches!(
        kind,
        "builtin:eq" | "builtin:eq8" | "builtin:distortion" | "builtin:disperser"
    )
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
    pub track_ids: Vec<String>,
    pub multiband_meter_ids: Vec<String>,
    pub distortion_meter_ids: Vec<String>,
}
struct ClipEvent {
    asset: Arc<AudioAsset>,
    start: u64,
    end: u64,
    offset: u64,
    gain: f32,
    fade_in: u64,
    fade_out: u64,
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum SidechainSource {
    Track(usize),
    Bus(usize),
}
struct DelayComp {
    data: [Vec<f32>; MAX_CHANNELS],
    index: usize,
    delay: usize,
}
impl DelayComp {
    fn new(delay: usize) -> Self {
        let len = delay.max(1) + MAX_BLOCK_SIZE;
        Self {
            data: [vec![0.0; len], vec![0.0; len]],
            index: 0,
            delay,
        }
    }
    fn process(&mut self, b: &mut AudioBuffer, n: usize) {
        if self.delay == 0 {
            return;
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
    }
}
struct TrackNode {
    clips: Vec<ClipEvent>,
    cursor: usize,
    midi_events: Vec<ScheduledNoteEvent>,
    midi_cursor: usize,
    live_events: Vec<NoteEvent>,
    event_buffer: Vec<NoteEvent>,
    instrument: Option<Box<dyn Instrument>>,
    effects: Vec<Box<dyn DspEffect>>,
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
    effects: Vec<Box<dyn DspEffect>>,
    effect_sidechains: Vec<Option<SidechainSource>>,
    buffer: AudioBuffer,
    gain: Smoother,
}
pub struct AudioGraph {
    sample_rate: u32,
    tracks: Vec<TrackNode>,
    buses: Vec<BusNode>,
    master_effects: Vec<Box<dyn DspEffect>>,
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
}

impl AudioGraph {
    pub fn build(
        spec: &GraphSnapshot,
        assets: &HashMap<String, Arc<AudioAsset>>,
        sample_rate: u32,
    ) -> (Box<Self>, Bindings) {
        let sr = sample_rate as f32;
        let mut effects = HashMap::new();
        let mut tracks_map = HashMap::new();
        let mut sends_map = HashMap::new();
        let mut buses_map = HashMap::new();
        let mut track_ids = Vec::new();
        let mut multiband_meter_ids = Vec::new();
        let mut distortion_meter_ids = Vec::new();
        let mut latencies = Vec::new();
        let mut tracks = Vec::new();
        let tempo = TempoMap::new(spec.transport.bpm, sample_rate);
        let mut next_note_id = 1_i32;
        for (ts_idx, ts) in spec.tracks.iter().take(MAX_TRACKS).enumerate() {
            tracks_map.insert(ts.id.clone(), ts_idx);
            track_ids.push(ts.id.clone());
            let mut chain = Vec::new();
            let mut effect_sidechains = Vec::new();
            for (es_idx, es) in ts.effects.iter().enumerate() {
                if let Some(fx) = create_effect(es, sr) {
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
                    effects.insert(
                        es.id.clone(),
                        EffectRef {
                            chain: ChainKind::Track,
                            owner: ts_idx,
                            effect: chain.len(),
                        },
                    );
                    let _ = es_idx;
                    chain.push(fx)
                }
            }
            let latency = chain.iter().map(|v| v.latency_samples()).sum();
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
                    })
                }
            }
            clips.sort_by_key(|v| v.start);
            let mut midi_events = Vec::new();
            for clip in ts.midi_clips.iter().filter(|clip| !clip.muted) {
                let clip_start = sec_to_samples(clip.start_sec, sample_rate);
                let clip_length = sec_to_samples(clip.duration_sec, sample_rate);
                let loop_length = clip.loop_length_ticks.max(1);
                for note in clip.notes.iter().filter(|note| !note.muted) {
                    let mut cycle = 0_u64;
                    loop {
                        let tick = if clip.loop_enabled {
                            note.start_ticks.saturating_sub(clip.loop_start_ticks)
                                + cycle * loop_length
                        } else {
                            note.start_ticks
                        };
                        let relative = tempo.ticks_to_samples(tick);
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
                        let off =
                            (relative + tempo.ticks_to_samples(note.length_ticks)).min(clip_length);
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
            let instrument = ts
                .instrument
                .as_ref()
                .and_then(|spec| create_instrument(spec, sr));
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
                if let Some(fx) = create_effect(es, sr) {
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
                    effects.insert(
                        es.id.clone(),
                        EffectRef {
                            chain: ChainKind::Bus,
                            owner: bs_idx,
                            effect: chain.len(),
                        },
                    );
                    chain.push(fx)
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
            if let Some(fx) = create_effect(es, sr) {
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
                effects.insert(
                    es.id.clone(),
                    EffectRef {
                        chain: ChainKind::Master,
                        owner: 0,
                        effect: master_effects.len(),
                    },
                );
                master_effects.push(fx)
            }
        }
        let track_count = tracks.len();
        let bus_count = buses.len();
        (
            Box::new(Self {
                sample_rate,
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
            }),
            Bindings {
                tracks: tracks_map,
                buses: buses_map,
                sends: sends_map,
                effects,
                track_ids,
                multiband_meter_ids,
                distortion_meter_ids,
            },
        )
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
                playhead_sec: 0.0,
                is_playing: false,
                loop_: super::types::LoopSpec {
                    enabled: false,
                    start_sec: 0.0,
                    end_sec: 0.0,
                },
            },
        };
        Self::build(&s, &HashMap::new(), sample_rate).0
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
        let sidechain_taps = &self.sidechain_taps;
        let bus_sidechain_taps = &self.bus_sidechain_taps;
        for (index, track) in self.tracks.iter_mut().enumerate() {
            track.buffer.clear(frames);
            if timeline {
                while track.cursor < track.clips.len() && track.clips[track.cursor].end <= position
                {
                    track.cursor += 1
                }
                for clip in track.clips.iter().skip(track.cursor) {
                    if clip.start >= position + frames as u64 {
                        break;
                    }
                    render_clip(clip, position, frames, &mut track.buffer)
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
                track
                    .event_buffer
                    .extend(track.live_events.drain(..).take(128));
                track.event_buffer.sort_by_key(|event| event.sample_offset);
                instrument.process(&track.event_buffer, &mut track.buffer, frames);
            } else {
                track.live_events.clear();
            }
            for (effect_index, fx) in track.effects.iter_mut().enumerate() {
                let sidechain = track
                    .effect_sidechains
                    .get(effect_index)
                    .copied()
                    .flatten()
                    .and_then(|source| {
                        sidechain_from_taps(source, sidechain_taps, bus_sidechain_taps)
                    });
                fx.process_with_sidechain(&mut track.buffer, sidechain, frames)
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
            track.pdc.process(&mut track.buffer, frames);
            for channel in 0..MAX_CHANNELS {
                self.next_sidechain_taps[index].channels[channel][..frames]
                    .copy_from_slice(&track.buffer.channels[channel][..frames])
            }
            if let Some(bus) = track.output_bus.and_then(|index| self.buses.get_mut(index)) {
                for ch in 0..2 {
                    for i in 0..frames {
                        bus.buffer.channels[ch][i] += track.buffer.channels[ch][i]
                    }
                }
            } else {
                for ch in 0..2 {
                    for i in 0..frames {
                        self.master_buffer.channels[ch][i] += track.buffer.channels[ch][i]
                    }
                }
            }
            if let Some(m) = levels.get_mut(index) {
                m.peak = peak;
                m.rms = (sum / (frames.max(1) * 2) as f32).sqrt()
            }
        }
        for (bus_index, bus) in self.buses.iter_mut().enumerate() {
            for (effect_index, fx) in bus.effects.iter_mut().enumerate() {
                let sidechain = bus
                    .effect_sidechains
                    .get(effect_index)
                    .copied()
                    .flatten()
                    .and_then(|source| {
                        sidechain_from_taps(source, sidechain_taps, bus_sidechain_taps)
                    });
                fx.process_with_sidechain(&mut bus.buffer, sidechain, frames)
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
        }
        for (effect_index, fx) in self.master_effects.iter_mut().enumerate() {
            let sidechain = self
                .master_effect_sidechains
                .get(effect_index)
                .copied()
                .flatten()
                .and_then(|source| sidechain_from_taps(source, sidechain_taps, bus_sidechain_taps));
            fx.process_with_sidechain(&mut self.master_buffer, sidechain, frames)
        }
        let (mut peak, mut sum) = (0.0_f32, 0.0_f32);
        for (i, frame) in out.chunks_exact_mut(2).take(frames).enumerate() {
            let g = self.master_gain.next();
            let l = self.master_buffer.channels[0][i] * g;
            let r = self.master_buffer.channels[1][i] * g;
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
        )
    }
    pub fn reset(&mut self, position: u64) {
        for tap in self
            .sidechain_taps
            .iter_mut()
            .chain(self.next_sidechain_taps.iter_mut())
        {
            tap.clear(MAX_BLOCK_SIZE)
        }
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
            if let Some(instrument) = &mut t.instrument {
                instrument.reset();
            }
            for fx in &mut t.effects {
                fx.reset()
            }
        }
        for b in &mut self.buses {
            for fx in &mut b.effects {
                fx.reset()
            }
        }
        for fx in &mut self.master_effects {
            fx.reset()
        }
    }
    pub fn set_effect(&mut self, target: EffectRef, param: &str, value: f32) {
        let fx: Option<&mut Box<dyn DspEffect>> = match target.chain {
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
            if let Some(levels) = effect.multiband_levels() {
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
            if let Some(spectrum) = effect.effect_spectrum() {
                if index >= MAX_EFFECT_METERS {
                    break;
                }
                output[index] = spectrum;
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
    pub fn active_voice_counts(&self) -> Vec<u32> {
        self.tracks
            .iter()
            .map(|track| {
                track
                    .instrument
                    .as_ref()
                    .map_or(0, |instrument| instrument.active_voice_count() as u32)
            })
            .collect()
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
            .map(|effect| effect.tail_samples())
            .chain(self.tracks.iter().filter_map(|track| {
                track
                    .instrument
                    .as_ref()
                    .map(|instrument| instrument.tail_samples())
            }))
            .max()
            .unwrap_or(0)
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
fn render_clip(c: &ClipEvent, pos: u64, n: usize, b: &mut AudioBuffer) {
    let from = pos.max(c.start);
    let to = (pos + n as u64).min(c.end);
    for timeline in from..to {
        let dst = (timeline - pos) as usize;
        let src = (c.offset + timeline - c.start) as usize;
        if src >= c.asset.frames {
            break;
        }
        let rel = timeline - c.start;
        let remaining = c.end - timeline;
        let fade_in = if c.fade_in > 0 {
            ((rel as f32 / c.fade_in as f32).min(1.0) * std::f32::consts::FRAC_PI_2).sin()
        } else {
            1.0
        };
        let fade_out = if c.fade_out > 0 {
            ((remaining as f32 / c.fade_out as f32).min(1.0) * std::f32::consts::FRAC_PI_2).sin()
        } else {
            1.0
        };
        for ch in 0..2 {
            let source = c
                .asset
                .channels
                .get(ch)
                .or_else(|| c.asset.channels.first())
                .and_then(|v| v.get(src))
                .copied()
                .unwrap_or(0.0);
            b.channels[ch][dst] += source * c.gain * fade_in * fade_out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::types::{
        BusSpec, EffectSpec, InstrumentSpec, LoopSpec, MasterSpec, MidiClipSpec, MidiNoteSpec,
        SendSpec, SidechainSpec, TrackSpec, TransportSpec,
    };

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

    fn render_midi(block_size: usize) -> Vec<f32> {
        let (mut graph, _) = AudioGraph::build(&midi_snapshot(), &HashMap::new(), 48_000);
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
    fn transport_all_notes_off_releases_a_sustained_voice() {
        let mut snapshot = midi_snapshot();
        snapshot.tracks[0].midi_clips[0].notes[0].length_ticks = 20_000;
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000);
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
        assert_eq!(graph.active_voice_counts()[0], 1);
        graph.all_notes_off();
        // The synth release parameter is an exponential time constant, so
        // allow enough blocks to fall below its -86 dB idle threshold.
        for _ in 0..600 {
            graph.process(0, 256, &mut output[..512], &mut levels, &mut master, false)
        }
        assert_eq!(graph.active_voice_counts()[0], 0)
    }

    #[test]
    fn looping_midi_keeps_producing_audio_beyond_the_first_second() {
        let mut snapshot = midi_snapshot();
        let clip = &mut snapshot.tracks[0].midi_clips[0];
        clip.duration_sec = 4.0;
        clip.loop_enabled = true;
        clip.loop_length_ticks = 960;
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000);
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

        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000);
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

        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000);
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
        let (mut graph, _) = AudioGraph::build(&snapshot, &HashMap::new(), 48_000);
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
}
