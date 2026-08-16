// Control-plane ownership and allocation-free CPAL callback core.
pub use super::midi_service::{
    DetachedMidiInputRoute, MidiConnectPlan, MidiInputRouteRequest, OpenedMidiInputRoute,
};
use super::{
    asset::{decode_asset, AudioAsset},
    command::{param_key, AudioCommand, MetronomeSettings},
    device,
    dsp::{
        create_effect, PluginControl, PluginEditorAction, DISTORTION_SPECTRUM_BINS,
        LIMITER_METER_VALUES,
    },
    graph::{AudioGraph, Bindings},
    instrument::{LiveMidiMessage, NoteEvent, NoteEventKind},
    metrics::RealtimeMetrics,
    midi_service::MidiService,
    plugin::StableProcessorRegistry,
    runtime::{SchedulerSnapshot, SleepPolicy, WakeReason},
    types::{
        db_to_gain, AudioBackendInfo, AudioDeviceInfo, AudioSettings, DecodeProgress, EffectSpec,
        EngineSnapshot, EqFrequencyResponse, ExportProgress, ExportRequest, ExportResult,
        GraphSnapshot, Level, LimiterMetrics, MidiInputPortInfo, MultibandLevels, NativeAssetInfo,
        PluginParameterChange, StereoLevel, StreamStatus,
    },
    COMMAND_CAPACITY, MAX_BLOCK_SIZE, MAX_EFFECT_METERS, MAX_TRACKS, RETIRED_GRAPH_CAPACITY,
};
use cpal::Stream;
use crossbeam_queue::ArrayQueue;
pub use ministudio_dsp::EmbeddedPluginEditor;
use mp3lame_encoder::{
    max_required_buffer_size, Bitrate, Builder as Mp3Builder, FlushGap, InterleavedPcm, Mode,
    Quality,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};

enum ExportSink {
    Wav {
        writer: hound::WavWriter<BufWriter<File>>,
        bit_depth: u16,
    },
    Mp3 {
        encoder: mp3lame_encoder::Encoder,
        writer: BufWriter<File>,
        encoded: Vec<u8>,
    },
}

impl ExportSink {
    fn new(request: &ExportRequest) -> Result<Self, String> {
        match request.format.as_str() {
            "wav" => {
                let spec = hound::WavSpec {
                    channels: 2,
                    sample_rate: request.sample_rate,
                    bits_per_sample: request.bit_depth,
                    sample_format: if request.bit_depth == 32 {
                        hound::SampleFormat::Float
                    } else {
                        hound::SampleFormat::Int
                    },
                };
                let writer = hound::WavWriter::create(Path::new(&request.output_path), spec)
                    .map_err(|error| error.to_string())?;
                Ok(Self::Wav {
                    writer,
                    bit_depth: request.bit_depth,
                })
            }
            "mp3" => {
                let encoder = Mp3Builder::new()
                    .ok_or("failed to allocate LAME MP3 encoder")?
                    .with_num_channels(2)
                    .map_err(|error| error.to_string())?
                    .with_sample_rate(request.sample_rate)
                    .map_err(|error| error.to_string())?
                    .with_brate(mp3_bitrate(request.mp3_bitrate_kbps)?)
                    .map_err(|error| error.to_string())?
                    .with_mode(Mode::JointStereo)
                    .map_err(|error| error.to_string())?
                    .with_quality(Quality::NearBest)
                    .map_err(|error| error.to_string())?
                    .with_to_write_vbr_tag(false)
                    .map_err(|error| error.to_string())?
                    .build()
                    .map_err(|error| error.to_string())?;
                let writer = BufWriter::new(
                    File::create(Path::new(&request.output_path))
                        .map_err(|error| error.to_string())?,
                );
                Ok(Self::Mp3 {
                    encoder,
                    writer,
                    encoded: Vec::with_capacity(max_required_buffer_size(MAX_BLOCK_SIZE)),
                })
            }
            format => Err(format!("unsupported export format: {format}")),
        }
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<(), String> {
        match self {
            Self::Wav { writer, bit_depth } => {
                for value in samples {
                    if *bit_depth == 32 {
                        writer
                            .write_sample(*value)
                            .map_err(|error| error.to_string())?
                    } else if *bit_depth == 24 {
                        writer
                            .write_sample((value.clamp(-1.0, 1.0) * 8_388_607.0) as i32)
                            .map_err(|error| error.to_string())?
                    } else {
                        writer
                            .write_sample((value.clamp(-1.0, 1.0) * 32_767.0) as i16)
                            .map_err(|error| error.to_string())?
                    }
                }
                Ok(())
            }
            Self::Mp3 {
                encoder,
                writer,
                encoded,
            } => {
                encoded.clear();
                encoded.reserve(max_required_buffer_size(samples.len() / 2));
                encoder
                    .encode_to_vec(InterleavedPcm(samples), encoded)
                    .map_err(|error| error.to_string())?;
                writer.write_all(encoded).map_err(|error| error.to_string())
            }
        }
    }

    fn finalize(self) -> Result<(), String> {
        match self {
            Self::Wav { writer, .. } => writer.finalize().map_err(|error| error.to_string()),
            Self::Mp3 {
                mut encoder,
                mut writer,
                mut encoded,
            } => {
                encoded.clear();
                encoded.reserve(7_200);
                encoder
                    .flush_to_vec::<FlushGap>(&mut encoded)
                    .map_err(|error| error.to_string())?;
                writer
                    .write_all(&encoded)
                    .map_err(|error| error.to_string())?;
                writer.flush().map_err(|error| error.to_string())
            }
        }
    }
}

fn mp3_bitrate(value: u16) -> Result<Bitrate, String> {
    match value {
        128 => Ok(Bitrate::Kbps128),
        192 => Ok(Bitrate::Kbps192),
        256 => Ok(Bitrate::Kbps256),
        320 => Ok(Bitrate::Kbps320),
        _ => Err("MP3 bitrate must be 128, 192, 256, or 320 kbps".into()),
    }
}
use triple_buffer::{triple_buffer, Input, Output};

const MAX_METRONOME_CLICK_SAMPLES: usize = 8192;

fn metronome_click_waveforms(
    sample_rate: u32,
) -> (
    [f32; MAX_METRONOME_CLICK_SAMPLES],
    [f32; MAX_METRONOME_CLICK_SAMPLES],
    usize,
) {
    let mut accent = [0.0; MAX_METRONOME_CLICK_SAMPLES];
    let mut regular = [0.0; MAX_METRONOME_CLICK_SAMPLES];
    let rate = sample_rate.max(1) as f32;
    let len = ((rate * 0.035).round() as usize).clamp(1, MAX_METRONOME_CLICK_SAMPLES);
    for index in 0..len {
        let t = index as f32 / rate;
        let attack = (t / 0.00035).min(1.0);
        let body = (-t * 76.0).exp();
        let edge = (-t * 210.0).exp();
        accent[index] = attack
            * body
            * ((std::f32::consts::TAU * 1760.0 * t).sin()
                + 0.22 * edge * (std::f32::consts::TAU * 3520.0 * t).sin());
        regular[index] = attack
            * body
            * ((std::f32::consts::TAU * 1180.0 * t).sin()
                + 0.16 * edge * (std::f32::consts::TAU * 2360.0 * t).sin());
    }
    (accent, regular, len)
}

#[derive(Clone, Copy)]
pub struct MeterFrame {
    pub position: u64,
    pub levels: [Level; MAX_TRACKS],
    pub master: Level,
    pub pdc: usize,
    pub playing: bool,
    pub count_in_beats_remaining: u32,
    pub active_voice_counts: [u32; MAX_TRACKS],
    pub multiband_levels: [[[f32; 2]; 3]; MAX_EFFECT_METERS],
    pub distortion_spectra: [[f32; DISTORTION_SPECTRUM_BINS]; MAX_EFFECT_METERS],
    pub limiter_metrics: [[f32; LIMITER_METER_VALUES]; MAX_EFFECT_METERS],
    pub scheduler: SchedulerSnapshot,
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
            count_in_beats_remaining: 0,
            active_voice_counts: [0; MAX_TRACKS],
            multiband_levels: [[[0.0; 2]; 3]; MAX_EFFECT_METERS],
            distortion_spectra: [[0.0; DISTORTION_SPECTRUM_BINS]; MAX_EFFECT_METERS],
            limiter_metrics: [[-120.0, -120.0, 0.0, -120.0, -120.0, -120.0, -120.0];
                MAX_EFFECT_METERS],
            scheduler: SchedulerSnapshot::default(),
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
    metronome: MetronomeSettings,
    count_in_remaining: u64,
    count_in_elapsed: u64,
    click_accent: [f32; MAX_METRONOME_CLICK_SAMPLES],
    click_regular: [f32; MAX_METRONOME_CLICK_SAMPLES],
    click_len: usize,
}

/// Hard callback budget for control commands. Remaining commands stay queued for the next
/// block; a UI flood can therefore add bounded control latency but can never consume an entire
/// audio deadline.
const MAX_COMMANDS_PER_CALLBACK: usize = 256;
/// Hardware/UI MIDI has its own bounded budget so a faulty device cannot starve graph process.
const MAX_MIDI_PACKETS_PER_CALLBACK: usize = 512;

impl AudioCore {
    #[cfg(test)]
    pub fn placeholder() -> Self {
        let (_, rx): (Producer<AudioCommand>, Consumer<AudioCommand>) = RingBuffer::new(1);
        let (tx, _): (Producer<Box<AudioGraph>>, Consumer<Box<AudioGraph>>) = RingBuffer::new(1);
        let (input, _) = triple_buffer(&MeterFrame::default());
        let (click_accent, click_regular, click_len) = metronome_click_waveforms(48_000);
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
            metronome: MetronomeSettings {
                enabled: false,
                gain: db_to_gain(-12.0),
                bpm: 120.0,
                numerator: 4,
                denominator: 4,
            },
            count_in_remaining: 0,
            count_in_elapsed: 0,
            click_accent,
            click_regular,
            click_len,
        }
    }
    fn new(
        commands: Consumer<AudioCommand>,
        retired: Producer<Box<AudioGraph>>,
        graph: Box<AudioGraph>,
        meters: Input<MeterFrame>,
        midi_input: Arc<ArrayQueue<LiveMidiMessage>>,
    ) -> Self {
        let (click_accent, click_regular, click_len) =
            metronome_click_waveforms(graph.sample_rate());
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
            metronome: MetronomeSettings {
                enabled: false,
                gain: db_to_gain(-12.0),
                bpm: 120.0,
                numerator: 4,
                denominator: 4,
            },
            count_in_remaining: 0,
            count_in_elapsed: 0,
            click_accent,
            click_regular,
            click_len,
        }
    }
    pub fn render(&mut self, output: &mut [f32], frames: usize) {
        if self.placeholder {
            output.fill(0.0);
            return;
        }
        self.drain_commands();
        let output = &mut output[..frames * 2];
        if self.playing && self.count_in_remaining > 0 {
            output.fill(0.0);
            let count_in_frames = frames.min(self.count_in_remaining as usize);
            self.mix_metronome(
                &mut output[..count_in_frames * 2],
                self.count_in_elapsed,
                count_in_frames,
                true,
            );
            self.count_in_elapsed += count_in_frames as u64;
            self.count_in_remaining -= count_in_frames as u64;
            let frame = self.meters.input_buffer_mut();
            frame.levels.fill(Level::default());
            frame.master = Level::default();
            self.publish();
            if count_in_frames == frames {
                return;
            }
            self.render_timeline(&mut output[count_in_frames * 2..], frames - count_in_frames);
            return;
        }
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
        self.render_timeline(output, frames);
    }
    fn render_timeline(&mut self, output: &mut [f32], frames: usize) {
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
            let mut segment = if let Some((_, end)) = loop_range {
                remaining.min((end - self.position) as usize)
            } else {
                remaining
            };
            if let Some(boundary) = self
                .graph
                .tempo_map()
                .next_change_sample_after(self.position)
            {
                segment = segment.min(boundary.saturating_sub(self.position) as usize);
            }
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
            self.mix_metronome(&mut output[from..to], self.position, segment, false);
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
            frame.count_in_beats_remaining = 0;
            self.graph
                .write_active_voice_counts(&mut frame.active_voice_counts);
            self.graph
                .write_multiband_levels(&mut frame.multiband_levels);
            self.graph
                .write_distortion_spectra(&mut frame.distortion_spectra);
            self.graph.write_limiter_metrics(&mut frame.limiter_metrics);
            frame.scheduler = self.graph.scheduler_snapshot();
            self.meters.publish();
        }
    }
    fn beat_samples(&self) -> f64 {
        let map = self.graph.tempo_map();
        let tick = map.samples_to_ticks(self.position);
        let bpm = map.bpm_at_tick(tick).clamp(1.0, 999.0);
        let denominator = f64::from(map.time_signature_at_tick(tick).denominator.max(1));
        self.graph.sample_rate() as f64 * 60.0 / bpm * (4.0 / denominator)
    }
    fn count_in_samples(&self, bars: u8) -> u64 {
        (self.beat_samples() * f64::from(self.metronome.numerator.max(1)) * f64::from(bars))
            .round()
            .max(0.0) as u64
    }
    fn count_in_beats_remaining(&self) -> u32 {
        if self.count_in_remaining == 0 {
            0
        } else {
            (self.count_in_remaining as f64 / self.beat_samples())
                .ceil()
                .clamp(0.0, u32::MAX as f64) as u32
        }
    }
    fn mix_metronome(&self, output: &mut [f32], start_sample: u64, frames: usize, force: bool) {
        if (!force && !self.metronome.enabled) || frames == 0 || self.click_len == 0 {
            return;
        }
        let map = self.graph.tempo_map();
        let start_tick = map.samples_to_ticks(start_sample);
        let (mut bar, beat_position) = map.tick_to_bar_beat(start_tick);
        let mut beat = beat_position.floor().max(0.0) as u32;
        let mut beat_start =
            map.ticks_to_samples(map.bar_beat_to_tick(bar, f64::from(beat))) as i64;
        if beat_start > start_sample as i64 {
            if beat > 0 {
                beat -= 1;
            } else if bar > 1 {
                bar -= 1;
                beat = u32::from(map.time_signature_at_bar(bar).numerator).saturating_sub(1);
            }
            beat_start = map.ticks_to_samples(map.bar_beat_to_tick(bar, f64::from(beat))) as i64;
        }
        let gain = self.metronome.gain.clamp(0.0, 2.0);
        for frame in 0..frames {
            let absolute = start_sample as i64 + frame as i64;
            let signature = map.time_signature_at_bar(bar);
            let mut next_bar = bar;
            let mut next_beat = beat + 1;
            if next_beat >= u32::from(signature.numerator) {
                next_bar += 1;
                next_beat = 0;
            }
            let mut next_start =
                map.ticks_to_samples(map.bar_beat_to_tick(next_bar, f64::from(next_beat))) as i64;
            while absolute >= next_start {
                bar = next_bar;
                beat = next_beat;
                beat_start = next_start;
                let next_signature = map.time_signature_at_bar(bar);
                next_beat = beat + 1;
                next_bar = bar;
                if next_beat >= u32::from(next_signature.numerator) {
                    next_bar += 1;
                    next_beat = 0;
                }
                next_start = map
                    .ticks_to_samples(map.bar_beat_to_tick(next_bar, f64::from(next_beat)))
                    as i64;
            }
            let age = absolute - beat_start;
            if age < 0 || age as usize >= self.click_len {
                continue;
            }
            let click = if beat == 0 {
                self.click_accent[age as usize]
            } else {
                self.click_regular[age as usize]
            } * gain;
            output[frame * 2] += click;
            output[frame * 2 + 1] += click;
        }
    }
    fn drain_commands(&mut self) {
        for _ in 0..MAX_COMMANDS_PER_CALLBACK {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                AudioCommand::SetPlaying {
                    playing,
                    count_in_bars,
                } => {
                    if self.playing && !playing {
                        self.graph.all_notes_off()
                    }
                    self.playing = playing;
                    self.count_in_remaining = if playing {
                        self.count_in_samples(count_in_bars.min(4))
                    } else {
                        0
                    };
                    self.count_in_elapsed = 0;
                    if playing {
                        self.graph.wake_all(WakeReason::Transport);
                    }
                }
                AudioCommand::SetMetronome(settings) => self.metronome = settings,
                AudioCommand::SeekTo(v) => {
                    self.position = v;
                    self.graph.reset(v);
                    self.count_in_remaining = 0;
                    self.count_in_elapsed = 0;
                }
                AudioCommand::Stop => {
                    self.playing = false;
                    self.position = 0;
                    self.graph.reset(0);
                    self.count_in_remaining = 0;
                    self.count_in_elapsed = 0;
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
                AudioCommand::SetEffectBypass { target, bypassed } => {
                    self.graph.set_effect_bypassed(target, bypassed)
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
                AudioCommand::SetSchedulerEnabled(enabled) => {
                    self.graph.set_scheduler_enabled(enabled)
                }
                AudioCommand::SetEffectSleepPolicy { target, policy } => {
                    self.graph.set_effect_sleep_policy(target, policy)
                }
                AudioCommand::WakeAll(reason) => self.graph.wake_all(reason),
                AudioCommand::LiveMidi { track, event } => self.graph.push_live_event(track, event),
                AudioCommand::SwapGraph(new_graph) => {
                    if self.retired.is_full() {
                        std::mem::forget(new_graph)
                    } else {
                        let old = std::mem::replace(&mut self.graph, new_graph);
                        let _ = self.retired.push(old);
                        self.graph.activate(self.position)
                    }
                }
            }
        }
        for _ in 0..MAX_MIDI_PACKETS_PER_CALLBACK {
            let Some(message) = self.midi_input.pop() else {
                break;
            };
            self.graph.push_live_event(message.track, message.event);
        }
    }
    fn publish(&mut self) {
        let count_in_beats_remaining = self.count_in_beats_remaining();
        let frame = self.meters.input_buffer_mut();
        frame.position = self.position;
        frame.playing = self.playing;
        frame.count_in_beats_remaining = count_in_beats_remaining;
        frame.pdc = self.graph.max_latency();
        self.graph
            .write_active_voice_counts(&mut frame.active_voice_counts);
        self.graph
            .write_multiband_levels(&mut frame.multiband_levels);
        self.graph
            .write_distortion_spectra(&mut frame.distortion_spectra);
        self.graph.write_limiter_metrics(&mut frame.limiter_metrics);
        frame.scheduler = self.graph.scheduler_snapshot();
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
    cpu_load: Arc<AtomicU32>,
    cpu_peak: Arc<AtomicU32>,
    metrics: Arc<RealtimeMetrics>,
}
/// A near-silence streak for one track's plug-in control, driving the process-isolation idle
/// hint. Lives entirely on the control plane (`poll`); the realtime graph never sees it.
#[derive(Default)]
struct IdleTracker {
    silent_since: Option<std::time::Instant>,
    trimmed: bool,
}

/// Below this the track counts as silent for idle-trim purposes - about -80 dBFS, well under
/// anything perceptible, so genuinely quiet-but-playing material never trips it.
const IDLE_SILENCE_PEAK: f32 = 0.0001;
/// How long a track must stay under `IDLE_SILENCE_PEAK` before its plug-in gets the idle hint.
/// Long enough that a normal pause between phrases never triggers it - this is for "loaded but
/// not currently being worked on", not "the note just ended".
const IDLE_TRIM_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

pub struct NativeEngine {
    settings: AudioSettings,
    runtime: Option<Runtime>,
    assets: HashMap<String, Arc<AudioAsset>>,
    bindings: Bindings,
    last_snapshot: Option<GraphSnapshot>,
    last_error: Option<String>,
    graph_revision: u64,
    midi_service: MidiService,
    stable_processors: StableProcessorRegistry,
    idle_trackers: HashMap<String, IdleTracker>,
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
                plugin_controls: HashMap::new(),
                track_ids: Vec::new(),
                multiband_meter_ids: Vec::new(),
                distortion_meter_ids: Vec::new(),
                limiter_meter_ids: Vec::new(),
            },
            last_snapshot: None,
            last_error: None,
            graph_revision: 0,
            midi_service: MidiService::default(),
            stable_processors: StableProcessorRegistry::default(),
            idle_trackers: HashMap::new(),
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
    pub fn dispose(&mut self) -> Vec<DetachedMidiInputRoute> {
        self.close_all_plugin_editors();
        self.runtime = None;
        let routes = self.midi_service.detach_all();
        self.stable_processors.clear();
        routes
    }
    fn restart_stream(&mut self) -> Result<(), String> {
        self.close_all_plugin_editors();
        let resume = self.runtime.as_mut().map(|runtime| {
            runtime.meters.update();
            *runtime.meters.output_buffer_mut()
        });
        let previous_xruns = self
            .runtime
            .as_ref()
            .map_or(0, |runtime| runtime.xruns.load(Ordering::Relaxed));
        self.runtime = None;
        if let Ok(rate) = device::negotiated_sample_rate(&self.settings) {
            self.settings.sample_rate = rate;
        }
        let graph = if let Some(spec) = &self.last_snapshot {
            let (g, b) = AudioGraph::build_with_plugins(
                spec,
                &self.assets,
                self.settings.sample_rate,
                &mut self.stable_processors,
            )?;
            self.bindings = b;
            self.midi_service.refresh_routes(&self.bindings.tracks);
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
            self.midi_service.ingress(),
        );
        let xruns = Arc::new(AtomicU64::new(previous_xruns));
        let failed = Arc::new(AtomicBool::new(false));
        let cpu_load = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let cpu_peak = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let metrics = Arc::new(RealtimeMetrics::default());
        match device::open_stream(
            &self.settings,
            core,
            Arc::clone(&xruns),
            Arc::clone(&failed),
            Arc::clone(&cpu_load),
            Arc::clone(&cpu_peak),
            Arc::clone(&metrics),
        ) {
            Ok(stream) => {
                self.runtime = Some(Runtime {
                    _stream: stream,
                    commands: command_tx,
                    retired: retired_rx,
                    meters: meter_rx,
                    xruns,
                    failed,
                    cpu_load,
                    cpu_peak,
                    metrics,
                });
                self.last_error = None;
                self.restore_transport(resume);
                Ok(())
            }
            Err(first) => {
                self.settings.backend_id = "default".into();
                self.settings.device_id = "default".into();
                if let Ok(rate) = device::negotiated_sample_rate(&self.settings) {
                    self.settings.sample_rate = rate;
                }
                let graph = if let Some(spec) = &self.last_snapshot {
                    let (graph, bindings) = AudioGraph::build_with_plugins(
                        spec,
                        &self.assets,
                        self.settings.sample_rate,
                        &mut self.stable_processors,
                    )?;
                    self.bindings = bindings;
                    self.midi_service.refresh_routes(&self.bindings.tracks);
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
                    self.midi_service.ingress(),
                );
                let failed = Arc::new(AtomicBool::new(false));
                let cpu_load = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
                let cpu_peak = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
                let metrics = Arc::new(RealtimeMetrics::default());
                match device::open_stream(
                    &self.settings,
                    core,
                    Arc::clone(&xruns),
                    Arc::clone(&failed),
                    Arc::clone(&cpu_load),
                    Arc::clone(&cpu_peak),
                    Arc::clone(&metrics),
                ) {
                    Ok(stream) => {
                        self.runtime = Some(Runtime {
                            _stream: stream,
                            commands: command_tx,
                            retired: retired_rx,
                            meters: meter_rx,
                            xruns,
                            failed,
                            cpu_load,
                            cpu_peak,
                            metrics,
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
            let _ = self.push(AudioCommand::SetPlaying {
                playing: true,
                count_in_bars: 0,
            });
        }
    }
    fn push(&mut self, command: AudioCommand) -> Result<(), String> {
        self.collect_retired();
        let runtime = self
            .runtime
            .as_mut()
            .ok_or("audio stream is not initialized")?;
        runtime
            .metrics
            .observe_command_depth(COMMAND_CAPACITY.saturating_sub(runtime.commands.slots()) + 1);
        runtime.commands.push(command).map_err(|_| {
            runtime.metrics.command_overflow();
            "audio command queue is full".into()
        })
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
        // Do not synchronously close every vendor GUI under the global engine
        // mutex. The old graph remains valid until the realtime swap retires
        // it; dropping its final PluginControl sends the GUI worker a
        // nonblocking Shutdown in graph-retirement order. This keeps unrelated
        // editors responsive while a new track/plugin graph is constructed.
        let (graph, bindings) = AudioGraph::build_with_plugins(
            &spec,
            &self.assets,
            self.settings.sample_rate,
            &mut self.stable_processors,
        )?;
        // Commit control-plane bindings only after the realtime queue accepts
        // the graph. If the bounded queue is full, the currently audible graph
        // and its editor/parameter lookup table must remain one transaction.
        self.push(AudioCommand::SwapGraph(graph))?;
        self.bindings = bindings;
        self.midi_service.refresh_routes(&self.bindings.tracks);
        self.last_snapshot = Some(spec);
        self.graph_revision = self.graph_revision.wrapping_add(1);
        // Stale entries (deleted/renamed tracks) would otherwise sit here forever; a rebuild is
        // infrequent enough that just losing the accumulated silence streaks is no loss.
        self.idle_trackers.clear();
        Ok(())
    }
    pub fn open_plugin_editor(&self, target_id: &str) -> Result<(), String> {
        let control = self
            .bindings
            .plugin_controls
            .get(target_id)
            .ok_or_else(|| format!("plug-in instance is not available: {target_id}"))?;
        if !control.has_editor() {
            return Err("plug-in does not expose a compatible native editor".into());
        }
        control.open_editor()
    }
    /// Looks up a plug-in's control handle without holding it past the lookup. Callers whose
    /// next step is a slow, blocking native call (opening a heavy instrument's editor view, for
    /// example) must clone this Arc and release the engine lock *before* making that call - the
    /// same `Arc<Mutex<NativeEngine>>` gates every other control-plane command (parameter
    /// changes, meters, transport), so holding it for however long one plug-in's GUI takes to
    /// come up would freeze every other loaded instrument's controls along with it, not just
    /// the slow one's.
    pub fn plugin_control(&self, target_id: &str) -> Result<Arc<dyn PluginControl>, String> {
        self.bindings
            .plugin_controls
            .get(target_id)
            .cloned()
            .ok_or_else(|| format!("plug-in instance is not available: {target_id}"))
    }
    pub fn close_plugin_editor(&self, target_id: &str) -> Result<(), String> {
        self.bindings
            .plugin_controls
            .get(target_id)
            .ok_or_else(|| format!("plug-in instance is not available: {target_id}"))?
            .close_editor()
    }
    pub fn plugin_editor_is_open(&self, target_id: &str) -> bool {
        self.bindings
            .plugin_controls
            .get(target_id)
            .is_some_and(|control| control.is_editor_open())
    }
    pub fn take_plugin_editor_actions(
        &self,
        target_id: &str,
    ) -> Result<Vec<PluginEditorAction>, String> {
        Ok(self
            .bindings
            .plugin_controls
            .get(target_id)
            .ok_or_else(|| format!("plug-in instance is not available: {target_id}"))?
            .take_editor_actions())
    }
    pub fn save_plugin_state(&self, target_id: &str) -> Result<Vec<u8>, String> {
        self.bindings
            .plugin_controls
            .get(target_id)
            .ok_or_else(|| format!("plug-in instance is not available: {target_id}"))?
            .save_state()
    }
    pub fn load_plugin_state(&self, target_id: &str, state: Vec<u8>) -> Result<(), String> {
        self.bindings
            .plugin_controls
            .get(target_id)
            .ok_or_else(|| format!("plug-in instance is not available: {target_id}"))?
            .load_state(state)
    }
    fn close_all_plugin_editors(&self) {
        for control in self.bindings.plugin_controls.values() {
            if control.is_editor_open() {
                let _ = control.close_editor();
            }
        }
    }
    pub fn play(&mut self, from: Option<f64>, count_in_bars: u8) -> Result<(), String> {
        if let Some(sec) = from {
            self.push(AudioCommand::SeekTo(
                (sec * f64::from(self.settings.sample_rate)) as u64,
            ))?
        }
        self.push(AudioCommand::SetPlaying {
            playing: true,
            count_in_bars: count_in_bars.min(4),
        })
    }
    pub fn pause(&mut self) -> Result<(), String> {
        self.push(AudioCommand::SetPlaying {
            playing: false,
            count_in_bars: 0,
        })
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.push(AudioCommand::Stop)
    }
    pub fn seek(&mut self, sec: f64) -> Result<(), String> {
        self.push(AudioCommand::SeekTo(
            (sec * f64::from(self.settings.sample_rate)) as u64,
        ))
    }
    pub fn set_metronome(
        &mut self,
        enabled: bool,
        gain_db: f32,
        bpm: f64,
        numerator: u8,
        denominator: u8,
    ) -> Result<(), String> {
        self.push(AudioCommand::SetMetronome(MetronomeSettings {
            enabled,
            gain: db_to_gain(gain_db.clamp(-60.0, 6.0)),
            bpm: bpm.clamp(20.0, 400.0),
            numerator: numerator.clamp(1, 32),
            denominator: match denominator {
                1 | 2 | 4 | 8 | 16 => denominator,
                _ => 4,
            },
        }))
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
    pub fn set_effect_bypassed(&mut self, id: &str, bypassed: bool) -> Result<(), String> {
        let target = *self.bindings.effects.get(id).ok_or("unknown effect")?;
        if let Some(spec) = self
            .last_snapshot
            .as_mut()
            .and_then(|snapshot| find_effect_mut(snapshot, id))
        {
            spec.bypassed = bypassed;
        }
        self.push(AudioCommand::SetEffectBypass { target, bypassed })
    }
    /// Diagnostic global kill switch used for scheduler A/B rendering.
    pub fn set_scheduler_enabled(&mut self, enabled: bool) -> Result<(), String> {
        self.push(AudioCommand::SetSchedulerEnabled(enabled))
    }
    /// Per-node compatibility escape hatch. External plug-ins remain conservative
    /// by default; this API also lets diagnostics force a native node to run.
    pub fn set_effect_sleep_policy(&mut self, id: &str, policy: SleepPolicy) -> Result<(), String> {
        let target = *self.bindings.effects.get(id).ok_or("unknown effect")?;
        self.push(AudioCommand::SetEffectSleepPolicy { target, policy })
    }
    pub fn notify_automation_or_modulation(&mut self, modulation: bool) -> Result<(), String> {
        self.push(AudioCommand::WakeAll(if modulation {
            WakeReason::Modulation
        } else {
            WakeReason::Automation
        }))
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
    /// Drives the process-isolation idle hint from each track's measured output level, not
    /// from mute/solo flags - an instrument that is simply not being played right now (in the
    /// mix, unmuted, just quiet) still qualifies, which is exactly the common case in a large
    /// template between takes. Only instrument plug-ins are tracked here: an external effect's
    /// individual contribution is not separable from its track's post-chain level, and effect
    /// instances are rarely numerous enough for their isolation overhead to matter the way a
    /// mega-template's several dozen instrument tracks can.
    fn update_idle_hints(&mut self, track_levels: &[Level]) {
        let now = std::time::Instant::now();
        for (index, track_id) in self.bindings.track_ids.iter().enumerate() {
            let Some(control) = self.bindings.plugin_controls.get(track_id) else {
                continue;
            };
            let peak = track_levels.get(index).map_or(0.0, |level| level.peak);
            let tracker = self.idle_trackers.entry(track_id.clone()).or_default();
            if peak > IDLE_SILENCE_PEAK {
                if tracker.trimmed {
                    control.set_idle(false);
                }
                *tracker = IdleTracker::default();
            } else {
                let silent_since = *tracker.silent_since.get_or_insert(now);
                if !tracker.trimmed && now.duration_since(silent_since) >= IDLE_TRIM_AFTER {
                    control.set_idle(true);
                    tracker.trimmed = true;
                }
            }
        }
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
        let (frame, running, xruns, cpu_load, cpu_peak, realtime_metrics) =
            if let Some(runtime) = &mut self.runtime {
                runtime.meters.update();
                let xruns = runtime.xruns.load(Ordering::Relaxed);
                (
                    *runtime.meters.output_buffer_mut(),
                    true,
                    xruns,
                    f32::from_bits(runtime.cpu_load.load(Ordering::Relaxed)),
                    f32::from_bits(runtime.cpu_peak.load(Ordering::Relaxed)),
                    runtime.metrics.snapshot(xruns),
                )
            } else {
                (
                    MeterFrame::default(),
                    false,
                    0,
                    0.0,
                    0.0,
                    Default::default(),
                )
            };
        let track_levels = frame.levels[..self.bindings.track_ids.len().min(MAX_TRACKS)].to_vec();
        self.update_idle_hints(&track_levels);
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
        let limiter_metrics = frame.limiter_metrics
            [..self.bindings.limiter_meter_ids.len().min(MAX_EFFECT_METERS)]
            .iter()
            .map(|values| LimiterMetrics {
                input_peak_db: values[0],
                output_peak_db: values[1],
                gain_reduction_db: values[2],
                true_peak_db: values[3],
                momentary_lufs: values[4],
                short_term_lufs: values[5],
                integrated_lufs: values[6],
            })
            .collect();
        let plugin_deadline_misses = self
            .bindings
            .plugin_controls
            .values()
            .map(|control| control.realtime_deadline_misses())
            .sum();
        let mut plugin_parameter_changes = Vec::with_capacity(32);
        for (target_id, control) in &self.bindings.plugin_controls {
            for (parameter_id, value) in control.take_parameter_changes() {
                if plugin_parameter_changes.len() >= 1024 {
                    break;
                }
                plugin_parameter_changes.push(PluginParameterChange {
                    target_id: target_id.clone(),
                    parameter_id,
                    value,
                });
            }
        }
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
                cpu_load_percent: f64::from(cpu_load.clamp(0.0, 8.0)) * 100.0,
                cpu_peak_percent: f64::from(cpu_peak.clamp(0.0, 8.0)) * 100.0,
                count_in_beats_remaining: frame.count_in_beats_remaining,
                callback_p50_ms: realtime_metrics.callback_p50_ns as f64 / 1_000_000.0,
                callback_p95_ms: realtime_metrics.callback_p95_ns as f64 / 1_000_000.0,
                callback_p99_ms: realtime_metrics.callback_p99_ns as f64 / 1_000_000.0,
                callback_max_ms: realtime_metrics.callback_max_ns as f64 / 1_000_000.0,
                command_queue_high_water: realtime_metrics.command_high_water,
                command_queue_overflow: realtime_metrics.command_overflow,
                plugin_deadline_misses,
                scheduler_running_nodes: frame.scheduler.running_nodes,
                scheduler_tail_nodes: frame.scheduler.tail_nodes,
                scheduler_sleeping_nodes: frame.scheduler.sleeping_nodes,
                scheduler_total_nodes: frame.scheduler.total_nodes,
                scheduler_skipped_process_calls: frame.scheduler.skipped_process_calls,
                scheduler_wake_count: frame.scheduler.wake_count,
                scheduler_sleep_count: frame.scheduler.sleep_count,
            },
            graph_revision: self.graph_revision,
            playing: frame.playing,
            active_voice_counts: frame.active_voice_counts
                [..self.bindings.track_ids.len().min(MAX_TRACKS)]
                .to_vec(),
            multiband_levels,
            distortion_spectra,
            limiter_metrics,
            plugin_parameter_changes,
        }
    }
    pub fn backends(&self) -> Vec<AudioBackendInfo> {
        device::list_backends()
    }
    pub fn devices(&self, id: &str) -> Result<Vec<AudioDeviceInfo>, String> {
        device::list_devices(id)
    }
    /// A cheap snapshot used by the Tauri layer before it releases the global engine lock and
    /// performs the potentially blocking OS enumeration on the blocking pool.
    pub fn midi_route_snapshot(&self) -> HashMap<String, String> {
        self.midi_service.route_snapshot()
    }

    pub fn scan_midi_inputs(
        connected_routes: &HashMap<String, String>,
    ) -> Result<Vec<MidiInputPortInfo>, String> {
        MidiService::scan_inputs(connected_routes)
    }
    /// Resolves a route while the engine lock is held, but never talks to the operating system.
    /// Existing hardware connections are retargeted with one atomic store.
    pub fn prepare_midi_input(
        &mut self,
        port_id: &str,
        track_id: &str,
    ) -> Result<MidiConnectPlan, String> {
        let track = *self
            .bindings
            .tracks
            .get(track_id)
            .ok_or("unknown MIDI target track")?;
        Ok(self.midi_service.prepare_input(port_id, track_id, track))
    }

    /// Performs the potentially slow MidiSrv work without borrowing `NativeEngine` or holding its
    /// global mutex. The returned connection is installed in a short second control-plane step.
    pub fn open_midi_input(request: MidiInputRouteRequest) -> Result<OpenedMidiInputRoute, String> {
        MidiService::open_input(request)
    }

    pub fn install_midi_input(
        &mut self,
        opened: OpenedMidiInputRoute,
    ) -> Option<DetachedMidiInputRoute> {
        let target = self.bindings.tracks.get(opened.target_track_id()).copied();
        self.midi_service.install_input(opened, target)
    }

    pub fn detach_midi_input(&mut self, port_id: &str) -> Option<DetachedMidiInputRoute> {
        self.midi_service.detach_input(port_id)
    }

    pub fn release_midi_input(route: DetachedMidiInputRoute) {
        MidiService::release_input(route);
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
        if !matches!(request.format.as_str(), "wav" | "mp3") {
            return Err("export format must be wav or mp3".into());
        }
        if request.sample_rate != self.settings.sample_rate {
            return Err("export sample rate must match the loaded project sample rate".into());
        }
        let (graph, _) = AudioGraph::build(spec, &self.assets, request.sample_rate)?;
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
            let (mut analysis, _) = AudioGraph::build(spec, &self.assets, request.sample_rate)?;
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
        let mut sink = ExportSink::new(request)?;
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
                sink.finalize()?;
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
            for value in &mut output[..frames * 2] {
                *value *= normalization_gain;
                peak = peak.max(value.abs());
                clipped |= value.abs() > 1.0;
            }
            sink.write_samples(&output[..frames * 2])?;
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
        sink.finalize()?;
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
        EffectSpec, InstrumentSpec, LoopSpec, MasterSpec, MidiClipSpec, MidiNoteSpec, TrackSpec,
        TransportSpec,
    };
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        thread,
        time::Duration,
    };

    struct TrackingAllocator;

    thread_local! {
        static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
        static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
        static DEALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    #[global_allocator]
    static TEST_ALLOCATOR: TrackingAllocator = TrackingAllocator;

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            TRACK_ALLOCATIONS.with(|enabled| {
                if enabled.get() {
                    ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            TRACK_ALLOCATIONS.with(|enabled| {
                if enabled.get() {
                    DEALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    fn track_allocations<T>(callback: impl FnOnce() -> T) -> (T, usize, usize) {
        ALLOCATION_COUNT.with(|count| count.set(0));
        DEALLOCATION_COUNT.with(|count| count.set(0));
        TRACK_ALLOCATIONS.with(|enabled| enabled.set(true));
        let result = callback();
        TRACK_ALLOCATIONS.with(|enabled| enabled.set(false));
        let allocations = ALLOCATION_COUNT.with(Cell::get);
        let deallocations = DEALLOCATION_COUNT.with(Cell::get);
        (result, allocations, deallocations)
    }

    fn metronome_test_core() -> (AudioCore, Producer<AudioCommand>, Output<MeterFrame>) {
        let (command_tx, command_rx) = RingBuffer::new(16);
        let (retired_tx, _) = RingBuffer::new(1);
        let (meter_tx, meter_rx) = triple_buffer(&MeterFrame::default());
        let core = AudioCore::new(
            command_rx,
            retired_tx,
            AudioGraph::empty(48_000),
            meter_tx,
            Arc::new(ArrayQueue::new(16)),
        );
        (core, command_tx, meter_rx)
    }

    #[test]
    fn count_in_clicks_without_advancing_the_timeline() {
        let (mut core, mut commands, mut meters) = metronome_test_core();
        commands
            .push(AudioCommand::SetMetronome(MetronomeSettings {
                enabled: false,
                gain: db_to_gain(-12.0),
                bpm: 120.0,
                numerator: 4,
                denominator: 4,
            }))
            .expect("metronome settings should enqueue");
        commands
            .push(AudioCommand::SetPlaying {
                playing: true,
                count_in_bars: 1,
            })
            .expect("count-in should enqueue");

        let mut output = vec![0.0; 512 * 2];
        core.render(&mut output, 512);
        meters.update();
        let frame = *meters.output_buffer_mut();
        assert_eq!(frame.position, 0, "count-in must hold the project clock");
        assert_eq!(frame.count_in_beats_remaining, 4);
        assert!(output.iter().any(|sample| sample.abs() > 0.001));

        for _ in 0..188 {
            core.render(&mut output, 512);
        }
        core.render(&mut output, 512);
        meters.update();
        let frame = *meters.output_buffer_mut();
        assert!(frame.position > 0, "timeline must start after the count-in");
        assert_eq!(frame.count_in_beats_remaining, 0);
    }

    #[test]
    fn full_render_call_graph_is_allocation_and_deallocation_free() {
        let graph_spec = GraphSnapshot {
            tracks: vec![TrackSpec {
                id: "rt-allocation-track".into(),
                kind: "instrument".into(),
                name: "RT allocation probe".into(),
                clips: Vec::new(),
                midi_clips: Vec::new(),
                instrument: Some(InstrumentSpec {
                    id: "rt-allocation-synth".into(),
                    kind: "builtin:testtone".into(),
                    plugin: None,
                    params: HashMap::new(),
                    bypassed: false,
                }),
                volume_db: 0.0,
                pan: 0.0,
                muted: false,
                solo: false,
                effects: vec![
                    EffectSpec {
                        id: "rt-utility".into(),
                        kind: "builtin:utility".into(),
                        bypassed: false,
                        plugin: None,
                        sidechain: None,
                        params: HashMap::new(),
                    },
                    EffectSpec {
                        id: "rt-clipper".into(),
                        kind: "builtin:clipper".into(),
                        bypassed: false,
                        plugin: None,
                        sidechain: None,
                        params: HashMap::new(),
                    },
                    EffectSpec {
                        id: "rt-colorizer".into(),
                        kind: "builtin:resonator".into(),
                        bypassed: false,
                        plugin: None,
                        sidechain: None,
                        params: HashMap::from([
                            ("quality".into(), 1.0),
                            ("color".into(), 1.0),
                            ("morph".into(), 0.72),
                            ("gate".into(), 0.2),
                            ("midi".into(), 1.0),
                            ("pitch0".into(), 1.0),
                            ("pitch1".into(), 0.0),
                            ("pitch2".into(), 0.0),
                            ("pitch3".into(), 0.0),
                            ("pitch4".into(), 0.0),
                            ("pitch5".into(), 0.0),
                            ("pitch6".into(), 0.0),
                            ("pitch7".into(), 0.0),
                            ("pitch8".into(), 0.0),
                            ("pitch9".into(), 0.0),
                            ("pitch10".into(), 0.0),
                            ("pitch11".into(), 0.0),
                        ]),
                    },
                ],
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
                is_playing: false,
                loop_: LoopSpec {
                    enabled: false,
                    start_sec: 0.0,
                    end_sec: 0.0,
                },
            },
        };
        let (graph, _) = AudioGraph::build(&graph_spec, &HashMap::new(), 48_000).unwrap();
        let (mut command_tx, command_rx) = RingBuffer::new(128);
        let (retired_tx, _) = RingBuffer::new(1);
        let (meter_tx, _) = triple_buffer(&MeterFrame::default());
        let mut core = AudioCore::new(
            command_rx,
            retired_tx,
            graph,
            meter_tx,
            Arc::new(ArrayQueue::new(16)),
        );
        let mut output = [0.0; 512];
        core.render(&mut output, 256);
        let (_, allocations, deallocations) = track_allocations(|| {
            for index in 0..32 {
                assert!(command_tx
                    .push(AudioCommand::LiveMidi {
                        track: 0,
                        event: NoteEvent {
                            sample_offset: (index % 16) as u32,
                            kind: if index == 0 {
                                NoteEventKind::NoteOn {
                                    note_id: 91,
                                    pitch: 60,
                                    velocity: 0.8,
                                    tuning_cents: 0.0,
                                }
                            } else {
                                NoteEventKind::Controller {
                                    cc: 1,
                                    value: index as f32 / 31.0,
                                }
                            },
                        },
                    })
                    .is_ok());
                core.render(&mut output, 256);
            }
        });
        drop(command_tx);
        assert_eq!(
            allocations, 0,
            "AudioCore::render allocated on its call graph"
        );
        assert_eq!(
            deallocations, 0,
            "AudioCore::render deallocated on its call graph"
        );
    }

    #[test]
    fn validates_supported_mp3_bitrates() {
        assert!(matches!(mp3_bitrate(128), Ok(Bitrate::Kbps128)));
        assert!(matches!(mp3_bitrate(320), Ok(Bitrate::Kbps320)));
        assert!(mp3_bitrate(160).is_err());
    }

    #[test]
    fn mp3_sink_writes_a_decodable_frame_stream() {
        let path = std::env::temp_dir().join(format!(
            "ministudio-mp3-smoke-{}-{}.mp3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let request = ExportRequest {
            output_path: path.to_string_lossy().into_owned(),
            format: "mp3".into(),
            bit_depth: 24,
            mp3_bitrate_kbps: 192,
            sample_rate: 48_000,
            normalize: false,
        };
        let mut sink = ExportSink::new(&request).expect("MP3 encoder should initialize");
        sink.write_samples(&vec![0.0; 4_800 * 2])
            .expect("silence should encode");
        sink.finalize().expect("MP3 stream should flush");
        let encoded = std::fs::read(&path).expect("MP3 output should exist");
        assert!(encoded.len() > 1_000);
        assert!(encoded
            .windows(2)
            .any(|bytes| bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0));
        std::fs::remove_file(path).expect("temporary MP3 should be removable");
    }

    #[test]
    #[ignore = "opens the machine's real default audio output"]
    fn native_play_advances_the_audio_clock() {
        let mut engine = NativeEngine::default();
        engine.init().expect("default audio stream should open");
        engine
            .play(Some(0.0), 0)
            .expect("play command should enqueue");
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
                        cc_lanes: Vec::new(),
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
                    tempo_points: Vec::new(),
                    time_signatures: Vec::new(),
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
        engine
            .play(Some(0.0), 0)
            .expect("play command should enqueue");
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
