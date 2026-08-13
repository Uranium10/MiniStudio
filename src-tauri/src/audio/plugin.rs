//! External VST3/CLAP discovery and realtime adapters.
use super::{
    dsp::{AudioBuffer, DspEffect},
    instrument::{Instrument, NoteEvent, NoteEventKind},
    types::{EffectSpec, InstrumentSpec},
    MAX_BLOCK_SIZE, MAX_CHANNELS,
};
use clack_extensions::{
    audio_ports::{AudioPortFlags, AudioPortInfoBuffer, PluginAudioPorts},
    params::{ParamInfoBuffer, PluginParams},
};
use clack_host::utils::Cookie;
use clack_host::{
    events::{
        event_types::{MidiEvent as ClapMidiEvent, ParamValueEvent as ClapParamValueEvent},
        io::{EventBuffer, InputEvents, OutputEvents},
    },
    prelude::{
        AudioPortBuffer, AudioPortBufferType, AudioPorts, ClapId, HostInfo, InputAudioBuffers,
        InputChannel, Pckn, PluginAudioConfiguration, PluginEntry, PluginInstance,
        StartedPluginAudioProcessor, StoppedPluginAudioProcessor,
    },
};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::{
    collections::HashSet,
    ffi::{CString, OsStr},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{sync_channel, SyncSender},
};
use vst3_host::{
    BusAudioBuffers, BusDirection, MediaType, MidiChannel, MidiEvent, Plugin, Vst3Host,
};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginDescriptor {
    pub format: String,
    pub uid: String,
    pub name: String,
    pub vendor: String,
    pub category: String,
    pub path: String,
    pub is_instrument: bool,
    pub param_count: u32,
    pub has_editor: bool,
    pub audio_input_buses: u32,
    pub audio_output_buses: u32,
    pub supports_sidechain: bool,
    pub parameters: Vec<PluginParameterDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginParameterDescriptor {
    pub id: String,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default_value: f64,
}

pub fn default_plugin_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "windows")]
    {
        paths.push(PathBuf::from(r"C:\Program Files\Common Files\VST3"));
        paths.push(PathBuf::from(r"C:\Program Files\Common Files\CLAP"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            paths.push(PathBuf::from(&local).join("Programs/Common/VST3"));
            paths.push(PathBuf::from(local).join("Programs/Common/CLAP"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        paths.extend([
            PathBuf::from("/Library/Audio/Plug-Ins/VST3"),
            PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
        ]);
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(&home).join("Library/Audio/Plug-Ins/VST3"));
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        paths.extend([
            PathBuf::from("/usr/lib/vst3"),
            PathBuf::from("/usr/local/lib/vst3"),
            PathBuf::from("/usr/lib/clap"),
            PathBuf::from("/usr/local/lib/clap"),
        ]);
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(&home).join(".vst3"));
            paths.push(PathBuf::from(home).join(".clap"));
        }
    }
    paths
}

pub fn collect_plugin_binaries(custom_paths: &[String]) -> Vec<(String, PathBuf)> {
    let mut roots = default_plugin_paths();
    roots.extend(custom_paths.iter().map(PathBuf::from));
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        collect_from_path(&root, &mut found, &mut seen);
    }
    found
}

fn collect_from_path(path: &Path, found: &mut Vec<(String, PathBuf)>, seen: &mut HashSet<PathBuf>) {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "vst3" || extension == "clap" {
        let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if seen.insert(normalized.clone()) {
            found.push((extension, normalized));
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_from_path(&entry.path(), found, seen);
    }
}

pub fn probe_plugin(format: &str, path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    match format {
        "vst3" => probe_vst3(path),
        "clap" => probe_clap(path),
        _ => Err(format!("unsupported plugin format: {format}")),
    }
}

fn probe_vst3(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let detailed = vst3_host::get_detailed_plugin_info(path).map_err(|error| error.to_string())?;
    let audio_input_buses = detailed.buses.audio_inputs.len() as u32;
    let audio_output_buses = detailed.buses.audio_outputs.len() as u32;
    let supports_sidechain = detailed.buses.audio_inputs.len() > 1
        || detailed
            .buses
            .audio_inputs
            .iter()
            .any(|bus| bus.bus_type != 0);
    let info = detailed.info;
    let category_lower = info.category.to_ascii_lowercase();
    let is_instrument = category_lower.contains("instrument") || category_lower.contains("synth");
    let parameters = Vst3Host::builder()
        .sample_rate(48_000.0)
        .block_size(256)
        .with_process_isolation(false)
        .build()
        .and_then(|mut host| host.load_plugin_class(path, &info.uid))
        .and_then(|plugin| plugin.get_parameters())
        .map(|params| {
            params
                .into_iter()
                .filter(|parameter| parameter.can_automate && !parameter.is_read_only)
                .map(|parameter| PluginParameterDescriptor {
                    id: parameter.id.to_string(),
                    name: parameter.name,
                    module: String::new(),
                    min: parameter.min,
                    max: parameter.max,
                    default_value: parameter.default,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let param_count = parameters.len() as u32;
    Ok(vec![PluginDescriptor {
        format: "vst3".into(),
        uid: info.uid,
        name: info.name,
        vendor: info.vendor,
        category: info.category,
        path: path.to_string_lossy().into_owned(),
        is_instrument,
        param_count,
        has_editor: info.has_gui,
        audio_input_buses,
        audio_output_buses,
        supports_sidechain,
        parameters,
    }])
}

fn probe_clap(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    use clack_host::prelude::PluginEntry;
    // Loading arbitrary native modules is inherently unsafe; callers run this in a disposable probe process.
    let entry =
        unsafe { PluginEntry::load(path.as_os_str()) }.map_err(|error| error.to_string())?;
    let factory = entry
        .get_plugin_factory()
        .ok_or("CLAP entry has no plugin factory")?;
    Ok(factory
        .plugin_descriptors()
        .filter_map(|descriptor| {
            let uid = descriptor.id()?.to_string_lossy().into_owned();
            let name = descriptor.name()?.to_string_lossy().into_owned();
            let vendor = descriptor
                .vendor()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default();
            let features: Vec<String> = descriptor
                .features()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let is_instrument = features
                .iter()
                .any(|v| v == "instrument" || v == "synthesizer" || v == "sampler");
            let (audio_input_buses, audio_output_buses, supports_sidechain, parameters) =
                inspect_clap_metadata(&entry, &uid).unwrap_or((0, 0, false, Vec::new()));
            Some(PluginDescriptor {
                format: "clap".into(),
                uid,
                name,
                vendor,
                category: features.first().cloned().unwrap_or_else(|| "CLAP".into()),
                path: path.to_string_lossy().into_owned(),
                is_instrument,
                param_count: parameters.len() as u32,
                has_editor: false,
                audio_input_buses,
                audio_output_buses,
                supports_sidechain,
                parameters,
            })
        })
        .collect())
}

#[derive(Clone, Copy)]
struct ClapPortLayout {
    channels: usize,
    is_main: bool,
}

fn query_clap_ports(instance: &mut PluginInstance<()>, is_input: bool) -> Vec<ClapPortLayout> {
    let mut plugin = instance.plugin_handle();
    let Some(extension) = plugin.get_extension::<PluginAudioPorts>() else {
        return Vec::new();
    };
    let count = extension.count(&mut plugin, is_input).min(16);
    let mut ports = Vec::with_capacity(count as usize);
    for index in 0..count {
        let mut buffer = AudioPortInfoBuffer::new();
        if let Some(info) = extension.get(&mut plugin, index, is_input, &mut buffer) {
            ports.push(ClapPortLayout {
                channels: info.channel_count.clamp(1, 16) as usize,
                is_main: info.flags.contains(AudioPortFlags::IS_MAIN),
            });
        }
    }
    ports
}

fn inspect_clap_metadata(
    entry: &PluginEntry,
    uid: &str,
) -> Result<(u32, u32, bool, Vec<PluginParameterDescriptor>), String> {
    let uid = CString::new(uid).map_err(|_| "CLAP plugin ID contains a NUL byte")?;
    let host_info = HostInfo::new(
        "MiniDAW Probe",
        "MiniDAW",
        "https://github.com",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|error| error.to_string())?;
    let mut instance = PluginInstance::<()>::new(|_| (), |_| (), entry, &uid, &host_info)
        .map_err(|error| error.to_string())?;
    let inputs = query_clap_ports(&mut instance, true);
    let outputs = query_clap_ports(&mut instance, false);
    let supports_sidechain = inputs.len() > 1 || inputs.iter().skip(1).any(|port| !port.is_main);
    let mut parameters = Vec::new();
    let mut plugin = instance.plugin_handle();
    if let Some(extension) = plugin.get_extension::<PluginParams>() {
        let count = extension.count(&mut plugin).min(16_384);
        for index in 0..count {
            let mut buffer = ParamInfoBuffer::new();
            if let Some(info) = extension.get_info(&mut plugin, index, &mut buffer) {
                parameters.push(PluginParameterDescriptor {
                    id: info.id.get().to_string(),
                    name: String::from_utf8_lossy(info.name).into_owned(),
                    module: String::from_utf8_lossy(info.module).into_owned(),
                    min: info.min_value,
                    max: info.max_value,
                    default_value: info.default_value,
                });
            }
        }
    }
    Ok((
        inputs.len() as u32,
        outputs.len() as u32,
        supports_sidechain,
        parameters,
    ))
}

fn load_vst3(path: &str, uid: &str, sample_rate: f32) -> Result<Plugin, String> {
    let mut host = Vst3Host::builder()
        .sample_rate(sample_rate as f64)
        .block_size(MAX_BLOCK_SIZE)
        .input_channels(MAX_CHANNELS)
        .output_channels(MAX_CHANNELS)
        .with_process_isolation(false)
        .build()
        .map_err(|error| error.to_string())?;
    let mut plugin = host
        .load_plugin_class(path, uid)
        .map_err(|error| error.to_string())?;
    if let Ok(layout) = plugin.audio_bus_layout() {
        for (index, bus) in layout.inputs.iter().enumerate() {
            if !bus.active {
                let _ = plugin.set_bus_active(
                    MediaType::Audio,
                    BusDirection::Input,
                    index as i32,
                    true,
                );
            }
        }
        for (index, bus) in layout.outputs.iter().enumerate() {
            if !bus.active {
                let _ = plugin.set_bus_active(
                    MediaType::Audio,
                    BusDirection::Output,
                    index as i32,
                    true,
                );
            }
        }
    }
    plugin
        .start_processing()
        .map_err(|error| error.to_string())?;
    Ok(plugin)
}

struct Vst3Effect {
    plugin: Plugin,
    buffers: BusAudioBuffers,
    bypassed: bool,
    latency: usize,
    tail: usize,
}

impl Vst3Effect {
    fn new(spec: &EffectSpec, sample_rate: f32) -> Result<Self, String> {
        let reference = spec
            .plugin
            .as_ref()
            .ok_or("external effect is missing its plugin reference")?;
        let plugin = load_vst3(&reference.path, &reference.uid, sample_rate)?;
        let latency = plugin.latency_samples() as usize;
        let tail = plugin.tail_samples() as usize;
        let buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            plugin,
            buffers,
            bypassed: spec.bypassed,
            latency,
            tail,
        })
    }
}

impl DspEffect for Vst3Effect {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.buffers.sample_rate = sample_rate as f64;
    }
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
        self.process_with_sidechain(buffer, None, frames)
    }
    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        if self.bypassed {
            return;
        }
        let frames = frames.min(MAX_BLOCK_SIZE);
        self.buffers.block_size = frames;
        self.buffers.clear();
        if let Some(main) = self.buffers.inputs.first_mut() {
            copy_stereo_to_vst_bus(main, buffer, frames);
        }
        if let Some(sidechain) = sidechain {
            for aux in self
                .buffers
                .inputs
                .iter_mut()
                .skip(1)
                .filter(|bus| bus.active)
            {
                copy_stereo_to_vst_bus(aux, sidechain, frames);
            }
        }
        if self.plugin.process_bus_audio(&mut self.buffers).is_ok() {
            if let Some(main) = self.buffers.outputs.first() {
                copy_vst_bus_to_stereo(main, buffer, frames, false);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        if let Some(id) = id
            .strip_prefix("param:")
            .and_then(|id| id.parse::<u32>().ok())
        {
            let _ = self.plugin.set_parameter(id, value.clamp(0.0, 1.0) as f64);
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }
    fn reset(&mut self) {
        let _ = self.plugin.stop_processing();
        let _ = self.plugin.start_processing();
    }
    fn tail_samples(&self) -> usize {
        self.tail
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
}

struct Vst3Instrument {
    plugin: Plugin,
    buffers: BusAudioBuffers,
    active_notes: HashSet<i32>,
    tail: usize,
}

impl Vst3Instrument {
    fn new(spec: &InstrumentSpec, sample_rate: f32) -> Result<Self, String> {
        let reference = spec
            .plugin
            .as_ref()
            .ok_or("external instrument is missing its plugin reference")?;
        let plugin = load_vst3(&reference.path, &reference.uid, sample_rate)?;
        let tail = plugin.tail_samples() as usize;
        let buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            plugin,
            buffers,
            active_notes: HashSet::with_capacity(64),
            tail,
        })
    }
    fn send_event(&mut self, event: &NoteEvent) {
        let midi = match event.kind {
            NoteEventKind::NoteOn {
                note_id,
                pitch,
                velocity,
                ..
            } => {
                self.active_notes.insert(note_id);
                MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: pitch,
                    velocity: (velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
                }
            }
            NoteEventKind::NoteOff {
                note_id,
                pitch,
                velocity,
            } => {
                self.active_notes.remove(&note_id);
                MidiEvent::NoteOff {
                    channel: MidiChannel::Ch1,
                    note: pitch,
                    velocity: (velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
                }
            }
            NoteEventKind::PolyPressure {
                pitch, pressure, ..
            } => MidiEvent::PolyAftertouch {
                channel: MidiChannel::Ch1,
                note: pitch,
                pressure: (pressure.clamp(0.0, 1.0) * 127.0).round() as u8,
            },
            NoteEventKind::Controller { cc, value } => MidiEvent::ControlChange {
                channel: MidiChannel::Ch1,
                controller: cc,
                value: (value.clamp(0.0, 1.0) * 127.0).round() as u8,
            },
            NoteEventKind::PitchBend { value } => MidiEvent::PitchBend {
                channel: MidiChannel::Ch1,
                value: ((value.clamp(-1.0, 1.0) + 1.0) * 8191.5).round() as u16,
            },
            NoteEventKind::AllNotesOff => {
                self.active_notes.clear();
                MidiEvent::ControlChange {
                    channel: MidiChannel::Ch1,
                    controller: 123,
                    value: 0,
                }
            }
        };
        let _ = self
            .plugin
            .send_midi_event_at(midi, event.sample_offset as i32);
    }
}

impl Instrument for Vst3Instrument {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.buffers.sample_rate = sample_rate as f64;
    }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize) {
        let frames = frames.min(MAX_BLOCK_SIZE);
        self.buffers.block_size = frames;
        for event in events {
            self.send_event(event);
        }
        self.buffers.clear();
        if self.plugin.process_bus_audio(&mut self.buffers).is_ok() {
            if let Some(main) = self.buffers.outputs.first() {
                copy_vst_bus_to_stereo(main, out, frames, true);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        if let Some(id) = id
            .strip_prefix("param:")
            .and_then(|id| id.parse::<u32>().ok())
        {
            let _ = self.plugin.set_parameter(id, value.clamp(0.0, 1.0) as f64);
        }
    }
    fn reset(&mut self) {
        self.active_notes.clear();
        let _ = self.plugin.stop_processing();
        let _ = self.plugin.start_processing();
    }
    fn tail_samples(&self) -> usize {
        self.tail
    }
    fn active_voice_count(&self) -> usize {
        self.active_notes.len()
    }
}

fn copy_stereo_to_vst_bus(
    bus: &mut vst3_host::AudioBusBuffer,
    source: &AudioBuffer,
    frames: usize,
) {
    if !bus.active || bus.channels.is_empty() {
        return;
    }
    if bus.channels.len() == 1 {
        for sample in 0..frames {
            bus.channels[0][sample] =
                (source.channels[0][sample] + source.channels[1][sample]) * 0.5;
        }
        return;
    }
    bus.channels[0][..frames].copy_from_slice(&source.channels[0][..frames]);
    bus.channels[1][..frames].copy_from_slice(&source.channels[1][..frames]);
}

fn copy_vst_bus_to_stereo(
    bus: &vst3_host::AudioBusBuffer,
    target: &mut AudioBuffer,
    frames: usize,
    add: bool,
) {
    if !bus.active || bus.channels.is_empty() {
        return;
    }
    for sample in 0..frames {
        let left = bus.channels[0][sample];
        let right = bus.channels.get(1).map_or(left, |channel| channel[sample]);
        if add {
            target.channels[0][sample] += left;
            target.channels[1][sample] += right;
        } else {
            target.channels[0][sample] = left;
            target.channels[1][sample] = right;
        }
    }
}

pub fn create_external_effect(spec: &EffectSpec, sample_rate: f32) -> Option<Box<dyn DspEffect>> {
    match spec.plugin.as_ref()?.format.as_str() {
        "vst3" => Vst3Effect::new(spec, sample_rate)
            .ok()
            .map(|effect| Box::new(effect) as Box<dyn DspEffect>),
        "clap" => ClapEffect::new(spec, sample_rate)
            .ok()
            .map(|effect| Box::new(effect) as Box<dyn DspEffect>),
        _ => None,
    }
}

pub fn create_external_instrument(
    spec: &InstrumentSpec,
    sample_rate: f32,
) -> Option<Box<dyn Instrument>> {
    match spec.plugin.as_ref()?.format.as_str() {
        "vst3" => Vst3Instrument::new(spec, sample_rate)
            .ok()
            .map(|instrument| Box::new(instrument) as Box<dyn Instrument>),
        "clap" => ClapInstrument::new(spec, sample_rate)
            .ok()
            .map(|instrument| Box::new(instrument) as Box<dyn Instrument>),
        _ => None,
    }
}

struct ClapRuntime {
    processor: Option<StartedPluginAudioProcessor<()>>,
    owner_return: SyncSender<StoppedPluginAudioProcessor<()>>,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    input_data: Vec<Vec<Vec<f32>>>,
    output_data: Vec<Vec<Vec<f32>>>,
    main_input: Option<usize>,
    main_output: Option<usize>,
    input_events: EventBuffer,
}

struct ClapReady {
    processor: StartedPluginAudioProcessor<()>,
    inputs: Vec<ClapPortLayout>,
    outputs: Vec<ClapPortLayout>,
}

impl ClapRuntime {
    fn new(path: &str, uid: &str, sample_rate: f32, expects_input: bool) -> Result<Self, String> {
        let path = path.to_owned();
        let uid = uid.to_owned();
        let (ready_tx, ready_rx) = sync_channel::<Result<ClapReady, String>>(1);
        let (owner_return, owner_rx) = sync_channel::<StoppedPluginAudioProcessor<()>>(1);
        std::thread::Builder::new()
            .name("minidaw-clap-owner".into())
            .spawn(move || {
                let result = (|| -> Result<(PluginEntry, PluginInstance<()>, StartedPluginAudioProcessor<()>, Vec<ClapPortLayout>, Vec<ClapPortLayout>), String> {
                    let entry = unsafe { PluginEntry::load(OsStr::new(&path)) }
                        .map_err(|error| error.to_string())?;
                    let uid = CString::new(uid).map_err(|_| "CLAP plugin ID contains a NUL byte")?;
                    let host_info = HostInfo::new(
                        "MiniDAW",
                        "MiniDAW",
                        "https://github.com",
                        env!("CARGO_PKG_VERSION"),
                    )
                    .map_err(|error| error.to_string())?;
                    let mut instance = PluginInstance::<()>::new(
                        |_| (),
                        |_| (),
                        &entry,
                        &uid,
                        &host_info,
                    )
                    .map_err(|error| error.to_string())?;
                    let mut inputs = query_clap_ports(&mut instance, true);
                    let mut outputs = query_clap_ports(&mut instance, false);
                    if expects_input && inputs.is_empty() {
                        inputs.push(ClapPortLayout { channels: 2, is_main: true });
                    }
                    if outputs.is_empty() {
                        outputs.push(ClapPortLayout { channels: 2, is_main: true });
                    }
                    let stopped = instance
                        .activate(
                            |_, _| (),
                            PluginAudioConfiguration {
                                sample_rate: sample_rate as f64,
                                min_frames_count: 1,
                                max_frames_count: MAX_BLOCK_SIZE as u32,
                            },
                        )
                        .map_err(|error| error.to_string())?;
                    let processor = stopped
                        .start_processing()
                        .map_err(|error| error.to_string())?;
                    Ok((entry, instance, processor, inputs, outputs))
                })();
                match result {
                    Ok((_entry, mut instance, processor, inputs, outputs)) => {
                        if ready_tx.send(Ok(ClapReady { processor, inputs, outputs })).is_ok() {
                            if let Ok(stopped) = owner_rx.recv() {
                                instance.deactivate(stopped);
                            }
                        }
                    }
                    Err(error) => { let _ = ready_tx.send(Err(error)); }
                }
            })
            .map_err(|error| error.to_string())?;
        let ready = ready_rx.recv().map_err(|error| error.to_string())??;
        let main_input = ready
            .inputs
            .iter()
            .position(|port| port.is_main)
            .or((!ready.inputs.is_empty()).then_some(0));
        let main_output = ready
            .outputs
            .iter()
            .position(|port| port.is_main)
            .or((!ready.outputs.is_empty()).then_some(0));
        let input_channels = ready.inputs.iter().map(|port| port.channels).sum();
        let output_channels = ready.outputs.iter().map(|port| port.channels).sum();
        let input_data = ready
            .inputs
            .iter()
            .map(|port| {
                (0..port.channels)
                    .map(|_| vec![0.0; MAX_BLOCK_SIZE])
                    .collect()
            })
            .collect();
        let output_data = ready
            .outputs
            .iter()
            .map(|port| {
                (0..port.channels)
                    .map(|_| vec![0.0; MAX_BLOCK_SIZE])
                    .collect()
            })
            .collect();
        Ok(Self {
            processor: Some(ready.processor),
            owner_return,
            input_ports: AudioPorts::with_capacity(input_channels, ready.inputs.len()),
            output_ports: AudioPorts::with_capacity(output_channels, ready.outputs.len()),
            input_data,
            output_data,
            main_input,
            main_output,
            input_events: EventBuffer::with_capacity(128),
        })
    }

    fn clear_events(&mut self) {
        self.input_events.clear();
    }

    fn push_midi(&mut self, time: u32, data: [u8; 3]) {
        self.input_events.push(&ClapMidiEvent::new(time, 0, data));
    }

    fn push_parameter(&mut self, id: u32, value: f32) {
        if let Some(id) = ClapId::from_raw(id) {
            self.input_events.push(&ClapParamValueEvent::new(
                0,
                id,
                Pckn::match_all(),
                value as f64,
                Cookie::empty(),
            ));
        }
    }

    fn process(
        &mut self,
        input: Option<&AudioBuffer>,
        sidechain: Option<&AudioBuffer>,
        out: &mut AudioBuffer,
        frames: usize,
        add: bool,
    ) {
        let frames = frames.min(MAX_BLOCK_SIZE);
        for port in &mut self.input_data {
            for channel in port {
                channel[..frames].fill(0.0);
            }
        }
        for port in &mut self.output_data {
            for channel in port {
                channel[..frames].fill(0.0);
            }
        }
        if let (Some(source), Some(index)) = (input, self.main_input) {
            copy_stereo_to_clap_port(&mut self.input_data[index], source, frames);
        }
        if let Some(source) = sidechain {
            for (index, port) in self.input_data.iter_mut().enumerate() {
                if Some(index) != self.main_input {
                    copy_stereo_to_clap_port(port, source, frames);
                }
            }
        }
        let input_events = InputEvents::from_buffer(&self.input_events);
        let mut output_events = OutputEvents::void();
        let input_audio = if !self.input_data.is_empty() {
            self.input_ports
                .with_input_buffers(self.input_data.iter_mut().map(|port| {
                    AudioPortBuffer {
                        latency: 0,
                        channels: AudioPortBufferType::f32_input_only(
                            port.iter_mut()
                                .map(|channel| InputChannel::variable(&mut channel[..frames])),
                        ),
                    }
                }))
        } else {
            InputAudioBuffers::empty()
        };
        let mut output_audio =
            self.output_ports
                .with_output_buffers(self.output_data.iter_mut().map(|port| AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        port.iter_mut().map(|channel| &mut channel[..frames]),
                    ),
                }));
        if let Some(processor) = self.processor.as_mut() {
            if processor
                .process(
                    &input_audio,
                    &mut output_audio,
                    &input_events,
                    &mut output_events,
                    None,
                    None,
                )
                .is_ok()
            {
                if let Some(index) = self.main_output {
                    copy_clap_port_to_stereo(&self.output_data[index], out, frames, add);
                }
            }
        }
        self.input_events.clear();
    }

    fn reset(&mut self) {
        if let Some(processor) = self.processor.as_mut() {
            processor.reset();
        }
        self.input_events.clear();
    }
}

impl Drop for ClapRuntime {
    fn drop(&mut self) {
        if let Some(processor) = self.processor.take() {
            let _ = self.owner_return.send(processor.stop_processing());
        }
    }
}

fn copy_stereo_to_clap_port(port: &mut [Vec<f32>], source: &AudioBuffer, frames: usize) {
    if port.is_empty() {
        return;
    }
    if port.len() == 1 {
        for sample in 0..frames {
            port[0][sample] = (source.channels[0][sample] + source.channels[1][sample]) * 0.5;
        }
        return;
    }
    port[0][..frames].copy_from_slice(&source.channels[0][..frames]);
    port[1][..frames].copy_from_slice(&source.channels[1][..frames]);
}

fn copy_clap_port_to_stereo(port: &[Vec<f32>], target: &mut AudioBuffer, frames: usize, add: bool) {
    if port.is_empty() {
        return;
    }
    for sample in 0..frames {
        let left = port[0][sample];
        let right = port.get(1).map_or(left, |channel| channel[sample]);
        if add {
            target.channels[0][sample] += left;
            target.channels[1][sample] += right;
        } else {
            target.channels[0][sample] = left;
            target.channels[1][sample] = right;
        }
    }
}

struct ClapEffect {
    runtime: ClapRuntime,
    scratch: AudioBuffer,
    bypassed: bool,
}
impl ClapEffect {
    fn new(spec: &EffectSpec, sample_rate: f32) -> Result<Self, String> {
        let reference = spec
            .plugin
            .as_ref()
            .ok_or("external effect is missing its plugin reference")?;
        Ok(Self {
            runtime: ClapRuntime::new(&reference.path, &reference.uid, sample_rate, true)?,
            scratch: AudioBuffer::new(),
            bypassed: spec.bypassed,
        })
    }
}
impl DspEffect for ClapEffect {
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn process(&mut self, buffer: &mut AudioBuffer, frames: usize) {
        self.process_with_sidechain(buffer, None, frames)
    }
    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        if self.bypassed {
            return;
        }
        for channel in 0..MAX_CHANNELS {
            self.scratch.channels[channel][..frames]
                .copy_from_slice(&buffer.channels[channel][..frames]);
        }
        self.runtime.clear_events();
        self.runtime
            .process(Some(&self.scratch), sidechain, buffer, frames, false);
    }
    fn set_param(&mut self, id: &str, value: f32) {
        if let Some(id) = id.strip_prefix("param:").and_then(|id| id.parse().ok()) {
            self.runtime.push_parameter(id, value);
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }
    fn reset(&mut self) {
        self.runtime.reset();
    }
}

struct ClapInstrument {
    runtime: ClapRuntime,
    active_notes: HashSet<i32>,
}
impl ClapInstrument {
    fn new(spec: &InstrumentSpec, sample_rate: f32) -> Result<Self, String> {
        let reference = spec
            .plugin
            .as_ref()
            .ok_or("external instrument is missing its plugin reference")?;
        Ok(Self {
            runtime: ClapRuntime::new(&reference.path, &reference.uid, sample_rate, false)?,
            active_notes: HashSet::with_capacity(64),
        })
    }
    fn push_event(&mut self, event: &NoteEvent) {
        let time = event.sample_offset;
        match event.kind {
            NoteEventKind::NoteOn {
                note_id,
                pitch,
                velocity,
                ..
            } => {
                self.active_notes.insert(note_id);
                self.runtime.push_midi(
                    time,
                    [
                        0x90,
                        pitch,
                        (velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
                    ],
                );
            }
            NoteEventKind::NoteOff {
                note_id,
                pitch,
                velocity,
            } => {
                self.active_notes.remove(&note_id);
                self.runtime.push_midi(
                    time,
                    [
                        0x80,
                        pitch,
                        (velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
                    ],
                );
            }
            NoteEventKind::PolyPressure {
                pitch, pressure, ..
            } => self.runtime.push_midi(
                time,
                [
                    0xa0,
                    pitch,
                    (pressure.clamp(0.0, 1.0) * 127.0).round() as u8,
                ],
            ),
            NoteEventKind::Controller { cc, value } => self.runtime.push_midi(
                time,
                [0xb0, cc, (value.clamp(0.0, 1.0) * 127.0).round() as u8],
            ),
            NoteEventKind::PitchBend { value } => {
                let bend = ((value.clamp(-1.0, 1.0) + 1.0) * 8191.5).round() as u16;
                self.runtime.push_midi(
                    time,
                    [0xe0, (bend & 0x7f) as u8, ((bend >> 7) & 0x7f) as u8],
                );
            }
            NoteEventKind::AllNotesOff => {
                self.active_notes.clear();
                self.runtime.push_midi(time, [0xb0, 123, 0]);
            }
        }
    }
}
impl Instrument for ClapInstrument {
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize) {}
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize) {
        for event in events {
            self.push_event(event);
        }
        self.runtime.process(None, None, out, frames, true);
    }
    fn set_param(&mut self, id: &str, value: f32) {
        if let Some(id) = id.strip_prefix("param:").and_then(|id| id.parse().ok()) {
            self.runtime.push_parameter(id, value);
        }
    }
    fn reset(&mut self) {
        self.active_notes.clear();
        self.runtime.reset();
    }
    fn tail_samples(&self) -> usize {
        0
    }
    fn active_voice_count(&self) -> usize {
        self.active_notes.len()
    }
}
