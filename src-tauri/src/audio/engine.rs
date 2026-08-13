// Control-plane ownership and allocation-free CPAL callback core.
use super::{
    asset::{decode_asset, AudioAsset},
    command::{param_key, AudioCommand},
    device,
    dsp::{create_effect, DISTORTION_SPECTRUM_BINS},
    graph::{AudioGraph, Bindings},
    instrument::{LiveMidiMessage, NoteEvent, NoteEventKind},
    types::{
        db_to_gain, AudioBackendInfo, AudioDeviceInfo, AudioSettings, DecodeProgress, EffectSpec,
        EngineSnapshot, EqFrequencyResponse, ExportProgress, ExportRequest, ExportResult,
        GraphSnapshot, Level, MidiInputPortInfo, MultibandLevels, NativeAssetInfo, StereoLevel,
        StreamStatus,
    },
    COMMAND_CAPACITY, MAX_BLOCK_SIZE, MAX_EFFECT_METERS, MAX_TRACKS, RETIRED_GRAPH_CAPACITY,
};
use cpal::Stream;
use crossbeam_queue::ArrayQueue;
use midir::{Ignore, MidiInput, MidiInputConnection};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        Arc,
    },
};
use triple_buffer::{triple_buffer, Input, Output};

#[derive(Clone, Copy)]
pub struct MeterFrame {
    pub position: u64,
    pub levels: [Level; MAX_TRACKS],
    pub master: Level,
    pub pdc: usize,
    pub playing: bool,
    pub active_voice_counts: [u32; MAX_TRACKS],
    pub multiband_levels: [[[f32; 2]; 3]; MAX_EFFECT_METERS],
    pub distortion_spectra: [[f32; DISTORTION_SPECTRUM_BINS]; MAX_EFFECT_METERS],
}
impl Default for MeterFrame {
    fn default() -> Self {
        Self {
            position: 0,
            levels: [Level {
                peak: 0.0,
                rms: 0.0,
            }; MAX_TRACKS],
            master: Level::default(),
            pdc: 0,
            playing: false,
            active_voice_counts: [0; MAX_TRACKS],
            multiband_levels: [[[0.0; 2]; 3]; MAX_EFFECT_METERS],
            distortion_spectra: [[0.0; DISTORTION_SPECTRUM_BINS]; MAX_EFFECT_METERS],
        }
    }
}

pub struct AudioCore {
    commands: Consumer<AudioCommand>,
    retired: Producer<Box<AudioGraph>>,
    graph: Box<AudioGraph>,
    meters: Input<MeterFrame>,
    position: u64,
    playing: bool,
    placeholder: bool,
    loop_tail: [f32; 2048],
    loop_tail_frames: usize,
    midi_input: Arc<ArrayQueue<LiveMidiMessage>>,
}
impl AudioCore {
    #[cfg(test)]
    pub fn placeholder() -> Self {
        let (_, rx): (Producer<AudioCommand>, Consumer<AudioCommand>) = RingBuffer::new(1);
        let (tx, _): (Producer<Box<AudioGraph>>, Consumer<Box<AudioGraph>>) = RingBuffer::new(1);
        let (input, _) = triple_buffer(&MeterFrame::default());
        Self {
            commands: rx,
            retired: tx,
            graph: AudioGraph::empty(48000),
            meters: input,
            position: 0,
            playing: false,
            placeholder: true,
            loop_tail: [0.0; 2048],
            loop_tail_frames: 0,
            midi_input: Arc::new(ArrayQueue::new(1)),
        }
    }
    fn new(
        commands: Consumer<AudioCommand>,
        retired: Producer<Box<AudioGraph>>,
        graph: Box<AudioGraph>,
        meters: Input<MeterFrame>,
        midi_input: Arc<ArrayQueue<LiveMidiMessage>>,
    ) -> Self {
        Self {
            commands,
            retired,
            graph,
            meters,
            position: 0,
            playing: false,
            placeholder: false,
            loop_tail: [0.0; 2048],
            loop_tail_frames: 0,
            midi_input,
        }
    }
    pub fn render(&mut self, output: &mut [f32], frames: usize) {
        if self.placeholder {
            output.fill(0.0);
            return;
        }
        self.drain_commands();
        if !self.playing {
            let mut levels = [Level::default(); MAX_TRACKS];
            let mut master = Level::default();
            self.graph.process(
                self.position,
                frames,
                output,
                &mut levels,
                &mut master,
                false,
            );
            let frame = self.meters.input_buffer_mut();
            frame.levels = levels;
            frame.master = master;
            self.publish();
            return;
        }
        let mut done = 0;
        while done < frames {
            let remaining = frames - done;
            let loop_range = self.graph.loop_range();
            if let Some((start, end)) = loop_range {
                if self.position >= end {
                    self.position = start;
                    self.graph.reset(start)
                }
            }
            let segment = if let Some((_, end)) = loop_range {
                remaining.min((end - self.position) as usize)
            } else {
                remaining
            };
            let from = done * 2;
            let to = (done + segment) * 2;
            let mut levels = [Level::default(); MAX_TRACKS];
            let mut master = Level::default();
            self.graph.process(
                self.position,
                segment,
                &mut output[from..to],
                &mut levels,
                &mut master,
                true,
            );
            if self.loop_tail_frames > 0 {
                let blend = segment.min(self.loop_tail_frames);
                for i in 0..blend {
                    let t = (i + 1) as f32 / blend as f32;
                    for ch in 0..2 {
                        output[from + i * 2 + ch] =
                            self.loop_tail[i * 2 + ch] * (1.0 - t) + output[from + i * 2 + ch] * t
                    }
                }
                self.loop_tail_frames = 0
            }
            self.position += segment as u64;
            if let Some((_, end)) = loop_range {
                if self.position >= end {
                    let crossfade = ((self.graph.sample_rate() as f32 * 0.003) as usize)
                        .min(1024)
                        .min(segment);
                    self.loop_tail_frames = crossfade;
                    let source = to - crossfade * 2;
                    self.loop_tail[..crossfade * 2].copy_from_slice(&output[source..to])
                }
            }
            done += segment;
            let frame = self.meters.input_buffer_mut();
            frame.position = self.position;
            frame.levels = levels;
            frame.master = master;
            frame.pdc = self.graph.max_latency();
            frame.playing = true;
            for (index, count) in self
                .graph
                .active_voice_counts()
                .into_iter()
                .take(MAX_TRACKS)
                .enumerate()
            {
                frame.active_voice_counts[index] = count;
            }
            self.graph
                .write_multiband_levels(&mut frame.multiband_levels);
            self.graph
                .write_distortion_spectra(&mut frame.distortion_spectra);
            self.meters.publish();
        }
    }
    fn drain_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                AudioCommand::SetPlaying(v) => {
                    if self.playing && !v {
                        self.graph.all_notes_off()
                    }
                    self.playing = v
                }
                AudioCommand::SeekTo(v) => {
                    self.position = v;
                    self.graph.reset(v)
                }
                AudioCommand::Stop => {
                    self.playing = false;
                    self.position = 0;
                    self.graph.reset(0)
                }
                AudioCommand::SetTrackGain { track, gain } => {
                    if let Some((g, _, _, _)) = self.graph.track_mut(track) {
                        g.set_target(gain)
                    }
                }
                AudioCommand::SetTrackPan { track, pan } => {
                    if let Some((_, p, _, _)) = self.graph.track_mut(track) {
                        p.set_target(pan)
                    }
                }
                AudioCommand::SetTrackMute { track, muted } => {
                    if let Some((_, _, m, _)) = self.graph.track_mut(track) {
                        *m = muted
                    }
                }
                AudioCommand::SetTrackSolo { track, solo } => {
                    if let Some((_, _, _, s)) = self.graph.track_mut(track) {
                        *s = solo
                    }
                }
                AudioCommand::SetSendGain { track, send, gain } => {
                    if let Some(v) = self.graph.send_gain(track, send) {
                        v.set_target(gain)
                    }
                }
                AudioCommand::SetBusGain { bus, gain } => {
                    if let Some(value) = self.graph.bus_gain(bus) {
                        value.set_target(gain)
                    }
                }
                AudioCommand::SetMasterGain { gain } => self.graph.master_gain().set_target(gain),
                AudioCommand::SetEffectParam {
                    target,
                    param,
                    len,
                    value,
                } => {
                    if let Ok(key) = std::str::from_utf8(&param[..len]) {
                        self.graph.set_effect(target, key, value)
                    }
                }
                AudioCommand::SetInstrumentParam {
                    track,
                    param,
                    len,
                    value,
                } => {
                    if let Ok(key) = std::str::from_utf8(&param[..len]) {
                        self.graph.set_instrument_param(track, key, value)
                    }
                }
                AudioCommand::LiveMidi { track, event } => self.graph.push_live_event(track, event),
                AudioCommand::SwapGraph(new_graph) => {
                    if self.retired.is_full() {
                        std::mem::forget(new_graph)
                    } else {
                        let old = std::mem::replace(&mut self.graph, new_graph);
                        let _ = self.retired.push(old);
                        self.graph.reset(self.position)
                    }
                }
            }
        }
        while let Some(message) = self.midi_input.pop() {
            self.graph.push_live_event(message.track, message.event);
        }
    }
    fn publish(&mut self) {
        let frame = self.meters.input_buffer_mut();
        frame.position = self.position;
        frame.playing = self.playing;
        frame.pdc = self.graph.max_latency();
        for (index, count) in self
            .graph
            .active_voice_counts()
            .into_iter()
            .take(MAX_TRACKS)
            .enumerate()
        {
            frame.active_voice_counts[index] = count;
        }
        self.graph
            .write_multiband_levels(&mut frame.multiband_levels);
        self.graph
            .write_distortion_spectra(&mut frame.distortion_spectra);
        self.meters.publish();
    }
}

struct Runtime {
    _stream: Stream,
    commands: Producer<AudioCommand>,
    retired: Consumer<Box<AudioGraph>>,
    meters: Output<MeterFrame>,
    xruns: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
}
pub struct NativeEngine {
    settings: AudioSettings,
    runtime: Option<Runtime>,
    assets: HashMap<String, Arc<AudioAsset>>,
    bindings: Bindings,
    last_snapshot: Option<GraphSnapshot>,
    last_error: Option<String>,
    graph_revision: u64,
    midi_input: Arc<ArrayQueue<LiveMidiMessage>>,
    midi_connections: HashMap<String, MidiInputConnection<()>>,
    midi_note_counter: Arc<AtomicI32>,
}
impl Default for NativeEngine {
    fn default() -> Self {
        Self {
            settings: AudioSettings::default(),
            runtime: None,
            assets: HashMap::new(),
            bindings: Bindings {
                tracks: HashMap::new(),
                buses: HashMap::new(),
                sends: HashMap::new(),
                effects: HashMap::new(),
                track_ids: Vec::new(),
                multiband_meter_ids: Vec::new(),
                distortion_meter_ids: Vec::new(),
            },
            last_snapshot: None,
            last_error: None,
            graph_revision: 0,
            midi_input: Arc::new(ArrayQueue::new(2048)),
            midi_connections: HashMap::new(),
            midi_note_counter: Arc::new(AtomicI32::new(1)),
        }
    }
}
impl NativeEngine {
    pub fn init(&mut self) -> Result<(), String> {
        if self.runtime.is_some() {
            return Ok(());
        }
        self.restart_stream()
    }
    pub fn dispose(&mut self) {
        self.runtime = None;
        self.midi_connections.clear()
    }
    fn restart_stream(&mut self) -> Result<(), String> {
        let resume = self.runtime.as_mut().map(|runtime| {
            runtime.meters.update();
            *runtime.meters.output_buffer_mut()
        });
        let previous_xruns = self
            .runtime
            .as_ref()
            .map_or(0, |runtime| runtime.xruns.load(Ordering::Relaxed));
        self.runtime = None;
        let graph = if let Some(spec) = &self.last_snapshot {
            let (g, b) = AudioGraph::build(spec, &self.assets, self.settings.sample_rate);
            self.bindings = b;
            g
        } else {
            AudioGraph::empty(self.settings.sample_rate)
        };
        let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (retired_tx, retired_rx) = RingBuffer::new(RETIRED_GRAPH_CAPACITY);
        let (meter_tx, meter_rx) = triple_buffer(&MeterFrame::default());
        let core = AudioCore::new(
            command_rx,
            retired_tx,
            graph,
            meter_tx,
            Arc::clone(&self.midi_input),
        );
        let xruns = Arc::new(AtomicU64::new(previous_xruns));
        let failed = Arc::new(AtomicBool::new(false));
        match device::open_stream(
            &self.settings,
            core,
            Arc::clone(&xruns),
            Arc::clone(&failed),
        ) {
            Ok(stream) => {
                self.runtime = Some(Runtime {
                    _stream: stream,
                    commands: command_tx,
                    retired: retired_rx,
                    meters: meter_rx,
                    xruns,
                    failed,
                });
                self.last_error = None;
                self.restore_transport(resume);
                Ok(())
            }
            Err(first) => {
                self.settings.backend_id = "default".into();
                self.settings.device_id = "default".into();
                let graph = if let Some(spec) = &self.last_snapshot {
                    let (graph, bindings) =
                        AudioGraph::build(spec, &self.assets, self.settings.sample_rate);
                    self.bindings = bindings;
                    graph
                } else {
                    AudioGraph::empty(self.settings.sample_rate)
                };
                let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
                let (retired_tx, retired_rx) = RingBuffer::new(RETIRED_GRAPH_CAPACITY);
                let (meter_tx, meter_rx) = triple_buffer(&MeterFrame::default());
                let core = AudioCore::new(
                    command_rx,
                    retired_tx,
                    graph,
                    meter_tx,
                    Arc::clone(&self.midi_input),
                );
                let failed = Arc::new(AtomicBool::new(false));
                match device::open_stream(
                    &self.settings,
                    core,
                    Arc::clone(&xruns),
                    Arc::clone(&failed),
                ) {
                    Ok(stream) => {
                        self.runtime = Some(Runtime {
                            _stream: stream,
                            commands: command_tx,
                            retired: retired_rx,
                            meters: meter_rx,
                            xruns,
                            failed,
                        });
                        self.last_error = Some(format!(
                            "requested audio device failed; using system default: {first}"
                        ));
                        self.restore_transport(resume);
                        Ok(())
                    }
                    Err(second) => {
                        let error = format!("{first}; fallback failed: {second}");
                        self.last_error = Some(error.clone());
                        Err(error)
                    }
                }
            }
        }
    }
    fn restore_transport(&mut self, frame: Option<MeterFrame>) {
        let Some(frame) = frame else { return };
        if frame.position > 0 {
            let _ = self.push(AudioCommand::SeekTo(frame.position));
        }
        if frame.playing {
            let _ = self.push(AudioCommand::SetPlaying(true));
        }
    }
    fn push(&mut self, command: AudioCommand) -> Result<(), String> {
        self.collect_retired();
        self.runtime
            .as_mut()
            .ok_or("audio stream is not initialized")?
            .commands
            .push(command)
            .map_err(|_| "audio command queue is full".into())
    }
    fn collect_retired(&mut self) {
        if let Some(runtime) = &mut self.runtime {
            while let Ok(graph) = runtime.retired.pop() {
                drop(graph)
            }
        }
    }
    pub fn load(
        &mut self,
        path: &str,
        on_progress: impl FnMut(DecodeProgress),
    ) -> Result<NativeAssetInfo, String> {
        let asset = decode_asset(path, self.settings.sample_rate, on_progress)?;
        let info = asset.info();
        self.assets.insert(asset.id.clone(), asset);
        Ok(info)
    }
    pub fn unload(&mut self, id: &str) {
        self.assets.remove(id);
        self.collect_retired()
    }
    pub fn asset_peaks(&self, id: &str, lod: u8) -> Result<&[f32], String> {
        self.assets
            .get(id)
            .ok_or_else(|| format!("unknown asset: {id}"))?
            .peaks(lod)
            .ok_or_else(|| format!("unsupported peak LOD: {lod}"))
    }
    pub fn sync_graph(&mut self, spec: GraphSnapshot) -> Result<(), String> {
        let (graph, bindings) = AudioGraph::build(&spec, &self.assets, self.settings.sample_rate);
        self.bindings = bindings;
        self.last_snapshot = Some(spec);
        self.graph_revision = self.graph_revision.wrapping_add(1);
        self.push(AudioCommand::SwapGraph(graph))
    }
    pub fn play(&mut self, from: Option<f64>) -> Result<(), String> {
        if let Some(sec) = from {
            self.push(AudioCommand::SeekTo(
                (sec * f64::from(self.settings.sample_rate)) as u64,
            ))?
        }
        self.push(AudioCommand::SetPlaying(true))
    }
    pub fn pause(&mut self) -> Result<(), String> {
        self.push(AudioCommand::SetPlaying(false))
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.push(AudioCommand::Stop)
    }
    pub fn seek(&mut self, sec: f64) -> Result<(), String> {
        self.push(AudioCommand::SeekTo(
            (sec * f64::from(self.settings.sample_rate)) as u64,
        ))
    }
    pub fn set_track_gain(&mut self, id: &str, db: f32) -> Result<(), String> {
        let i = *self.bindings.tracks.get(id).ok_or("unknown track")?;
        self.push(AudioCommand::SetTrackGain {
            track: i,
            gain: db_to_gain(db),
        })
    }
    pub fn set_track_pan(&mut self, id: &str, pan: f32) -> Result<(), String> {
        let i = *self.bindings.tracks.get(id).ok_or("unknown track")?;
        self.push(AudioCommand::SetTrackPan { track: i, pan })
    }
    pub fn set_track_mute(&mut self, id: &str, v: bool) -> Result<(), String> {
        let i = *self.bindings.tracks.get(id).ok_or("unknown track")?;
        self.push(AudioCommand::SetTrackMute { track: i, muted: v })
    }
    pub fn set_track_solo(&mut self, id: &str, v: bool) -> Result<(), String> {
        let i = *self.bindings.tracks.get(id).ok_or("unknown track")?;
        self.push(AudioCommand::SetTrackSolo { track: i, solo: v })
    }
    pub fn set_send(&mut self, id: &str, db: f32) -> Result<(), String> {
        let (t, s) = *self.bindings.sends.get(id).ok_or("unknown send")?;
        self.push(AudioCommand::SetSendGain {
            track: t,
            send: s,
            gain: db_to_gain(db),
        })
    }
    pub fn set_bus_gain(&mut self, id: &str, db: f32) -> Result<(), String> {
        let bus = *self.bindings.buses.get(id).ok_or("unknown bus")?;
        self.push(AudioCommand::SetBusGain {
            bus,
            gain: db_to_gain(db),
        })
    }
    pub fn set_master_gain(&mut self, db: f32) -> Result<(), String> {
        self.push(AudioCommand::SetMasterGain {
            gain: db_to_gain(db),
        })
    }
    pub fn set_effect(&mut self, id: &str, param: &str, value: f32) -> Result<(), String> {
        let target = *self.bindings.effects.get(id).ok_or("unknown effect")?;
        if let Some(spec) = self
            .last_snapshot
            .as_mut()
            .and_then(|v| find_effect_mut(v, id))
        {
            spec.params.insert(param.to_owned(), value);
        }
        let (key, len) = param_key(param);
        self.push(AudioCommand::SetEffectParam {
            target,
            param: key,
            len,
            value,
        })
    }
    pub fn set_instrument_param(
        &mut self,
        track_id: &str,
        param: &str,
        value: f32,
    ) -> Result<(), String> {
        let track = *self
            .bindings
            .tracks
            .get(track_id)
            .ok_or("unknown instrument track")?;
        let (param, len) = param_key(param);
        self.push(AudioCommand::SetInstrumentParam {
            track,
            param,
            len,
            value,
        })
    }
    pub fn midi_note(
        &mut self,
        track_id: &str,
        note_id: i32,
        pitch: u8,
        velocity: f32,
        note_on: bool,
    ) -> Result<(), String> {
        let track = *self
            .bindings
            .tracks
            .get(track_id)
            .ok_or("unknown instrument track")?;
        let kind = if note_on && velocity > 0.0 {
            NoteEventKind::NoteOn {
                note_id,
                pitch: pitch.min(127),
                velocity: velocity.clamp(0.0, 1.0),
                tuning_cents: 0.0,
            }
        } else {
            NoteEventKind::NoteOff {
                note_id,
                pitch: pitch.min(127),
                velocity: velocity.clamp(0.0, 1.0),
            }
        };
        self.push(AudioCommand::LiveMidi {
            track,
            event: NoteEvent {
                sample_offset: 0,
                kind,
            },
        })
    }
    pub fn midi_all_notes_off(&mut self, track_id: &str) -> Result<(), String> {
        let track = *self
            .bindings
            .tracks
            .get(track_id)
            .ok_or("unknown instrument track")?;
        self.push(AudioCommand::LiveMidi {
            track,
            event: NoteEvent {
                sample_offset: 0,
                kind: NoteEventKind::AllNotesOff,
            },
        })
    }
    pub fn poll(&mut self) -> EngineSnapshot {
        let stream_failed = self
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.failed.swap(false, Ordering::AcqRel));
        if stream_failed {
            if let Err(error) = self.restart_stream() {
                self.last_error = Some(format!("audio stream recovery failed: {error}"));
            }
        }
        self.collect_retired();
        let (frame, running, xruns) = if let Some(runtime) = &mut self.runtime {
            runtime.meters.update();
            (
                *runtime.meters.output_buffer_mut(),
                true,
                runtime.xruns.load(Ordering::Relaxed),
            )
        } else {
            (MeterFrame::default(), false, 0)
        };
        let track_levels = frame.levels[..self.bindings.track_ids.len().min(MAX_TRACKS)].to_vec();
        let multiband_levels = frame.multiband_levels[..self
            .bindings
            .multiband_meter_ids
            .len()
            .min(MAX_EFFECT_METERS)]
            .iter()
            .map(|bands| MultibandLevels {
                low: StereoLevel {
                    left: bands[0][0],
                    right: bands[0][1],
                },
                mid: StereoLevel {
                    left: bands[1][0],
                    right: bands[1][1],
                },
                high: StereoLevel {
                    left: bands[2][0],
                    right: bands[2][1],
                },
            })
            .collect();
        let distortion_spectra = frame.distortion_spectra[..self
            .bindings
            .distortion_meter_ids
            .len()
            .min(MAX_EFFECT_METERS)]
            .iter()
            .map(|spectrum| spectrum.to_vec())
            .collect();
        EngineSnapshot {
            playhead_sec: frame.position as f64 / f64::from(self.settings.sample_rate),
            track_levels,
            master_level: frame.master,
            stream: StreamStatus {
                latency_ms: f64::from(self.settings.buffer_size)
                    / f64::from(self.settings.sample_rate)
                    * 1000.0,
                xruns,
                running,
                error: self.last_error.clone(),
                pdc_samples: frame.pdc,
            },
            graph_revision: self.graph_revision,
            playing: frame.playing,
            active_voice_counts: frame.active_voice_counts
                [..self.bindings.track_ids.len().min(MAX_TRACKS)]
                .to_vec(),
            multiband_levels,
            distortion_spectra,
        }
    }
    pub fn backends(&self) -> Vec<AudioBackendInfo> {
        device::list_backends()
    }
    pub fn devices(&self, id: &str) -> Result<Vec<AudioDeviceInfo>, String> {
        device::list_devices(id)
    }
    pub fn midi_inputs(&self) -> Result<Vec<MidiInputPortInfo>, String> {
        let input = MidiInput::new("MiniDAW MIDI scan").map_err(|error| error.to_string())?;
        input
            .ports()
            .iter()
            .enumerate()
            .map(|(index, port)| {
                let id = index.to_string();
                Ok(MidiInputPortInfo {
                    name: input.port_name(port).map_err(|error| error.to_string())?,
                    connected: self.midi_connections.contains_key(&id),
                    id,
                })
            })
            .collect()
    }
    pub fn connect_midi_input(&mut self, port_id: &str, track_id: &str) -> Result<(), String> {
        if self.midi_connections.contains_key(port_id) {
            return Ok(());
        }
        let track = *self
            .bindings
            .tracks
            .get(track_id)
            .ok_or("unknown MIDI target track")?;
        let mut input = MidiInput::new("MiniDAW MIDI input").map_err(|error| error.to_string())?;
        input.ignore(Ignore::None);
        let port_index = port_id.parse::<usize>().map_err(|_| "invalid MIDI port")?;
        let port = input
            .ports()
            .get(port_index)
            .cloned()
            .ok_or("MIDI port is unavailable")?;
        let queue = Arc::clone(&self.midi_input);
        let counter = Arc::clone(&self.midi_note_counter);
        let mut note_ids = [[-1_i32; 8]; 128];
        let mut note_counts = [0_usize; 128];
        let connection = input
            .connect(
                &port,
                "MiniDAW",
                move |_stamp, message, _| {
                    if message.is_empty() {
                        return;
                    }
                    let status = message[0] & 0xf0;
                    let pitch = message.get(1).copied().unwrap_or(0).min(127);
                    let value = message.get(2).copied().unwrap_or(0);
                    let kind = match status {
                        0x90 if value > 0 => {
                            let id = counter.fetch_add(1, Ordering::Relaxed);
                            let count = &mut note_counts[pitch as usize];
                            if *count < 8 {
                                note_ids[pitch as usize][*count] = id;
                                *count += 1;
                            }
                            NoteEventKind::NoteOn {
                                note_id: id,
                                pitch,
                                velocity: f32::from(value) / 127.0,
                                tuning_cents: 0.0,
                            }
                        }
                        0x80 | 0x90 => {
                            let count = &mut note_counts[pitch as usize];
                            let id = if *count > 0 {
                                *count -= 1;
                                note_ids[pitch as usize][*count]
                            } else {
                                -1
                            };
                            NoteEventKind::NoteOff {
                                note_id: id,
                                pitch,
                                velocity: f32::from(value) / 127.0,
                            }
                        }
                        0xb0 => NoteEventKind::Controller {
                            cc: pitch,
                            value: f32::from(value) / 127.0,
                        },
                        0xe0 => {
                            let bend = (u16::from(value) << 7) | u16::from(pitch);
                            NoteEventKind::PitchBend {
                                value: (f32::from(bend) - 8192.0) / 8192.0,
                            }
                        }
                        _ => return,
                    };
                    let _ = queue.push(LiveMidiMessage {
                        track,
                        event: NoteEvent {
                            sample_offset: 0,
                            kind,
                        },
                    });
                },
                (),
            )
            .map_err(|error| error.to_string())?;
        self.midi_connections.insert(port_id.to_owned(), connection);
        Ok(())
    }
    pub fn disconnect_midi_input(&mut self, port_id: &str) {
        self.midi_connections.remove(port_id);
    }
    pub fn settings(&self) -> AudioSettings {
        self.settings.clone()
    }
    pub fn set_settings(&mut self, v: AudioSettings) -> Result<(), String> {
        self.settings = v;
        self.restart_stream()
    }
    pub fn export(
        &self,
        request: &ExportRequest,
        cancelled: &AtomicBool,
        mut on_progress: impl FnMut(ExportProgress),
    ) -> Result<ExportResult, String> {
        let spec = self.last_snapshot.as_ref().ok_or("no project graph")?;
        if !matches!(request.bit_depth, 16 | 24 | 32) {
            return Err("export bit depth must be 16, 24, or 32".into());
        }
        if request.sample_rate != self.settings.sample_rate {
            return Err("export sample rate must match the loaded project sample rate".into());
        }
        let (graph, _) = AudioGraph::build(spec, &self.assets, request.sample_rate);
        let end = spec
            .tracks
            .iter()
            .flat_map(|track| {
                track
                    .clips
                    .iter()
                    .map(|clip| clip.start_sec + clip.duration_sec)
                    .chain(
                        track
                            .midi_clips
                            .iter()
                            .map(|clip| clip.start_sec + clip.duration_sec),
                    )
            })
            .fold(0.0_f64, f64::max);
        let total = (end * f64::from(request.sample_rate)) as usize + graph.tail_samples();
        let progress_step = (total / 100).max(MAX_BLOCK_SIZE);
        let mut normalization_gain = 1.0_f32;
        if request.normalize {
            let (mut analysis, _) = AudioGraph::build(spec, &self.assets, request.sample_rate);
            analysis.reset(0);
            let mut output = vec![0.0_f32; MAX_BLOCK_SIZE * 2];
            let mut levels = [Level::default(); MAX_TRACKS];
            let mut master = Level::default();
            let mut position = 0;
            let mut peak = 0.0_f32;
            let mut next_progress = 0;
            while position < total {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(ExportResult {
                        output_path: request.output_path.clone(),
                        peak_db: -120.0,
                        clipped: false,
                        cancelled: true,
                    });
                }
                let frames = MAX_BLOCK_SIZE.min(total - position);
                analysis.process(
                    position as u64,
                    frames,
                    &mut output[..frames * 2],
                    &mut levels,
                    &mut master,
                    true,
                );
                for value in &output[..frames * 2] {
                    peak = peak.max(value.abs())
                }
                position += frames;
                if position >= next_progress {
                    on_progress(ExportProgress {
                        stage: "analyze".into(),
                        rendered_frames: position as u64,
                        total_frames: total as u64,
                        fraction: position as f32 / total.max(1) as f32 * 0.5,
                    });
                    next_progress = position + progress_step
                }
            }
            if peak > 0.0 {
                normalization_gain = 0.999 / peak
            }
        }
        let wav = hound::WavSpec {
            channels: 2,
            sample_rate: request.sample_rate,
            bits_per_sample: request.bit_depth,
            sample_format: if request.bit_depth == 32 {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            },
        };
        let mut writer = hound::WavWriter::create(Path::new(&request.output_path), wav)
            .map_err(|e| e.to_string())?;
        let mut graph = graph;
        graph.reset(0);
        let mut output = vec![0.0_f32; MAX_BLOCK_SIZE * 2];
        let mut pos = 0;
        let mut peak = 0.0_f32;
        let mut clipped = false;
        let mut next_progress = 0;
        let mut levels = [Level::default(); MAX_TRACKS];
        let mut master = Level::default();
        while pos < total {
            if cancelled.load(Ordering::Relaxed) {
                writer.finalize().map_err(|e| e.to_string())?;
                return Ok(ExportResult {
                    output_path: request.output_path.clone(),
                    peak_db: if peak > 0.0 {
                        20.0 * peak.log10()
                    } else {
                        -120.0
                    },
                    clipped,
                    cancelled: true,
                });
            }
            let frames = MAX_BLOCK_SIZE.min(total - pos);
            graph.process(
                pos as u64,
                frames,
                &mut output[..frames * 2],
                &mut levels,
                &mut master,
                true,
            );
            for value in &output[..frames * 2] {
                let value = *value * normalization_gain;
                peak = peak.max(value.abs());
                clipped |= value.abs() > 1.0;
                if request.bit_depth == 32 {
                    writer.write_sample(value).map_err(|e| e.to_string())?
                } else if request.bit_depth == 24 {
                    writer
                        .write_sample((value.clamp(-1.0, 1.0) * 8_388_607.0) as i32)
                        .map_err(|e| e.to_string())?
                } else {
                    writer
                        .write_sample((value.clamp(-1.0, 1.0) * 32_767.0) as i16)
                        .map_err(|e| e.to_string())?
                }
            }
            pos += frames;
            if pos >= next_progress {
                let base = if request.normalize { 0.5 } else { 0.0 };
                let scale = if request.normalize { 0.5 } else { 1.0 };
                on_progress(ExportProgress {
                    stage: "render".into(),
                    rendered_frames: pos as u64,
                    total_frames: total as u64,
                    fraction: base + pos as f32 / total.max(1) as f32 * scale,
                });
                next_progress = pos + progress_step
            }
        }
        writer.finalize().map_err(|e| e.to_string())?;
        Ok(ExportResult {
            output_path: request.output_path.clone(),
            peak_db: if peak > 0.0 {
                20.0 * peak.log10()
            } else {
                -120.0
            },
            clipped,
            cancelled: false,
        })
    }
    pub fn eq_response(&self, id: &str, points: usize) -> Result<EqFrequencyResponse, String> {
        let spec = self
            .last_snapshot
            .as_ref()
            .and_then(|v| find_effect(v, id))
            .ok_or("unknown effect")?;
        let fx =
            create_effect(spec, self.settings.sample_rate as f32).ok_or("effect is not an EQ")?;
        fx.response(points)
            .ok_or("effect does not expose a frequency response".into())
    }
}
fn find_effect<'a>(graph: &'a GraphSnapshot, id: &str) -> Option<&'a EffectSpec> {
    graph
        .tracks
        .iter()
        .flat_map(|v| v.effects.iter())
        .chain(graph.buses.iter().flat_map(|v| v.effects.iter()))
        .chain(graph.master.effects.iter())
        .find(|v| v.id == id)
}
fn find_effect_mut<'a>(graph: &'a mut GraphSnapshot, id: &str) -> Option<&'a mut EffectSpec> {
    graph
        .tracks
        .iter_mut()
        .flat_map(|v| v.effects.iter_mut())
        .chain(graph.buses.iter_mut().flat_map(|v| v.effects.iter_mut()))
        .chain(graph.master.effects.iter_mut())
        .find(|v| v.id == id)
}

#[cfg(test)]
mod realtime_tests {
    use super::*;
    use crate::audio::types::{
        InstrumentSpec, LoopSpec, MasterSpec, MidiClipSpec, MidiNoteSpec, TrackSpec, TransportSpec,
    };
    use std::{thread, time::Duration};

    #[test]
    #[ignore = "opens the machine's real default audio output"]
    fn native_play_advances_the_audio_clock() {
        let mut engine = NativeEngine::default();
        engine.init().expect("default audio stream should open");
        engine.play(Some(0.0)).expect("play command should enqueue");
        thread::sleep(Duration::from_millis(120));
        let snapshot = engine.poll();
        assert!(snapshot.stream.running);
        assert!(snapshot.playing);
        assert!(
            snapshot.playhead_sec > 0.02,
            "audio clock did not advance: {}",
            snapshot.playhead_sec
        );
    }

    #[test]
    #[ignore = "opens the machine's real default audio output"]
    fn native_midi_stays_audible_for_multiple_seconds() {
        let mut engine = NativeEngine::default();
        engine.init().expect("default audio stream should open");
        engine
            .sync_graph(GraphSnapshot {
                tracks: vec![TrackSpec {
                    id: "survival-track".into(),
                    kind: "instrument".into(),
                    name: "Playback survival probe".into(),
                    clips: Vec::new(),
                    midi_clips: vec![MidiClipSpec {
                        id: "survival-clip".into(),
                        name: "Sustained note".into(),
                        start_sec: 0.0,
                        duration_sec: 4.0,
                        loop_enabled: false,
                        loop_start_ticks: 0,
                        loop_length_ticks: 7_680,
                        notes: vec![MidiNoteSpec {
                            id: "survival-note".into(),
                            pitch: 57,
                            velocity: 96,
                            start_ticks: 0,
                            length_ticks: 7_680,
                            release_velocity: 64,
                            muted: false,
                        }],
                        transpose_semitones: 0,
                        velocity_scale: 1.0,
                        muted: false,
                    }],
                    instrument: Some(InstrumentSpec {
                        id: "survival-synth".into(),
                        kind: "builtin:testtone".into(),
                        plugin: None,
                        params: HashMap::from([("gainDb".into(), -24.0)]),
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
                    is_playing: false,
                    loop_: LoopSpec {
                        enabled: false,
                        start_sec: 0.0,
                        end_sec: 0.0,
                    },
                },
            })
            .expect("test graph should enqueue");
        engine.play(Some(0.0)).expect("play command should enqueue");
        thread::sleep(Duration::from_millis(2_200));
        let snapshot = engine.poll();
        assert!(snapshot.playing);
        assert!(
            snapshot.playhead_sec > 1.8,
            "stream stopped early: {snapshot:?}"
        );
        assert!(
            snapshot.master_level.peak > 0.0001,
            "sound stopped early: {snapshot:?}"
        );
    }
}
