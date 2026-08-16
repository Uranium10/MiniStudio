//! External VST3/CLAP discovery and realtime adapters.
use clack_extensions::{
    audio_ports::{AudioPortFlags, AudioPortInfoBuffer, PluginAudioPorts},
    gui::{GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui},
    params::{ParamInfoBuffer, PluginParams},
    state::PluginState,
};
use clack_host::utils::Cookie;
use clack_host::{
    events::{
        event_types::{MidiEvent as ClapMidiEvent, ParamValueEvent as ClapParamValueEvent},
        io::{EventBuffer, InputEvents, OutputEvents},
    },
    host::{HostError, HostExtensions, HostHandlers, SharedHandler},
    prelude::{
        AudioPortBuffer, AudioPortBufferType, AudioPorts, ClapId, HostInfo, InputAudioBuffers,
        InputChannel, Pckn, PluginAudioConfiguration, PluginEntry, PluginInstance,
        StartedPluginAudioProcessor, StoppedPluginAudioProcessor,
    },
};
use ministudio_contracts::{EffectSpec, InstrumentSpec};
use ministudio_dsp::{
    AudioBuffer, DspEffect, EmbeddedPluginEditor, Instrument, NoteEvent, NoteEventKind,
    PluginControl, PluginEditorAction, PluginEditorState, MAX_BLOCK_SIZE, MAX_CHANNELS,
};
use raw_window_handle::{RawWindowHandle, Win32WindowHandle};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::{
    collections::{HashMap, HashSet},
    ffi::{CString, OsStr},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, sync_channel, Receiver, Sender, SyncSender},
        Arc, Mutex, TryLockError,
    },
};
use vst3_host::{
    embed::{EditorRect, EmbeddedEditor},
    midi::NoteId,
    realtime_ipc::{EditorActionReader, RealtimeClient},
    BusAudioBuffers, BusDirection, MediaType, MidiChannel, MidiEvent, Plugin, PluginWindow,
    Vst3Host,
};

/// Enter the VST3 helper process loop in the current executable. MiniStudio
/// calls this only after recognizing its private helper-mode command-line flag,
/// before Tauri, WebView2, audio devices or project state are initialized.
pub fn run_vst3_helper_process() {
    vst3_host::helper_process::run();
}

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
        paths.push(PathBuf::from(r"C:\Program Files\VST3"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            paths.push(PathBuf::from(&local).join("Programs/Common/VST3"));
            paths.push(PathBuf::from(local).join("Programs/Common/CLAP"));
        }
        if let Ok(roaming) = std::env::var("APPDATA") {
            paths.push(PathBuf::from(&roaming).join("VST3"));
            paths.push(PathBuf::from(roaming).join("CLAP"));
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
        let normalized =
            normalize_plugin_path(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
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

fn normalize_plugin_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

#[cfg(all(test, windows))]
mod path_tests {
    use super::*;

    #[test]
    fn windows_verbatim_paths_are_not_passed_to_plugins() {
        assert_eq!(
            normalize_plugin_path(PathBuf::from(
                r"\\?\C:\Program Files\Common Files\VST3\Example.vst3"
            )),
            PathBuf::from(r"C:\Program Files\Common Files\VST3\Example.vst3")
        );
        assert_eq!(
            normalize_plugin_path(PathBuf::from(r"\\?\UNC\studio\plugins\Example.vst3")),
            PathBuf::from(r"\\studio\plugins\Example.vst3")
        );
    }
}

pub fn probe_plugin(format: &str, path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    match format {
        "vst3" => probe_vst3(path),
        "clap" => probe_clap(path),
        _ => Err(format!("unsupported plugin format: {format}")),
    }
}

/// Fast first-pass discovery used to populate the browser. It never creates a
/// processor/controller instance, so a large plug-in folder can be catalogued
/// without waiting for every vendor UI and parameter tree to initialize.
pub fn probe_plugin_catalog(format: &str, path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    match format {
        "vst3" => {
            let info =
                vst3_host::discovery::get_plugin_info(path).map_err(|error| error.to_string())?;
            let category_lower = info.category.to_ascii_lowercase();
            Ok(vec![PluginDescriptor {
                format: "vst3".into(),
                uid: info.uid,
                name: info.name,
                vendor: info.vendor,
                category: info.category,
                path: path.to_string_lossy().into_owned(),
                is_instrument: category_lower.contains("instrument")
                    || category_lower.contains("synth"),
                param_count: 0,
                has_editor: info.has_gui,
                audio_input_buses: info.audio_inputs,
                audio_output_buses: info.audio_outputs,
                supports_sidechain: info.audio_inputs > 1,
                parameters: Vec::new(),
            }])
        }
        "clap" => {
            use clack_host::prelude::PluginEntry;
            let entry = unsafe { PluginEntry::load(path.as_os_str()) }
                .map_err(|error| error.to_string())?;
            let factory = entry
                .get_plugin_factory()
                .ok_or("CLAP entry has no plugin factory")?;
            Ok(factory
                .plugin_descriptors()
                .filter_map(|descriptor| {
                    let features: Vec<String> = descriptor
                        .features()
                        .map(|value| value.to_string_lossy().into_owned())
                        .collect();
                    Some(PluginDescriptor {
                        format: "clap".into(),
                        uid: descriptor.id()?.to_string_lossy().into_owned(),
                        name: descriptor.name()?.to_string_lossy().into_owned(),
                        vendor: descriptor
                            .vendor()
                            .map(|value| value.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        category: features.first().cloned().unwrap_or_else(|| "CLAP".into()),
                        path: path.to_string_lossy().into_owned(),
                        is_instrument: features.iter().any(|value| {
                            value == "instrument" || value == "synthesizer" || value == "sampler"
                        }),
                        param_count: 0,
                        has_editor: false,
                        audio_input_buses: 0,
                        audio_output_buses: 0,
                        supports_sidechain: false,
                        parameters: Vec::new(),
                    })
                })
                .collect())
        }
        _ => Err(format!("unsupported plugin format: {format}")),
    }
}

fn probe_vst3(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let info = vst3_host::discovery::get_plugin_info(path).map_err(|error| error.to_string())?;
    // This function only runs inside the disposable, deadline-limited probe process.
    // Instantiate the controller here so the main DAW process never pays for parameter
    // discovery or inherits a bad plug-in's crash, modal, or leaked state.
    let runtime = inspect_vst3_metadata(path, &info.uid).ok();
    let audio_input_buses = runtime.as_ref().map_or(info.audio_inputs, |value| value.0);
    let audio_output_buses = runtime.as_ref().map_or(info.audio_outputs, |value| value.1);
    let supports_sidechain = audio_input_buses > 1;
    let category_lower = info.category.to_ascii_lowercase();
    let is_instrument = category_lower.contains("instrument") || category_lower.contains("synth");
    let parameters = runtime.map_or_else(Vec::new, |value| value.2);
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

fn inspect_vst3_metadata(
    path: &Path,
    uid: &str,
) -> Result<(u32, u32, Vec<PluginParameterDescriptor>), String> {
    let mut host = Vst3Host::builder()
        .sample_rate(48_000.0)
        .block_size(MAX_BLOCK_SIZE)
        .input_channels(2)
        .output_channels(2)
        .with_process_isolation(false)
        .build()
        .map_err(|error| error.to_string())?;
    let plugin = host
        .load_plugin_class(path, uid)
        .map_err(|error| error.to_string())?;
    let layout = plugin.audio_bus_layout().ok();
    let inputs = layout.as_ref().map_or(0, |value| value.inputs.len() as u32);
    let outputs = layout
        .as_ref()
        .map_or(0, |value| value.outputs.len() as u32);
    let parameters = plugin
        .get_parameters()
        .unwrap_or_default()
        .into_iter()
        .filter(|parameter| !parameter.is_read_only)
        .take(65_536)
        .map(|parameter| PluginParameterDescriptor {
            id: parameter.id.to_string(),
            name: parameter.name,
            module: parameter.unit,
            min: parameter.min,
            max: parameter.max,
            default_value: parameter.default,
        })
        .collect();
    Ok((inputs, outputs, parameters))
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

fn query_clap_ports<H: HostHandlers>(
    instance: &mut PluginInstance<H>,
    is_input: bool,
) -> Vec<ClapPortLayout> {
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
        "MiniStudio Probe",
        "MiniStudio",
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
        let count = extension.count(&mut plugin).min(65_536);
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

#[derive(Clone, Copy)]
enum Vst3Role {
    Effect,
    Instrument,
}

fn load_vst3(path: &str, uid: &str, sample_rate: f32, role: Vst3Role) -> Result<Plugin, String> {
    let normalized_path = normalize_plugin_path(PathBuf::from(path));
    #[cfg(target_os = "windows")]
    let isolate = std::env::var("MINISTUDIO_VST3_PROCESS_ISOLATION")
        .ok()
        .map(|value| !matches!(value.trim(), "0" | "false" | "FALSE" | "off" | "OFF"))
        .unwrap_or(true);
    #[cfg(not(target_os = "windows"))]
    let isolate = false;
    let mut builder = Vst3Host::builder()
        .sample_rate(sample_rate as f64)
        .block_size(MAX_BLOCK_SIZE)
        .input_channels(MAX_CHANNELS)
        .output_channels(MAX_CHANNELS)
        .with_process_isolation(isolate);
    if isolate {
        let executable = std::env::current_exe()
            .map_err(|error| format!("failed to locate MiniStudio plug-in host: {error}"))?;
        builder = builder
            .self_hosted_helper(executable, "--ministudio-vst3-host")
            .response_timeout(std::time::Duration::from_secs(10))
            .auto_recover_plugins(true)
            .auto_recover_max_retries(1);
    }
    let mut host = builder.build().map_err(|error| error.to_string())?;
    let mut plugin = host
        .load_plugin_class(&normalized_path, uid)
        .map_err(|error| error.to_string())?;
    if let Ok(layout) = plugin.audio_bus_layout() {
        for (index, bus) in layout.inputs.iter().enumerate() {
            // Instruments normally have no audio input. Activating optional input
            // buses on sample players is rejected by some vendors during startup.
            // Effects retain their auxiliary buses for external sidechain routing.
            let should_be_active = matches!(role, Vst3Role::Effect);
            if bus.active != should_be_active {
                let _ = plugin.set_bus_active(
                    MediaType::Audio,
                    BusDirection::Input,
                    index as i32,
                    should_be_active,
                );
            }
        }
        for (index, bus) in layout.outputs.iter().enumerate() {
            // A DAW must not blindly activate every advertised multi-output bus.
            // Libraries such as BBC Symphony Orchestra expose many optional stems
            // and expect only the main bus until the user explicitly enables more.
            let should_be_active = index == 0;
            if bus.active != should_be_active {
                let _ = plugin.set_bus_active(
                    MediaType::Audio,
                    BusDirection::Output,
                    index as i32,
                    should_be_active,
                );
            }
        }
    }
    plugin
        .start_processing()
        .map_err(|error| error.to_string())?;
    Ok(plugin)
}

enum Vst3ControlCommand {
    Recover,
    Open(Sender<Result<(), String>>),
    OpenEmbedded {
        parent: usize,
        rect: EditorRect,
        reply: Sender<Result<(), String>>,
    },
    CloseEmbedded,
    Close(Sender<Result<(), String>>),
    SaveState(Sender<Result<Vec<u8>, String>>),
    LoadState(Vec<u8>, Sender<Result<(), String>>),
    TakeEditorActions(Sender<Vec<PluginEditorAction>>),
    SetEditorState(PluginEditorState),
    SetEditorPinned(PluginEditorState, Sender<Result<(), String>>),
    /// Resource hint only - see `PluginControl::set_idle`. Fire-and-forget, like
    /// `CloseEmbedded`: nothing downstream needs to know it landed.
    SetIdle(bool),
    Shutdown,
}

fn queue_vst3_recovery_once(pending: &AtomicBool, sender: &Sender<Vst3ControlCommand>) -> bool {
    if pending
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    if sender.send(Vst3ControlCommand::Recover).is_err() {
        pending.store(false, Ordering::Release);
        return false;
    }
    true
}

struct Vst3Control {
    sender: Sender<Vst3ControlCommand>,
    editor_open: Arc<AtomicBool>,
    pending_editor_rect: Arc<Mutex<Option<EditorRect>>>,
    has_editor: Arc<AtomicBool>,
    host_state: Mutex<PluginEditorState>,
    action_request_pending: Arc<AtomicBool>,
    action_reader: Arc<Mutex<Option<EditorActionReader>>>,
    recovery_handle: Arc<Mutex<Option<vst3_host::realtime_ipc::RealtimeRecoveryHandle>>>,
    parameter_reader: Arc<Mutex<Option<vst3_host::realtime_ipc::RealtimeParameterReader>>>,
}

struct Vst3Ready {
    plugin: Arc<Mutex<Plugin>>,
    realtime: Option<RealtimeClient>,
    buffers: BusAudioBuffers,
    latency: usize,
    tail: usize,
}

struct Vst3EmbeddedEditor {
    sender: Sender<Vst3ControlCommand>,
    editor_open: Arc<AtomicBool>,
    pending_editor_rect: Arc<Mutex<Option<EditorRect>>>,
}

impl EmbeddedPluginEditor for Vst3EmbeddedEditor {
    fn set_rect(&mut self, x: f32, y: f32, width: f32, height: f32) {
        let rect = EditorRect {
            x,
            y,
            width: width.max(1.0),
            height: height.max(1.0),
        };
        *self
            .pending_editor_rect
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(rect);
    }
}

impl Drop for Vst3EmbeddedEditor {
    fn drop(&mut self) {
        // The real editor is destroyed by the same message-pumped GUI thread
        // that created it. This is intentionally asynchronous: graph teardown
        // can release the Tauri UI thread immediately, while channel ordering
        // guarantees CloseEmbedded precedes the control's later Shutdown.
        self.editor_open.store(false, Ordering::Release);
        let _ = self.sender.send(Vst3ControlCommand::CloseEmbedded);
    }
}

impl Vst3Control {
    fn request<T>(
        &self,
        build: impl FnOnce(Sender<Result<T, String>>) -> Vst3ControlCommand,
    ) -> Result<T, String> {
        self.request_with_timeout(
            build,
            std::time::Duration::from_secs(10),
            "VST3 control operation",
        )
    }

    fn request_with_timeout<T>(
        &self,
        build: impl FnOnce(Sender<Result<T, String>>) -> Vst3ControlCommand,
        timeout: std::time::Duration,
        operation: &str,
    ) -> Result<T, String> {
        let (reply, receive) = channel();
        self.sender
            .send(build(reply))
            .map_err(|_| "VST3 control thread is unavailable".to_owned())?;
        receive
            .recv_timeout(timeout)
            .map_err(|_| format!("{operation} timed out after {} seconds", timeout.as_secs()))?
    }
}

fn start_vst3_instance(
    path: String,
    uid: String,
    state: Vec<u8>,
    params: Vec<(String, f32)>,
    sample_rate: f32,
    role: Vst3Role,
) -> Result<(Vst3Ready, Arc<Vst3Control>), String> {
    let (sender, receiver) = channel();
    let (ready_sender, ready_receiver) = sync_channel(1);
    let editor_open = Arc::new(AtomicBool::new(false));
    let has_editor = Arc::new(AtomicBool::new(false));
    let pending_editor_rect = Arc::new(Mutex::new(None));
    let action_request_pending = Arc::new(AtomicBool::new(false));
    let action_reader = Arc::new(Mutex::new(None));
    let recovery_handle = Arc::new(Mutex::new(None));
    let parameter_reader = Arc::new(Mutex::new(None));
    let recovery_pending = Arc::new(AtomicBool::new(false));
    let recovery_sender = sender.clone();
    let state_checkpoint = Arc::new(Mutex::new(state.clone()));
    let parameter_checkpoint = params.clone();
    let control = Arc::new(Vst3Control {
        sender,
        editor_open: Arc::clone(&editor_open),
        pending_editor_rect: Arc::clone(&pending_editor_rect),
        has_editor: Arc::clone(&has_editor),
        host_state: Mutex::new(PluginEditorState::default()),
        action_request_pending: Arc::clone(&action_request_pending),
        action_reader: Arc::clone(&action_reader),
        recovery_handle: Arc::clone(&recovery_handle),
        parameter_reader: Arc::clone(&parameter_reader),
    });

    std::thread::Builder::new()
        .name("ministudio-vst3-owner".into())
        .spawn(move || {
            let owner_checkpoint = Arc::clone(&state_checkpoint);
            // VST3's control-thread identity is captured while the component and
            // controller are initialized. Keep every non-realtime operation,
            // native window, message pump, and final destruction on this same
            // owner thread for the complete instance lifetime.
            let initialized = (|| -> Result<Vst3Ready, String> {
                let mut plugin = load_vst3(&path, &uid, sample_rate, role)?;
                if !state.is_empty() {
                    plugin
                        .load_state(&state)
                        .map_err(|error| format!("failed to restore VST3 state: {error}"))?;
                }
                for (id, value) in params {
                    if let Ok(id) = id.strip_prefix("param:").unwrap_or(&id).parse::<u32>() {
                        let _ = plugin.set_parameter(id, value.clamp(0.0, 1.0) as f64);
                    }
                }
                let latency = plugin.latency_samples() as usize;
                let tail = plugin.tail_samples() as usize;
                let buffers = plugin
                    .create_bus_audio_buffers(MAX_BLOCK_SIZE)
                    .map_err(|error| error.to_string())?;
                // The shared-memory endpoint has one audio-side owner. Detach it before the
                // remaining Plugin value is put behind the control-plane mutex, otherwise a
                // slow editor/state request can turn `try_lock` failure into a silent block.
                let realtime = plugin.take_isolated_realtime();
                if let Some(reader) = realtime.as_ref().map(RealtimeClient::fault_reader) {
                    let sender = recovery_sender.clone();
                    let pending = Arc::clone(&recovery_pending);
                    let _ = std::thread::Builder::new()
                        .name("ministudio-vst3-fault-events".into())
                        .spawn(move || {
                            while reader.alive() {
                                if reader.wait(std::time::Duration::from_secs(1)) {
                                    let _ = queue_vst3_recovery_once(&pending, &sender);
                                }
                            }
                        });
                }
                *recovery_handle
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) =
                    realtime.as_ref().map(RealtimeClient::recovery_handle);
                *parameter_reader
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) =
                    realtime.as_ref().map(RealtimeClient::parameter_reader);
                *action_reader
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) =
                    realtime.as_ref().map(RealtimeClient::editor_action_reader);
                has_editor.store(plugin.has_editor(), Ordering::Release);
                Ok(Vst3Ready {
                    plugin: Arc::new(Mutex::new(plugin)),
                    realtime,
                    buffers,
                    latency,
                    tail,
                })
            })();

            match initialized {
                Ok(ready) => {
                    let plugin = Arc::clone(&ready.plugin);
                    if ready_sender.send(Ok(ready)).is_ok() {
                        run_vst3_control(
                            plugin,
                            receiver,
                            editor_open,
                            pending_editor_rect,
                            owner_checkpoint,
                            parameter_checkpoint,
                            action_request_pending,
                            recovery_pending,
                        );
                    }
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error));
                }
            }
        })
        .map_err(|error| error.to_string())?;

    let ready = ready_receiver
        .recv_timeout(std::time::Duration::from_secs(120))
        .map_err(|_| "VST3 instance initialization timed out after 120 seconds".to_owned())??;
    Ok((ready, control))
}

impl PluginControl for Vst3Control {
    fn has_editor(&self) -> bool {
        self.has_editor.load(Ordering::Acquire)
    }
    fn open_editor(&self) -> Result<(), String> {
        if !self.has_editor() {
            return Err("This VST3 plug-in does not provide an editor".into());
        }
        self.request_with_timeout(
            Vst3ControlCommand::Open,
            std::time::Duration::from_secs(120),
            "VST3 editor open",
        )
    }
    fn open_editor_embedded(
        &self,
        parent: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Result<Box<dyn EmbeddedPluginEditor>, String> {
        if !self.has_editor() {
            return Err("This VST3 plug-in does not provide an editor".into());
        }
        let rect = EditorRect {
            x,
            y,
            width: width.max(1.0),
            height: height.max(1.0),
        };
        #[cfg(not(target_os = "windows"))]
        return Err("embedded plug-in shell currently requires Windows".into());
        #[cfg(target_os = "windows")]
        self.request(|reply| Vst3ControlCommand::OpenEmbedded {
            parent,
            rect,
            reply,
        })?;
        Ok(Box::new(Vst3EmbeddedEditor {
            sender: self.sender.clone(),
            editor_open: Arc::clone(&self.editor_open),
            pending_editor_rect: Arc::clone(&self.pending_editor_rect),
        }))
    }
    fn close_editor(&self) -> Result<(), String> {
        self.request(Vst3ControlCommand::Close)
    }
    fn is_editor_open(&self) -> bool {
        self.editor_open.load(Ordering::Acquire)
    }
    fn save_state(&self) -> Result<Vec<u8>, String> {
        self.request(Vst3ControlCommand::SaveState)
    }
    fn load_state(&self, state: Vec<u8>) -> Result<(), String> {
        self.request(|reply| Vst3ControlCommand::LoadState(state, reply))
    }
    fn take_editor_actions(&self) -> Vec<PluginEditorAction> {
        if let Some(reader) = self
            .action_reader
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
        {
            let actions = reader
                .take()
                .into_iter()
                .map(map_vst3_editor_action)
                .collect::<Vec<_>>();
            if actions.contains(&PluginEditorAction::Closed) {
                self.editor_open.store(false, Ordering::Release);
            }
            return actions;
        }
        if self.action_request_pending.swap(true, Ordering::AcqRel) {
            return Vec::new();
        }
        let (reply, receive) = channel();
        if self
            .sender
            .send(Vst3ControlCommand::TakeEditorActions(reply))
            .is_err()
        {
            self.action_request_pending.store(false, Ordering::Release);
            return Vec::new();
        }
        let actions = receive
            .recv_timeout(std::time::Duration::from_millis(250))
            .unwrap_or_default();
        if actions.contains(&PluginEditorAction::Closed) {
            self.editor_open.store(false, Ordering::Release);
        }
        actions
    }
    fn wait_editor_actions(&self, timeout: std::time::Duration) -> Vec<PluginEditorAction> {
        if let Some(reader) = self
            .action_reader
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
        {
            let actions = reader
                .wait_and_take(timeout)
                .into_iter()
                .map(map_vst3_editor_action)
                .collect::<Vec<_>>();
            if actions.contains(&PluginEditorAction::Closed) {
                self.editor_open.store(false, Ordering::Release);
            }
            return actions;
        }
        std::thread::sleep(timeout);
        self.take_editor_actions()
    }
    fn set_editor_state(&self, mut state: PluginEditorState) {
        let mut current = self
            .host_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // Modal ownership belongs to the native control plane. Routine toolbar
        // snapshots from the webview must not re-enable or raise the editor
        // while a DAW-owned file dialog is still open.
        state.modal = current.modal;
        *current = state;
        drop(current);
        let _ = self.sender.send(Vst3ControlCommand::SetEditorState(state));
    }
    fn set_editor_owner_window(&self, owner_window: u64) {
        let mut current = self
            .host_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        current.owner_window = owner_window;
        let state = *current;
        drop(current);
        // This precedes OpenEditor on the same single-owner channel, so the
        // helper applies DAW ownership before the native window is shown.
        let _ = self.sender.send(Vst3ControlCommand::SetEditorState(state));
    }
    fn set_editor_modal(&self, modal: bool) -> Result<(), String> {
        let mut state = *self
            .host_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.modal = modal;
        self.request(|reply| Vst3ControlCommand::SetEditorPinned(state, reply))?;
        *self
            .host_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = state;
        Ok(())
    }
    fn take_parameter_changes(&self) -> Vec<(String, f32)> {
        self.parameter_reader
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .map(|reader| {
                reader
                    .take()
                    .into_iter()
                    .map(|(id, value)| (format!("param:{id}"), value.clamp(0.0, 1.0) as f32))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn set_editor_pinned(&self, pinned: bool) -> Result<(), String> {
        let mut state = *self
            .host_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.pinned = pinned;
        self.request(|reply| Vst3ControlCommand::SetEditorPinned(state, reply))?;
        *self
            .host_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = state;
        Ok(())
    }
    fn set_idle(&self, idle: bool) {
        let _ = self.sender.send(Vst3ControlCommand::SetIdle(idle));
    }
    fn realtime_deadline_misses(&self) -> u64 {
        self.recovery_handle
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .map_or(
                0,
                vst3_host::realtime_ipc::RealtimeRecoveryHandle::deadline_misses,
            )
    }
}

fn map_vst3_editor_action(
    action: vst3_host::process_isolation::EditorHostAction,
) -> PluginEditorAction {
    match action {
        vst3_host::process_isolation::EditorHostAction::Closed => PluginEditorAction::Closed,
        vst3_host::process_isolation::EditorHostAction::TogglePower => {
            PluginEditorAction::TogglePower
        }
        vst3_host::process_isolation::EditorHostAction::TogglePin => PluginEditorAction::TogglePin,
        vst3_host::process_isolation::EditorHostAction::ToggleBypass => {
            PluginEditorAction::ToggleBypass
        }
        vst3_host::process_isolation::EditorHostAction::SavePreset => {
            PluginEditorAction::SavePreset
        }
        vst3_host::process_isolation::EditorHostAction::LoadPreset => {
            PluginEditorAction::LoadPreset
        }
        vst3_host::process_isolation::EditorHostAction::ShowSidechain => {
            PluginEditorAction::ShowSidechain
        }
        vst3_host::process_isolation::EditorHostAction::AutomationOff => {
            PluginEditorAction::AutomationOff
        }
        vst3_host::process_isolation::EditorHostAction::AutomationWrite => {
            PluginEditorAction::AutomationWrite
        }
        vst3_host::process_isolation::EditorHostAction::AutomationRead => {
            PluginEditorAction::AutomationRead
        }
        vst3_host::process_isolation::EditorHostAction::AutomationLatch => {
            PluginEditorAction::AutomationLatch
        }
    }
}

impl Drop for Vst3Control {
    fn drop(&mut self) {
        let _ = self.sender.send(Vst3ControlCommand::Shutdown);
    }
}

fn run_vst3_control(
    plugin: Arc<Mutex<Plugin>>,
    receiver: Receiver<Vst3ControlCommand>,
    editor_open: Arc<AtomicBool>,
    pending_editor_rect: Arc<Mutex<Option<EditorRect>>>,
    state_checkpoint: Arc<Mutex<Vec<u8>>>,
    parameter_checkpoint: Vec<(String, f32)>,
    action_request_pending: Arc<AtomicBool>,
    recovery_pending: Arc<AtomicBool>,
) {
    // Do not preselect a COM apartment for a vendor's standalone GUI thread.
    // The original standalone host left this thread uninitialized and allowed
    // each plug-in framework to choose the apartment it requires. Forcing STA
    // here made some otherwise valid commercial editors (including sample
    // players) silently stop during view creation.
    let mut window = PluginWindow::new(Arc::clone(&plugin));
    let isolated = plugin
        .lock()
        .map(|plugin| plugin.isolation_pid().is_some())
        .unwrap_or(false);
    let mut embedded: Option<EmbeddedEditor> = None;
    loop {
        let received = if isolated {
            receiver
                .recv()
                .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
        } else {
            receiver.recv_timeout(std::time::Duration::from_millis(8))
        };
        match received {
            Ok(Vst3ControlCommand::Recover) => {
                editor_open.store(false, Ordering::Release);
                if let Ok(mut plugin) = plugin.lock() {
                    if plugin.recover().is_ok() {
                        let checkpoint = state_checkpoint
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .clone();
                        if !checkpoint.is_empty() {
                            let _ = plugin.load_state(&checkpoint);
                        } else {
                            for (id, value) in &parameter_checkpoint {
                                if let Ok(id) =
                                    id.strip_prefix("param:").unwrap_or(id).parse::<u32>()
                                {
                                    let _ = plugin.set_parameter(id, value.clamp(0.0, 1.0) as f64);
                                }
                            }
                        }
                    }
                }
                recovery_pending.store(false, Ordering::Release);
            }
            Ok(Vst3ControlCommand::Open(reply)) => {
                // Standalone and embedded views are mutually exclusive for one
                // VST3 instance. Both are owned by this same GUI worker.
                embedded.take();
                let result = if isolated {
                    plugin
                        .lock()
                        .map_err(|_| "VST3 instance lock was poisoned".to_owned())
                        .and_then(|mut plugin| {
                            plugin
                                .open_isolated_editor()
                                .map_err(|error| error.to_string())
                        })
                } else if window.is_open() {
                    Ok(())
                } else {
                    window.open().map_err(|error| error.to_string())
                };
                if result.is_ok() {
                    keep_hosting_windows_above_daw();
                }
                editor_open.store(result.is_ok(), Ordering::Release);
                let _ = reply.send(result);
            }
            Ok(Vst3ControlCommand::OpenEmbedded {
                parent,
                rect,
                reply,
            }) => {
                if isolated {
                    editor_open.store(false, Ordering::Release);
                    let _ = reply.send(Err(
                        "process-isolated VST3 editors use a helper-owned standalone window"
                            .to_owned(),
                    ));
                    continue;
                }
                window.close();
                embedded.take();
                pending_editor_rect
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
                #[cfg(target_os = "windows")]
                let result = std::num::NonZeroIsize::new(parent as isize)
                    .ok_or_else(|| "invalid native parent window".to_owned())
                    .and_then(|hwnd| {
                        EmbeddedEditor::embed(
                            Arc::clone(&plugin),
                            RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)),
                            rect,
                        )
                        .map_err(|error| error.to_string())
                    });
                #[cfg(not(target_os = "windows"))]
                let result: Result<EmbeddedEditor, String> =
                    Err("embedded plug-in shell currently requires Windows".into());
                match result {
                    Ok(editor) => {
                        embedded = Some(editor);
                        editor_open.store(true, Ordering::Release);
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        editor_open.store(false, Ordering::Release);
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Ok(Vst3ControlCommand::CloseEmbedded) => {
                embedded.take();
                pending_editor_rect
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
                editor_open.store(window.is_open(), Ordering::Release);
            }
            Ok(Vst3ControlCommand::SetIdle(_idle)) => {
                // A realtime-isolated helper is still servicing the shared-memory audio
                // deadline while its track happens to be silent. Emptying that process's
                // working set here produces page-fault stalls, latches a transport timeout,
                // and makes auto-recovery shut down and respawn the helper in a loop. Keep
                // the control-plane hint as the future P1 scheduler seam, but do not reclaim
                // pages until the graph can actually suspend this node after its tail ends.
            }
            Ok(Vst3ControlCommand::Close(reply)) => {
                embedded.take();
                pending_editor_rect
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
                if isolated {
                    let _ = plugin
                        .lock()
                        .map_err(|_| "VST3 instance lock was poisoned".to_owned())
                        .and_then(|mut plugin| {
                            plugin.close_editor().map_err(|error| error.to_string())
                        });
                } else {
                    window.close();
                }
                editor_open.store(false, Ordering::Release);
                let _ = reply.send(Ok(()));
            }
            Ok(Vst3ControlCommand::SaveState(reply)) => {
                let result = plugin
                    .lock()
                    .map_err(|_| "VST3 instance lock was poisoned".to_owned())
                    .and_then(|plugin| plugin.save_state().map_err(|error| error.to_string()));
                if let Ok(snapshot) = result.as_ref() {
                    *state_checkpoint
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()) = snapshot.clone();
                }
                let _ = reply.send(result);
            }
            Ok(Vst3ControlCommand::LoadState(state, reply)) => {
                let checkpoint = state.clone();
                let result = plugin
                    .lock()
                    .map_err(|_| "VST3 instance lock was poisoned".to_owned())
                    .and_then(|mut plugin| {
                        plugin.load_state(&state).map_err(|error| error.to_string())
                    });
                if result.is_ok() {
                    *state_checkpoint
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()) = checkpoint;
                }
                let _ = reply.send(result);
            }
            Ok(Vst3ControlCommand::TakeEditorActions(reply)) => {
                let actions = if isolated {
                    plugin
                        .try_lock()
                        .map(|mut plugin| {
                            plugin
                                .take_editor_host_actions()
                                .into_iter()
                                .map(|action| match action {
                                    vst3_host::process_isolation::EditorHostAction::Closed => PluginEditorAction::Closed,
                                    vst3_host::process_isolation::EditorHostAction::TogglePower => PluginEditorAction::TogglePower,
                                    vst3_host::process_isolation::EditorHostAction::TogglePin => PluginEditorAction::TogglePin,
                                    vst3_host::process_isolation::EditorHostAction::ToggleBypass => PluginEditorAction::ToggleBypass,
                                    vst3_host::process_isolation::EditorHostAction::SavePreset => PluginEditorAction::SavePreset,
                                    vst3_host::process_isolation::EditorHostAction::LoadPreset => PluginEditorAction::LoadPreset,
                                    vst3_host::process_isolation::EditorHostAction::ShowSidechain => PluginEditorAction::ShowSidechain,
                                    vst3_host::process_isolation::EditorHostAction::AutomationOff => PluginEditorAction::AutomationOff,
                                    vst3_host::process_isolation::EditorHostAction::AutomationWrite => PluginEditorAction::AutomationWrite,
                                    vst3_host::process_isolation::EditorHostAction::AutomationRead => PluginEditorAction::AutomationRead,
                                    vst3_host::process_isolation::EditorHostAction::AutomationLatch => PluginEditorAction::AutomationLatch,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    window
                        .take_host_actions()
                        .into_iter()
                        .map(|action| match action {
                            vst3_host::process_isolation::EditorHostAction::Closed => {
                                PluginEditorAction::Closed
                            }
                            vst3_host::process_isolation::EditorHostAction::TogglePower => {
                                PluginEditorAction::TogglePower
                            }
                            vst3_host::process_isolation::EditorHostAction::TogglePin => {
                                PluginEditorAction::TogglePin
                            }
                            vst3_host::process_isolation::EditorHostAction::ToggleBypass => {
                                PluginEditorAction::ToggleBypass
                            }
                            vst3_host::process_isolation::EditorHostAction::SavePreset => {
                                PluginEditorAction::SavePreset
                            }
                            vst3_host::process_isolation::EditorHostAction::LoadPreset => {
                                PluginEditorAction::LoadPreset
                            }
                            vst3_host::process_isolation::EditorHostAction::ShowSidechain => {
                                PluginEditorAction::ShowSidechain
                            }
                            vst3_host::process_isolation::EditorHostAction::AutomationOff => {
                                PluginEditorAction::AutomationOff
                            }
                            vst3_host::process_isolation::EditorHostAction::AutomationWrite => {
                                PluginEditorAction::AutomationWrite
                            }
                            vst3_host::process_isolation::EditorHostAction::AutomationRead => {
                                PluginEditorAction::AutomationRead
                            }
                            vst3_host::process_isolation::EditorHostAction::AutomationLatch => {
                                PluginEditorAction::AutomationLatch
                            }
                        })
                        .collect()
                };
                action_request_pending.store(false, Ordering::Release);
                let _ = reply.send(actions);
            }
            Ok(Vst3ControlCommand::SetEditorState(state)) => {
                if isolated {
                    if let Ok(mut plugin) = plugin.lock() {
                        let _ = plugin.set_editor_host_state(
                            vst3_host::process_isolation::EditorHostState {
                                bypassed: state.bypassed,
                                automation: state.automation.min(3),
                                pinned: state.pinned,
                                owner_window: state.owner_window,
                                modal: state.modal,
                            },
                        );
                    }
                } else {
                    window.set_host_state(vst3_host::process_isolation::EditorHostState {
                        bypassed: state.bypassed,
                        automation: state.automation.min(3),
                        pinned: state.pinned,
                        owner_window: state.owner_window,
                        modal: state.modal,
                    });
                }
            }
            Ok(Vst3ControlCommand::SetEditorPinned(state, reply)) => {
                let result = if isolated {
                    plugin
                        .lock()
                        .map_err(|_| "VST3 instance lock was poisoned".to_owned())
                        .and_then(|mut plugin| {
                            plugin
                                .set_editor_host_state(
                                    vst3_host::process_isolation::EditorHostState {
                                        bypassed: state.bypassed,
                                        automation: state.automation.min(3),
                                        pinned: state.pinned,
                                        owner_window: state.owner_window,
                                        modal: state.modal,
                                    },
                                )
                                .map_err(|error| error.to_string())
                        })
                } else {
                    window.set_host_state(vst3_host::process_isolation::EditorHostState {
                        bypassed: state.bypassed,
                        automation: state.automation.min(3),
                        pinned: state.pinned,
                        owner_window: state.owner_window,
                        modal: state.modal,
                    });
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Ok(Vst3ControlCommand::Shutdown)
            | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                embedded.take();
                pending_editor_rect
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
                if isolated {
                    if let Ok(mut plugin) = plugin.lock() {
                        let _ = plugin.close_editor();
                    }
                } else {
                    window.close();
                }
                editor_open.store(false, Ordering::Release);
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !isolated && window.closed_by_user() {
            window.close();
            editor_open.store(false, Ordering::Release);
        } else if !isolated && window.is_open() {
            let _ = window.service_platform_events();
        }
        if let Some(editor) = embedded.as_ref() {
            if let Some(rect) = pending_editor_rect
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take()
            {
                editor.set_rect(rect);
            }
        }
        pump_vst3_window_messages();
    }
}

#[cfg(target_os = "windows")]
fn pump_vst3_window_messages() {
    use winapi::um::winuser::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn pump_vst3_window_messages() {}

#[cfg(target_os = "windows")]
fn keep_hosting_windows_above_daw() {
    use winapi::{
        shared::{
            minwindef::{BOOL, LPARAM, TRUE},
            windef::HWND,
        },
        um::{
            processthreadsapi::GetCurrentThreadId,
            winuser::{
                EnumThreadWindows, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOSIZE,
            },
        },
    };
    unsafe extern "system" fn make_topmost(window: HWND, _context: LPARAM) -> BOOL {
        SetWindowPos(
            window,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        TRUE
    }
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), Some(make_topmost), 0);
    }
}

#[cfg(not(target_os = "windows"))]
fn keep_hosting_windows_above_daw() {}

struct Vst3Effect {
    plugin: Arc<Mutex<Plugin>>,
    realtime: Option<RealtimeClient>,
    control: Arc<Vst3Control>,
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
        let (ready, control) = start_vst3_instance(
            reference.path.clone(),
            reference.uid.clone(),
            reference.state.clone(),
            spec.params
                .iter()
                .map(|(id, value)| (id.clone(), *value))
                .collect(),
            sample_rate,
            Vst3Role::Effect,
        )?;
        Ok(Self {
            plugin: ready.plugin,
            realtime: ready.realtime,
            control,
            buffers: ready.buffers,
            bypassed: spec.bypassed,
            latency: ready.latency,
            tail: ready.tail,
        })
    }
}

impl DspEffect for Vst3Effect {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.buffers.sample_rate = sample_rate as f64;
    }
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        self.process_with_sidechain(events, buffer, None, frames)
    }
    fn process_with_sidechain(
        &mut self,
        _events: &[NoteEvent],
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        if self.bypassed {
            return;
        }
        let frames = frames.min(MAX_BLOCK_SIZE);
        prepare_vst_bus_block(&mut self.buffers, frames);
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
        let result = if let Some(realtime) = self.realtime.as_mut() {
            realtime.process_buses(&mut self.buffers)
        } else {
            match self.plugin.try_lock() {
                Ok(mut plugin) => plugin.process_bus_audio(&mut self.buffers),
                Err(TryLockError::Poisoned(poison)) => {
                    poison.into_inner().process_bus_audio(&mut self.buffers)
                }
                Err(TryLockError::WouldBlock) => return,
            }
        };
        if result.is_ok() {
            if let Some(main) = self.buffers.outputs.first() {
                copy_vst_bus_to_stereo(main, buffer, frames, false);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        if let Ok(id) = id.strip_prefix("param:").unwrap_or(id).parse::<u32>() {
            if let Some(realtime) = self.realtime.as_mut() {
                let _ = realtime.queue_parameter(id, value.clamp(0.0, 1.0) as f64, 0);
                return;
            }
            if let Ok(mut plugin) = self.plugin.try_lock() {
                let _ = plugin.set_parameter(id, value.clamp(0.0, 1.0) as f64);
            }
        }
    }
    fn set_tempo(&mut self, bpm: f64) {
        if let Ok(mut plugin) = self.plugin.try_lock() {
            let _ = plugin.set_tempo(bpm);
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }
    fn reset(&mut self) {
        if let Ok(mut plugin) = self.plugin.try_lock() {
            let _ = plugin.stop_processing();
            let _ = plugin.start_processing();
        }
    }
    fn tail_samples(&self) -> usize {
        self.tail
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        Some(self.control.clone())
    }
}

struct Vst3Instrument {
    plugin: Arc<Mutex<Plugin>>,
    realtime: Option<RealtimeClient>,
    control: Arc<Vst3Control>,
    buffers: BusAudioBuffers,
    active_notes: HashMap<i32, NoteId>,
    realtime_active_notes: HashSet<i32>,
    tail: usize,
}

impl Vst3Instrument {
    fn new(spec: &InstrumentSpec, sample_rate: f32) -> Result<Self, String> {
        let reference = spec
            .plugin
            .as_ref()
            .ok_or("external instrument is missing its plugin reference")?;
        let (ready, control) = start_vst3_instance(
            reference.path.clone(),
            reference.uid.clone(),
            reference.state.clone(),
            spec.params
                .iter()
                .map(|(id, value)| (id.clone(), *value))
                .collect(),
            sample_rate,
            Vst3Role::Instrument,
        )?;
        Ok(Self {
            plugin: ready.plugin,
            realtime: ready.realtime,
            control,
            buffers: ready.buffers,
            active_notes: HashMap::with_capacity(64),
            realtime_active_notes: HashSet::with_capacity(64),
            tail: ready.tail,
        })
    }
    fn send_event(plugin: &mut Plugin, active_notes: &mut HashMap<i32, NoteId>, event: &NoteEvent) {
        let offset = event.sample_offset as i32;
        let midi = match event.kind {
            NoteEventKind::NoteOn {
                note_id,
                pitch,
                velocity,
                ..
            } => {
                let velocity = (velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
                if let Ok(host_id) = plugin.note_on_at(MidiChannel::Ch1, pitch, velocity, offset) {
                    active_notes.insert(note_id, host_id);
                    return;
                }
                MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: pitch,
                    velocity,
                }
            }
            NoteEventKind::NoteOff {
                note_id,
                pitch,
                velocity,
            } => {
                if let Some(host_id) = active_notes.remove(&note_id) {
                    let _ = plugin.note_off_at(host_id, offset);
                    return;
                }
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
                active_notes.clear();
                let _ = plugin.midi_panic();
                MidiEvent::ControlChange {
                    channel: MidiChannel::Ch1,
                    controller: 123,
                    value: 0,
                }
            }
        };
        let _ = plugin.send_midi_event_at(midi, offset);
    }

    fn queue_realtime_event(&mut self, event: &NoteEvent) {
        let Some(realtime) = self.realtime.as_mut() else {
            return;
        };
        let offset = event.sample_offset as i32;
        let midi = match event.kind {
            NoteEventKind::NoteOn {
                note_id,
                pitch,
                velocity,
                ..
            } => {
                self.realtime_active_notes.insert(note_id);
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
                self.realtime_active_notes.remove(&note_id);
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
                self.realtime_active_notes.clear();
                MidiEvent::ControlChange {
                    channel: MidiChannel::Ch1,
                    controller: 123,
                    value: 0,
                }
            }
        };
        let _ = realtime.queue_midi(midi, offset);
    }
}

impl Instrument for Vst3Instrument {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.buffers.sample_rate = sample_rate as f64;
    }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize) {
        let frames = frames.min(MAX_BLOCK_SIZE);
        prepare_vst_bus_block(&mut self.buffers, frames);
        let result = if self.realtime.is_some() {
            for event in events {
                self.queue_realtime_event(event);
            }
            self.realtime
                .as_mut()
                .expect("realtime endpoint checked")
                .process_buses(&mut self.buffers)
        } else {
            match self.plugin.try_lock() {
                Ok(mut plugin) => {
                    for event in events {
                        Self::send_event(&mut plugin, &mut self.active_notes, event);
                    }
                    plugin.process_bus_audio(&mut self.buffers)
                }
                Err(TryLockError::Poisoned(poison)) => {
                    let mut plugin = poison.into_inner();
                    for event in events {
                        Self::send_event(&mut plugin, &mut self.active_notes, event);
                    }
                    plugin.process_bus_audio(&mut self.buffers)
                }
                Err(TryLockError::WouldBlock) => return,
            }
        };
        if result.is_ok() {
            if let Some(main) = self.buffers.outputs.first() {
                copy_vst_bus_to_stereo(main, out, frames, true);
            }
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        if let Ok(id) = id.strip_prefix("param:").unwrap_or(id).parse::<u32>() {
            if let Some(realtime) = self.realtime.as_mut() {
                let _ = realtime.queue_parameter(id, value.clamp(0.0, 1.0) as f64, 0);
                return;
            }
            if let Ok(mut plugin) = self.plugin.try_lock() {
                let _ = plugin.set_parameter(id, value.clamp(0.0, 1.0) as f64);
            }
        }
    }
    fn set_tempo(&mut self, bpm: f64) {
        if let Ok(mut plugin) = self.plugin.try_lock() {
            let _ = plugin.set_tempo(bpm);
        }
    }
    fn reset(&mut self) {
        self.active_notes.clear();
        self.realtime_active_notes.clear();
        if let Some(realtime) = self.realtime.as_mut() {
            let _ = realtime.queue_midi(
                MidiEvent::ControlChange {
                    channel: MidiChannel::Ch1,
                    controller: 123,
                    value: 0,
                },
                0,
            );
            return;
        }
        if let Ok(mut plugin) = self.plugin.try_lock() {
            let _ = plugin.stop_processing();
            let _ = plugin.start_processing();
        }
    }
    fn tail_samples(&self) -> usize {
        self.tail
    }
    fn active_voice_count(&self) -> usize {
        self.active_notes.len() + self.realtime_active_notes.len()
    }
    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        Some(self.control.clone())
    }
}

/// Present the exact current callback length to `vst3-host` while retaining the allocation made
/// for the maximum block. `vst3-host` 0.9 derives a bus-aware call's frame count from the first
/// channel vector length before consulting `BusAudioBuffers::block_size`. Leaving the vectors at
/// `MAX_BLOCK_SIZE` therefore advances a plug-in by (for example) 2048 samples while MiniStudio
/// consumes only a 256-sample callback. Stateful instruments then jump phase at every boundary,
/// which sounds like a wrong note or a telephone/8-bit ring. Truncating and growing within the
/// original capacity is allocation-free and makes the VST3 `numSamples` contract exact.
fn prepare_vst_bus_block(buffers: &mut BusAudioBuffers, frames: usize) {
    buffers.block_size = frames;
    for bus in buffers.inputs.iter_mut().chain(&mut buffers.outputs) {
        for channel in &mut bus.channels {
            channel.resize(frames, 0.0);
            channel.fill(0.0);
        }
    }
}

#[cfg(test)]
mod vst_bus_block_tests {
    use super::*;
    use vst3_host::audio::{AudioBusConfig, AudioBusLayout};

    #[test]
    fn bus_channels_expose_the_exact_callback_length_without_losing_capacity() {
        let layout = AudioBusLayout {
            inputs: vec![AudioBusConfig {
                channel_count: 2,
                active: true,
            }],
            outputs: vec![AudioBusConfig {
                channel_count: 2,
                active: true,
            }],
        };
        let mut buffers = BusAudioBuffers::new(&layout, MAX_BLOCK_SIZE, 48_000.0);
        let capacities = buffers
            .inputs
            .iter()
            .chain(&buffers.outputs)
            .flat_map(|bus| &bus.channels)
            .map(Vec::capacity)
            .collect::<Vec<_>>();

        prepare_vst_bus_block(&mut buffers, 256);
        assert_eq!(buffers.block_size, 256);
        assert!(buffers
            .inputs
            .iter()
            .chain(&buffers.outputs)
            .flat_map(|bus| &bus.channels)
            .all(|channel| channel.len() == 256));

        prepare_vst_bus_block(&mut buffers, 1024);
        assert_eq!(buffers.block_size, 1024);
        assert!(buffers
            .inputs
            .iter()
            .chain(&buffers.outputs)
            .flat_map(|bus| &bus.channels)
            .all(|channel| channel.len() == 1024));
        assert_eq!(
            capacities,
            buffers
                .inputs
                .iter()
                .chain(&buffers.outputs)
                .flat_map(|bus| &bus.channels)
                .map(Vec::capacity)
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(all(test, target_os = "windows"))]
mod isolated_realtime_hardware_tests {
    use super::*;

    /// Physical end-to-end regression for the signed self-host executable, named mapping,
    /// request/response events, timestamped MIDI, and bus-preserving audio return path.
    #[test]
    #[ignore = "requires the built MiniStudio executable and locally installed Serum VST3"]
    fn isolated_shared_memory_serum_renders() {
        let executable = PathBuf::from(
            std::env::var_os("MINISTUDIO_SELF_HOST_EXE")
                .expect("MINISTUDIO_SELF_HOST_EXE must point to ministudio.exe"),
        );
        let path = PathBuf::from(
            std::env::var_os("MINISTUDIO_TEST_SERUM_VST3")
                .expect("MINISTUDIO_TEST_SERUM_VST3 must point to Serum.vst3"),
        );
        let uid = std::env::var("MINISTUDIO_TEST_SERUM_UID")
            .unwrap_or_else(|_| "56535458667358736572756D00000000".to_string());
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(MAX_BLOCK_SIZE)
            .input_channels(MAX_CHANNELS)
            .output_channels(MAX_CHANNELS)
            .with_process_isolation(true)
            .self_hosted_helper(executable, "--ministudio-vst3-host")
            .build()
            .expect("build isolated host");
        let mut plugin = host
            .load_plugin_class(path, &uid)
            .expect("load Serum in helper");
        plugin.start_processing().expect("start Serum");
        let mut buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .expect("create bus buffers");
        plugin
            .send_midi_event_at(
                MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: 60,
                    velocity: 110,
                },
                0,
            )
            .expect("queue note on");
        let started = std::time::Instant::now();
        let mut energy = 0.0f64;
        let mut samples = 0usize;
        for _ in 0..96 {
            prepare_vst_bus_block(&mut buffers, 256);
            plugin
                .process_bus_audio(&mut buffers)
                .expect("process shared-memory block");
            for sample in buffers
                .outputs
                .iter()
                .flat_map(|bus| &bus.channels)
                .flat_map(|channel| channel.iter())
            {
                energy += f64::from(*sample) * f64::from(*sample);
                samples += 1;
            }
        }
        let elapsed = started.elapsed();
        let rms = (energy / samples.max(1) as f64).sqrt();
        assert!(rms > 1.0e-5, "isolated Serum render was silent: {rms}");
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "96 realtime blocks took {elapsed:?}"
        );
        eprintln!("shared-memory Serum: 96x256 frames in {elapsed:?}, rms={rms:.6}");
        plugin
            .open_isolated_editor()
            .expect("open Serum in the custom host window");
        std::thread::sleep(std::time::Duration::from_millis(500));
        plugin.close_editor().expect("close Serum editor");
        plugin.stop_processing().expect("stop Serum");
    }

    #[test]
    #[ignore = "requires the built MiniStudio executable and locally installed Serum VST3"]
    fn isolated_serum_audio_does_not_wait_for_control_mutex() {
        let executable = PathBuf::from(
            std::env::var_os("MINISTUDIO_SELF_HOST_EXE")
                .expect("MINISTUDIO_SELF_HOST_EXE must point to ministudio.exe"),
        );
        let path = PathBuf::from(
            std::env::var_os("MINISTUDIO_TEST_SERUM_VST3")
                .expect("MINISTUDIO_TEST_SERUM_VST3 must point to Serum.vst3"),
        );
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(MAX_BLOCK_SIZE)
            .input_channels(MAX_CHANNELS)
            .output_channels(MAX_CHANNELS)
            .with_process_isolation(true)
            .self_hosted_helper(executable, "--ministudio-vst3-host")
            .build()
            .expect("build isolated host");
        let mut plugin = host
            .load_plugin_class(path, "56535458667358736572756D00000000")
            .expect("load Serum in helper");
        plugin.start_processing().expect("start Serum");
        let mut buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .expect("create bus buffers");
        let mut realtime = plugin
            .take_isolated_realtime()
            .expect("isolated realtime endpoint");
        let control_plugin = Arc::new(Mutex::new(plugin));
        let stalled_plugin = Arc::clone(&control_plugin);
        let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(1);
        let stall = std::thread::spawn(move || {
            let _guard = stalled_plugin.lock().expect("control mutex");
            locked_tx.send(()).expect("announce control stall");
            std::thread::sleep(std::time::Duration::from_millis(500));
        });
        locked_rx.recv().expect("control mutex acquired");

        realtime
            .queue_midi(
                MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: 60,
                    velocity: 110,
                },
                0,
            )
            .expect("queue realtime note");
        let started = std::time::Instant::now();
        let mut peak = 0.0_f32;
        for _ in 0..96 {
            prepare_vst_bus_block(&mut buffers, 256);
            realtime
                .process_buses(&mut buffers)
                .expect("process while control mutex is stalled");
            for sample in buffers
                .outputs
                .iter()
                .flat_map(|bus| &bus.channels)
                .flat_map(|channel| channel.iter())
            {
                peak = peak.max(sample.abs());
            }
        }
        let elapsed = started.elapsed();
        assert!(peak > 1.0e-4, "control stall made realtime output silent");
        assert!(
            elapsed < std::time::Duration::from_millis(300),
            "realtime endpoint waited for the 500 ms control stall: {elapsed:?}"
        );
        stall.join().expect("control stall thread");
        control_plugin
            .lock()
            .expect("control plugin after stall")
            .stop_processing()
            .expect("stop Serum");
    }

    #[test]
    #[ignore = "requires the built MiniStudio executable and locally installed Serum VST3"]
    fn isolated_serum_editor_open_is_concurrent_with_realtime_audio() {
        let executable = PathBuf::from(
            std::env::var_os("MINISTUDIO_SELF_HOST_EXE")
                .expect("MINISTUDIO_SELF_HOST_EXE must point to ministudio.exe"),
        );
        let path = PathBuf::from(
            std::env::var_os("MINISTUDIO_TEST_SERUM_VST3")
                .expect("MINISTUDIO_TEST_SERUM_VST3 must point to Serum.vst3"),
        );
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(MAX_BLOCK_SIZE)
            .input_channels(MAX_CHANNELS)
            .output_channels(MAX_CHANNELS)
            .with_process_isolation(true)
            .self_hosted_helper(executable, "--ministudio-vst3-host")
            .build()
            .expect("build isolated host");
        let mut plugin = host
            .load_plugin_class(path, "56535458667358736572756D00000000")
            .expect("load Serum in helper");
        plugin.start_processing().expect("start Serum");
        let mut buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .expect("create bus buffers");
        let mut realtime = plugin
            .take_isolated_realtime()
            .expect("isolated realtime endpoint");
        let recovery = realtime.recovery_handle();
        let control = Arc::new(Mutex::new(plugin));
        let open_control = Arc::clone(&control);
        let start = Arc::new(std::sync::Barrier::new(2));
        let open_start = Arc::clone(&start);
        let open = std::thread::spawn(move || {
            open_start.wait();
            open_control
                .lock()
                .expect("editor control mutex")
                .open_isolated_editor()
                .expect("open Serum while audio is active");
        });

        realtime
            .queue_midi(
                MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: 60,
                    velocity: 110,
                },
                0,
            )
            .expect("queue realtime note");
        start.wait();
        let mut peak = 0.0_f32;
        for _ in 0..192 {
            prepare_vst_bus_block(&mut buffers, 256);
            realtime
                .process_buses(&mut buffers)
                .expect("native editor creation must not stall realtime audio");
            for sample in buffers
                .outputs
                .iter()
                .flat_map(|bus| &bus.channels)
                .flat_map(|channel| channel.iter())
            {
                peak = peak.max(sample.abs());
            }
        }
        open.join().expect("Serum editor thread");
        assert!(peak > 1.0e-4, "editor open made realtime output silent");
        assert_eq!(
            recovery.deadline_misses(),
            0,
            "editor creation crossed the realtime deadline"
        );
        let mut plugin = control.lock().expect("control after editor test");
        plugin.close_editor().expect("close Serum editor");
        plugin.stop_processing().expect("stop Serum");
    }

    #[test]
    #[ignore = "requires the built MiniStudio executable and locally installed Serum VST3"]
    fn isolated_worker_recovers_and_reattaches_the_same_mapping() {
        use winapi::{
            shared::minwindef::FALSE,
            um::{
                handleapi::CloseHandle,
                processthreadsapi::{OpenProcess, TerminateProcess},
                winnt::PROCESS_TERMINATE,
            },
        };

        let executable = PathBuf::from(
            std::env::var_os("MINISTUDIO_SELF_HOST_EXE")
                .expect("MINISTUDIO_SELF_HOST_EXE must point to ministudio.exe"),
        );
        let path = PathBuf::from(
            std::env::var_os("MINISTUDIO_TEST_SERUM_VST3")
                .expect("MINISTUDIO_TEST_SERUM_VST3 must point to Serum.vst3"),
        );
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(MAX_BLOCK_SIZE)
            .input_channels(MAX_CHANNELS)
            .output_channels(MAX_CHANNELS)
            .with_process_isolation(true)
            .self_hosted_helper(executable, "--ministudio-vst3-host")
            .build()
            .expect("build isolated host");
        let mut plugin = host
            .load_plugin_class(path, "56535458667358736572756D00000000")
            .expect("load Serum in helper");
        plugin.start_processing().expect("start Serum");
        let original_pid = plugin.isolation_pid().expect("helper pid");
        unsafe {
            let process = OpenProcess(PROCESS_TERMINATE, FALSE, original_pid);
            assert!(!process.is_null(), "open helper process");
            assert_ne!(TerminateProcess(process, 91), 0, "terminate helper");
            CloseHandle(process);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        plugin.recover().expect("recover isolated Serum");
        let replacement_pid = plugin.isolation_pid().expect("replacement pid");
        assert_ne!(replacement_pid, original_pid);
        assert_eq!(plugin.recovery_count(), 1);

        let mut buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .expect("create buffers after recovery");
        plugin
            .send_midi_event_at(
                MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: 64,
                    velocity: 110,
                },
                0,
            )
            .expect("queue note after recovery");
        let mut peak = 0.0f32;
        for _ in 0..32 {
            prepare_vst_bus_block(&mut buffers, 256);
            plugin
                .process_bus_audio(&mut buffers)
                .expect("process recovered block");
            for sample in buffers
                .outputs
                .iter()
                .flat_map(|bus| &bus.channels)
                .flat_map(|channel| channel.iter())
            {
                peak = peak.max(sample.abs());
            }
        }
        assert!(peak > 1.0e-4, "recovered shared-memory render was silent");
        plugin.stop_processing().expect("stop recovered Serum");
    }

    #[test]
    #[ignore = "requires the built MiniStudio executable and locally installed BBC SO VST3"]
    fn isolated_shared_memory_bbc_editor_opens_and_closes() {
        use winapi::{
            shared::{
                minwindef::{BOOL, FALSE, LPARAM, TRUE},
                windef::HWND,
            },
            um::winuser::{
                EnumWindows, GetClientRect, GetDpiForWindow, GetWindow, GetWindowLongPtrW,
                GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
                PostMessageW, ShowWindow, GWL_EXSTYLE, GWL_STYLE, GW_CHILD, SW_RESTORE,
                WM_LBUTTONUP, WS_CAPTION, WS_EX_TOPMOST,
            },
        };

        struct WindowSearch {
            pid: u32,
            hwnd: HWND,
        }
        unsafe extern "system" fn find_helper_window(hwnd: HWND, context: LPARAM) -> BOOL {
            let search = &mut *(context as *mut WindowSearch);
            let mut pid = 0_u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == search.pid && IsWindowVisible(hwnd) != 0 {
                search.hwnd = hwnd;
                FALSE
            } else {
                TRUE
            }
        }
        fn helper_window(pid: u32) -> HWND {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let mut search = WindowSearch {
                    pid,
                    hwnd: std::ptr::null_mut(),
                };
                unsafe {
                    EnumWindows(
                        Some(find_helper_window),
                        (&mut search as *mut WindowSearch) as LPARAM,
                    );
                }
                if !search.hwnd.is_null() {
                    return search.hwnd;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "helper editor window did not appear"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        fn click(hwnd: HWND, x: i32, y: i32) {
            let packed = (u32::from(x as u16) | (u32::from(y as u16) << 16)) as LPARAM;
            unsafe { PostMessageW(hwnd, WM_LBUTTONUP, 0, packed) };
        }

        let executable = PathBuf::from(
            std::env::var_os("MINISTUDIO_SELF_HOST_EXE")
                .expect("MINISTUDIO_SELF_HOST_EXE must point to ministudio.exe"),
        );
        let path = PathBuf::from(
            std::env::var_os("MINISTUDIO_TEST_BBC_VST3")
                .expect("MINISTUDIO_TEST_BBC_VST3 must point to BBC SO.vst3"),
        );
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(MAX_BLOCK_SIZE)
            .input_channels(MAX_CHANNELS)
            .output_channels(MAX_CHANNELS)
            .with_process_isolation(true)
            .self_hosted_helper(executable, "--ministudio-vst3-host")
            .build()
            .expect("build isolated host");
        let mut plugin = host
            .load_plugin_class(path, "56535453616E746262632073796D7068")
            .expect("load BBC in helper");
        plugin.start_processing().expect("start BBC");
        let mut buffers = plugin
            .create_bus_audio_buffers(MAX_BLOCK_SIZE)
            .expect("create BBC bus buffers");
        let mut realtime = plugin
            .take_isolated_realtime()
            .expect("BBC realtime endpoint");
        let editor_actions = realtime.editor_action_reader();
        let recovery = realtime.recovery_handle();
        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let opener = scope.spawn(|| {
                start.wait();
                plugin
                    .open_isolated_editor()
                    .expect("open BBC helper-owned editor");
            });
            start.wait();
            for _ in 0..96 {
                prepare_vst_bus_block(&mut buffers, 256);
                realtime
                    .process_buses(&mut buffers)
                    .expect("BBC editor creation must not stall realtime audio");
            }
            opener.join().expect("BBC editor thread");
        });
        assert_eq!(
            recovery.deadline_misses(),
            0,
            "BBC editor creation crossed the realtime deadline"
        );
        assert_eq!(plugin.get_editor_size().expect("editor size"), (1083, 917));
        let hwnd = helper_window(plugin.isolation_pid().expect("helper pid"));
        plugin
            .set_editor_host_state(vst3_host::process_isolation::EditorHostState {
                pinned: true,
                ..Default::default()
            })
            .expect("pin native host window");
        let pin_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 } & WS_EX_TOPMOST == 0
            && std::time::Instant::now() < pin_deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 } & WS_EX_TOPMOST,
            0,
            "acknowledged pin must apply native topmost state"
        );
        plugin
            .set_editor_host_state(Default::default())
            .expect("unpin native host window");
        let unpin_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 } & WS_EX_TOPMOST != 0
            && std::time::Instant::now() < unpin_deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 } & WS_EX_TOPMOST,
            0,
            "acknowledged unpin must restore ordinary z-order"
        );
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            assert_eq!(style & WS_CAPTION, 0, "system caption must stay hidden");
            let child = GetWindow(hwnd, GW_CHILD);
            assert!(
                !child.is_null(),
                "plug-in editor must have a host-owned child container"
            );
            let mut outer = std::mem::zeroed();
            let mut inner = std::mem::zeroed();
            assert_ne!(GetWindowRect(hwnd, &mut outer), 0);
            assert_ne!(GetWindowRect(child, &mut inner), 0);
            assert!(
                inner.top > outer.top,
                "custom chrome must sit above the plug-in pixels"
            );

            let mut client = std::mem::zeroed();
            assert_ne!(GetClientRect(hwnd, &mut client), 0);
            let dpi = GetDpiForWindow(hwnd).max(96);
            let chrome = (64 * dpi as i32 + 48) / 96;
            let button = (42 * dpi as i32 + 48) / 96;
            click(hwnd, client.right - button - button / 2, chrome / 2);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while IsIconic(hwnd) == 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_ne!(
                IsIconic(hwnd),
                0,
                "custom minimize button must minimize the host"
            );
            ShowWindow(hwnd, SW_RESTORE);
            click(hwnd, client.right - button / 2, chrome / 2);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while IsWindow(hwnd) != 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(
                IsWindow(hwnd),
                0,
                "custom close button must tear the editor down"
            );
        }
        assert!(
            editor_actions
                .wait_and_take(std::time::Duration::from_secs(1))
                .contains(&vst3_host::process_isolation::EditorHostAction::Closed),
            "custom close action must wake the event-driven host reader"
        );
        plugin.close_editor().expect("close BBC editor");
        plugin.stop_processing().expect("stop BBC");
    }
}

#[cfg(test)]
mod vst3_editor_proxy_tests {
    use super::*;

    #[test]
    fn resize_is_coalesced_and_drop_requests_owner_thread_teardown() {
        let (sender, receiver) = channel();
        let editor_open = Arc::new(AtomicBool::new(true));
        let pending_editor_rect = Arc::new(Mutex::new(None));
        {
            let mut proxy = Vst3EmbeddedEditor {
                sender,
                editor_open: Arc::clone(&editor_open),
                pending_editor_rect: Arc::clone(&pending_editor_rect),
            };
            proxy.set_rect(0.0, 52.0, 640.0, 480.0);
            proxy.set_rect(4.0, 56.0, 800.0, 600.0);
        }

        assert!(!editor_open.load(Ordering::Acquire));
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(50)),
            Ok(Vst3ControlCommand::CloseEmbedded)
        ));
        let latest = pending_editor_rect
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
            .expect("latest resize should remain available to the GUI worker");
        assert_eq!(latest.x, 4.0);
        assert_eq!(latest.y, 56.0);
        assert_eq!(latest.width, 800.0);
        assert_eq!(latest.height, 600.0);
    }

    #[test]
    fn realtime_fault_burst_queues_only_one_helper_recovery() {
        let (sender, receiver) = channel();
        let pending = AtomicBool::new(false);
        assert!(queue_vst3_recovery_once(&pending, &sender));
        assert!(!queue_vst3_recovery_once(&pending, &sender));
        assert!(matches!(
            receiver.try_recv(),
            Ok(Vst3ControlCommand::Recover)
        ));
        assert!(receiver.try_recv().is_err());

        pending.store(false, Ordering::Release);
        assert!(queue_vst3_recovery_once(&pending, &sender));
        assert!(matches!(
            receiver.try_recv(),
            Ok(Vst3ControlCommand::Recover)
        ));
    }

    /// Manual cross-vendor smoke test for the exact production owner-thread
    /// path, without involving Tauri/WebView. Example:
    ///
    /// `MINISTUDIO_VST3_SMOKE_PATH=... MINISTUDIO_VST3_SMOKE_UID=... cargo test
    /// -p ministudio-plugin installed_vst3_editor_opens -- --ignored`
    #[test]
    #[ignore = "opens an installed vendor plug-in's native editor"]
    fn installed_vst3_editor_opens() {
        let path = std::env::var("MINISTUDIO_VST3_SMOKE_PATH")
            .expect("MINISTUDIO_VST3_SMOKE_PATH is required");
        let uid = std::env::var("MINISTUDIO_VST3_SMOKE_UID")
            .expect("MINISTUDIO_VST3_SMOKE_UID is required");
        let spec = InstrumentSpec {
            id: "manual-vst3-editor-smoke".into(),
            kind: format!("vst3:{uid}"),
            params: HashMap::new(),
            bypassed: false,
            plugin: Some(ministudio_contracts::ExternalPluginRef {
                format: "vst3".into(),
                uid,
                name: "Manual VST3 smoke".into(),
                vendor: String::new(),
                path,
                audio_input_buses: 0,
                audio_output_buses: 0,
                supports_sidechain: false,
                has_editor: true,
                state: Vec::new(),
            }),
        };
        let instrument = try_create_external_instrument(&spec, 48_000.0)
            .expect("installed VST3 instance should initialize");
        let control = instrument
            .plugin_control()
            .expect("installed VST3 should expose a control handle");
        control
            .open_editor()
            .expect("installed VST3 editor should open");
        assert!(control.is_editor_open());
        std::thread::sleep(std::time::Duration::from_millis(750));
        control
            .close_editor()
            .expect("installed VST3 editor should close");
        assert!(!control.is_editor_open());
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
    try_create_external_effect(spec, sample_rate).ok()
}

/// Result-returning construction is the production path. A native plug-in
/// failure must reach the project loader instead of being collapsed into an
/// absent device, which otherwise leaves a silent rack card with the deeply
/// misleading "instance is not available" editor error.
pub fn try_create_external_effect(
    spec: &EffectSpec,
    sample_rate: f32,
) -> Result<Box<dyn DspEffect>, String> {
    match spec
        .plugin
        .as_ref()
        .ok_or("external effect is missing its plugin reference")?
        .format
        .as_str()
    {
        "vst3" => {
            Vst3Effect::new(spec, sample_rate).map(|effect| Box::new(effect) as Box<dyn DspEffect>)
        }
        "clap" => {
            ClapEffect::new(spec, sample_rate).map(|effect| Box::new(effect) as Box<dyn DspEffect>)
        }
        format => Err(format!("unsupported external effect format: {format}")),
    }
}

pub fn create_external_instrument(
    spec: &InstrumentSpec,
    sample_rate: f32,
) -> Option<Box<dyn Instrument>> {
    try_create_external_instrument(spec, sample_rate).ok()
}

/// Result-returning variant used by diagnostics and hosts that can surface a
/// vendor plug-in's initialization error instead of reducing it to `None`.
pub fn try_create_external_instrument(
    spec: &InstrumentSpec,
    sample_rate: f32,
) -> Result<Box<dyn Instrument>, String> {
    match spec
        .plugin
        .as_ref()
        .ok_or("external instrument is missing its plugin reference")?
        .format
        .as_str()
    {
        "vst3" => Vst3Instrument::new(spec, sample_rate)
            .map(|instrument| Box::new(instrument) as Box<dyn Instrument>),
        "clap" => ClapInstrument::new(spec, sample_rate)
            .map(|instrument| Box::new(instrument) as Box<dyn Instrument>),
        format => Err(format!("unsupported external instrument format: {format}")),
    }
}

struct ClapRuntime {
    processor: Option<StartedPluginAudioProcessor<ClapHost>>,
    owner_commands: SyncSender<ClapOwnerCommand>,
    control: Arc<ClapControl>,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    input_data: Vec<Vec<Vec<f32>>>,
    output_data: Vec<Vec<Vec<f32>>>,
    main_input: Option<usize>,
    main_output: Option<usize>,
    input_events: EventBuffer,
}

struct ClapReady {
    processor: StartedPluginAudioProcessor<ClapHost>,
    inputs: Vec<ClapPortLayout>,
    outputs: Vec<ClapPortLayout>,
}

enum ClapOwnerCommand {
    Callback,
    Open(Sender<Result<(), String>>),
    Close(Sender<Result<(), String>>),
    SaveState(Sender<Result<Vec<u8>, String>>),
    LoadState(Vec<u8>, Sender<Result<(), String>>),
    Shutdown(StoppedPluginAudioProcessor<ClapHost>),
}

struct ClapHost;

struct ClapHostShared {
    editor_open: Arc<AtomicBool>,
    owner_commands: SyncSender<ClapOwnerCommand>,
    callback_pending: Arc<AtomicBool>,
}

impl SharedHandler<'_> for ClapHostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {
        // CLAP permits this callback from arbitrary threads. A bounded try-send wakes the owner
        // without blocking a realtime caller and replaces the former permanent 8 ms poll loop.
        // The atomic coalesces repeated requests and preserves one pending callback even when the
        // bounded channel is temporarily full of lifecycle commands.
        if !self.callback_pending.swap(true, Ordering::AcqRel) {
            let _ = self.owner_commands.try_send(ClapOwnerCommand::Callback);
        }
    }
}

impl HostGuiImpl for ClapHostShared {
    fn resize_hints_changed(&self) {}
    fn request_resize(&self, _new_size: GuiSize) -> Result<(), HostError> {
        Ok(())
    }
    fn request_show(&self) -> Result<(), HostError> {
        self.editor_open.store(true, Ordering::Release);
        Ok(())
    }
    fn request_hide(&self) -> Result<(), HostError> {
        self.editor_open.store(false, Ordering::Release);
        Ok(())
    }
    fn closed(&self, _was_destroyed: bool) {
        self.editor_open.store(false, Ordering::Release);
    }
}

impl HostHandlers for ClapHost {
    type Shared<'a> = ClapHostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostGui>();
    }
}

struct ClapControl {
    sender: SyncSender<ClapOwnerCommand>,
    editor_open: Arc<AtomicBool>,
    has_editor: Arc<AtomicBool>,
}

impl ClapControl {
    fn request<T>(
        &self,
        build: impl FnOnce(Sender<Result<T, String>>) -> ClapOwnerCommand,
    ) -> Result<T, String> {
        let (reply, receive) = channel();
        self.sender
            .send(build(reply))
            .map_err(|_| "CLAP control thread is unavailable".to_owned())?;
        receive
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "CLAP control operation timed out".to_owned())?
    }
}

impl PluginControl for ClapControl {
    fn has_editor(&self) -> bool {
        self.has_editor.load(Ordering::Acquire)
    }
    fn open_editor(&self) -> Result<(), String> {
        if !self.has_editor() {
            return Err("This CLAP plug-in does not provide a floating editor".into());
        }
        self.request(ClapOwnerCommand::Open)
    }
    fn close_editor(&self) -> Result<(), String> {
        self.request(ClapOwnerCommand::Close)
    }
    fn is_editor_open(&self) -> bool {
        self.editor_open.load(Ordering::Acquire)
    }
    fn save_state(&self) -> Result<Vec<u8>, String> {
        self.request(ClapOwnerCommand::SaveState)
    }
    fn load_state(&self, state: Vec<u8>) -> Result<(), String> {
        self.request(|reply| ClapOwnerCommand::LoadState(state, reply))
    }
}

fn clap_gui_configuration(is_floating: bool) -> Option<GuiConfiguration<'static>> {
    #[cfg(target_os = "windows")]
    let api_type = GuiApiType::WIN32;
    #[cfg(target_os = "macos")]
    let api_type = GuiApiType::COCOA;
    #[cfg(target_os = "linux")]
    let api_type = GuiApiType::X11;
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    return None;
    Some(GuiConfiguration {
        api_type,
        is_floating,
    })
}

fn clap_has_editor(instance: &mut PluginInstance<ClapHost>) -> bool {
    let Some(floating) = clap_gui_configuration(true) else {
        return false;
    };
    let mut plugin = instance.plugin_handle();
    plugin
        .get_extension::<PluginGui>()
        .is_some_and(|gui| gui.is_api_supported(&mut plugin, floating))
}

fn open_clap_editor(instance: &mut PluginInstance<ClapHost>) -> Result<(), String> {
    let configuration = clap_gui_configuration(true).ok_or("No CLAP GUI API for this platform")?;
    let mut plugin = instance.plugin_handle();
    let gui = plugin
        .get_extension::<PluginGui>()
        .ok_or("CLAP plug-in does not expose the GUI extension")?;
    if !gui.is_api_supported(&mut plugin, configuration) {
        return Err("CLAP plug-in does not support a floating editor on this platform".into());
    }
    gui.create(&mut plugin, configuration)
        .map_err(|error| error.to_string())?;
    if let Err(error) = gui.show(&mut plugin) {
        gui.destroy(&mut plugin);
        return Err(error.to_string());
    }
    keep_hosting_windows_above_daw();
    Ok(())
}

fn close_clap_editor(instance: &mut PluginInstance<ClapHost>) {
    let mut plugin = instance.plugin_handle();
    if let Some(gui) = plugin.get_extension::<PluginGui>() {
        let _ = gui.hide(&mut plugin);
        gui.destroy(&mut plugin);
    }
}

fn save_clap_state(instance: &mut PluginInstance<ClapHost>) -> Result<Vec<u8>, String> {
    let mut plugin = instance.plugin_handle();
    let state = plugin
        .get_extension::<PluginState>()
        .ok_or("CLAP plug-in does not expose the state extension")?;
    let mut bytes = Vec::new();
    state
        .save(&mut plugin, &mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn load_clap_state(instance: &mut PluginInstance<ClapHost>, bytes: &[u8]) -> Result<(), String> {
    let mut plugin = instance.plugin_handle();
    let state = plugin
        .get_extension::<PluginState>()
        .ok_or("CLAP plug-in does not expose the state extension")?;
    state
        .load(&mut plugin, &mut std::io::Cursor::new(bytes))
        .map_err(|error| error.to_string())
}

impl ClapRuntime {
    fn new(path: &str, uid: &str, sample_rate: f32, expects_input: bool) -> Result<Self, String> {
        let path = path.to_owned();
        let uid = uid.to_owned();
        let (ready_tx, ready_rx) = sync_channel::<Result<ClapReady, String>>(1);
        let (owner_commands, owner_rx) = sync_channel::<ClapOwnerCommand>(16);
        let editor_open = Arc::new(AtomicBool::new(false));
        let has_editor = Arc::new(AtomicBool::new(false));
        let callback_pending = Arc::new(AtomicBool::new(false));
        let thread_editor_open = Arc::clone(&editor_open);
        let thread_has_editor = Arc::clone(&has_editor);
        let thread_callback_pending = Arc::clone(&callback_pending);
        let shared_owner_commands = owner_commands.clone();
        std::thread::Builder::new()
            .name("ministudio-clap-owner".into())
            .spawn(move || {
                let result = (|| -> Result<(PluginEntry, PluginInstance<ClapHost>, StartedPluginAudioProcessor<ClapHost>, Vec<ClapPortLayout>, Vec<ClapPortLayout>), String> {
                    let entry = unsafe { PluginEntry::load(OsStr::new(&path)) }
                        .map_err(|error| error.to_string())?;
                    let uid = CString::new(uid).map_err(|_| "CLAP plugin ID contains a NUL byte")?;
                    let host_info = HostInfo::new(
                        "MiniStudio",
                        "MiniStudio",
                        "https://github.com",
                        env!("CARGO_PKG_VERSION"),
                    )
                    .map_err(|error| error.to_string())?;
                    let shared_editor_open = Arc::clone(&thread_editor_open);
                    let mut instance = PluginInstance::<ClapHost>::new(
                        |_| ClapHostShared {
                            editor_open: shared_editor_open,
                            owner_commands: shared_owner_commands,
                            callback_pending,
                        },
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
                        thread_has_editor.store(
                            clap_has_editor(&mut instance),
                            Ordering::Release,
                        );
                        if ready_tx.send(Ok(ClapReady { processor, inputs, outputs })).is_ok() {
                            let mut gui_open = false;
                            loop {
                                let command = match owner_rx.recv() {
                                    Ok(command) => command,
                                    Err(_) => break,
                                };
                                if matches!(&command, ClapOwnerCommand::Callback) {
                                    thread_callback_pending.store(false, Ordering::Release);
                                    instance.call_on_main_thread_callback();
                                    continue;
                                }
                                // If `try_send(Callback)` encountered a full channel, the first
                                // lifecycle command that frees a slot services the coalesced request.
                                if thread_callback_pending.swap(false, Ordering::AcqRel) {
                                    instance.call_on_main_thread_callback();
                                }
                                if gui_open && !thread_editor_open.load(Ordering::Acquire) {
                                    close_clap_editor(&mut instance);
                                    gui_open = false;
                                }
                                match command {
                                    ClapOwnerCommand::Callback => unreachable!(),
                                    ClapOwnerCommand::Open(reply) => {
                                        let result = if gui_open {
                                            Ok(())
                                        } else {
                                            open_clap_editor(&mut instance)
                                        };
                                        if result.is_ok() {
                                            gui_open = true;
                                            thread_editor_open.store(true, Ordering::Release);
                                        }
                                        let _ = reply.send(result);
                                    }
                                    ClapOwnerCommand::Close(reply) => {
                                        if gui_open {
                                            close_clap_editor(&mut instance);
                                            gui_open = false;
                                            thread_editor_open.store(false, Ordering::Release);
                                        }
                                        let _ = reply.send(Ok(()));
                                    }
                                    ClapOwnerCommand::SaveState(reply) => {
                                        let _ = reply.send(save_clap_state(&mut instance));
                                    }
                                    ClapOwnerCommand::LoadState(bytes, reply) => {
                                        let _ = reply.send(load_clap_state(&mut instance, &bytes));
                                    }
                                    ClapOwnerCommand::Shutdown(stopped) => {
                                        if gui_open {
                                            close_clap_editor(&mut instance);
                                        }
                                        thread_editor_open.store(false, Ordering::Release);
                                        instance.deactivate(stopped);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => { let _ = ready_tx.send(Err(error)); }
                }
            })
            .map_err(|error| error.to_string())?;
        let ready = ready_rx.recv().map_err(|error| error.to_string())??;
        let control = Arc::new(ClapControl {
            sender: owner_commands.clone(),
            editor_open,
            has_editor,
        });
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
            owner_commands,
            control,
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
            let _ = self
                .owner_commands
                .send(ClapOwnerCommand::Shutdown(processor.stop_processing()));
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
        let mut runtime = ClapRuntime::new(&reference.path, &reference.uid, sample_rate, true)?;
        if !reference.state.is_empty() {
            runtime
                .control
                .load_state(reference.state.clone())
                .map_err(|error| format!("failed to restore CLAP state: {error}"))?;
        }
        for (id, value) in &spec.params {
            if let Ok(id) = id.strip_prefix("param:").unwrap_or(id).parse() {
                runtime.push_parameter(id, *value);
            }
        }
        Ok(Self {
            runtime,
            scratch: AudioBuffer::new(),
            bypassed: spec.bypassed,
        })
    }
}
impl DspEffect for ClapEffect {
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        self.process_with_sidechain(events, buffer, None, frames)
    }
    fn process_with_sidechain(
        &mut self,
        _events: &[NoteEvent],
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
        if let Ok(id) = id.strip_prefix("param:").unwrap_or(id).parse() {
            self.runtime.push_parameter(id, value);
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }
    fn reset(&mut self) {
        self.runtime.reset();
    }
    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        Some(self.runtime.control.clone())
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
        let mut runtime = ClapRuntime::new(&reference.path, &reference.uid, sample_rate, false)?;
        if !reference.state.is_empty() {
            runtime
                .control
                .load_state(reference.state.clone())
                .map_err(|error| format!("failed to restore CLAP state: {error}"))?;
        }
        for (id, value) in &spec.params {
            if let Ok(id) = id.strip_prefix("param:").unwrap_or(id).parse() {
                runtime.push_parameter(id, *value);
            }
        }
        Ok(Self {
            runtime,
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
        if let Ok(id) = id.strip_prefix("param:").unwrap_or(id).parse() {
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
    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        Some(self.runtime.control.clone())
    }
}
