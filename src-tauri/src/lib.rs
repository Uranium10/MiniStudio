// Typed Tauri IPC surface for the native audio control plane and phase-three stubs.
use ministudio_audio::audio;

use audio::{
    engine::NativeEngine,
    error::EngineError,
    plugin::PluginDescriptor,
    types::{
        AudioBackendInfo, AudioDeviceInfo, AudioSettings, DecodeProgress, EngineSnapshot,
        EqFrequencyResponse, ExportProgress, ExportRequest, ExportResult, GraphSnapshot,
        MidiInputPortInfo, NativeAssetInfo,
    },
};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Typescript;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tauri::{
    ipc::{Channel, Response},
    AppHandle, Manager, State,
};
#[cfg(feature = "desktop")]
use tauri::{plugin::TauriPlugin, Runtime};
use tauri_specta::{collect_commands, Builder};

#[cfg(not(feature = "desktop"))]
type AppRuntime = tauri::test::MockRuntime;
#[cfg(feature = "desktop")]
type AppRuntime = tauri::Wry;

pub const WINDOWS_VST3_PATHS: &[&str] = &[r"C:\Program Files\Common Files\VST3"];
pub const MACOS_VST3_PATHS: &[&str] = &[
    "/Library/Audio/Plug-Ins/VST3",
    "~/Library/Audio/Plug-Ins/VST3",
];
pub const LINUX_VST3_PATHS: &[&str] = &["~/.vst3", "/usr/lib/vst3"];

struct NativeEngineState {
    engine: Arc<Mutex<NativeEngine>>,
    export_cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct PluginRegistryState {
    paths: Mutex<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PluginFingerprint {
    bytes: u64,
    files: u64,
    modified_millis: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginCacheEntry {
    format: String,
    path: String,
    fingerprint: PluginFingerprint,
    descriptors: Vec<PluginDescriptor>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginCacheFile {
    version: u32,
    entries: Vec<PluginCacheEntry>,
}

const PLUGIN_CACHE_VERSION: u32 = 2;
const PLUGIN_CACHE_FILE: &str = "plugin-scan-cache-v2.json";

impl Default for NativeEngineState {
    fn default() -> Self {
        Self {
            engine: Arc::new(Mutex::new(NativeEngine::default())),
            export_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn with_engine<T>(
    state: &State<'_, NativeEngineState>,
    f: impl FnOnce(&mut NativeEngine) -> Result<T, String>,
) -> Result<T, EngineError> {
    let mut engine = state
        .engine
        .lock()
        .map_err(|_| EngineError::Internal("native engine control lock is poisoned".into()))?;
    f(&mut engine).map_err(EngineError::from)
}

#[tauri::command]
#[specta::specta]
fn list_storage_roots() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        return (b'A'..=b'Z')
            .map(|letter| format!("{}:\\", letter as char))
            .filter(|root| std::path::Path::new(root).exists())
            .collect();
    }
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec!["/".to_owned()];
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            roots.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path().to_string_lossy().into_owned()),
            );
        }
        return roots;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec!["/".to_owned()]
    }
}

#[derive(Clone, Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
struct MediaDirectoryEntry {
    path: String,
    name: String,
    is_directory: bool,
}

#[tauri::command]
#[specta::specta]
fn list_media_directory(path: String) -> Result<Vec<MediaDirectoryEntry>, EngineError> {
    let mut result = Vec::new();
    let entries = std::fs::read_dir(&path).map_err(|error| {
        EngineError::Asset(format!("cannot read media directory {path}: {error}"))
    })?;
    for entry in entries.flatten() {
        if is_hidden_media_entry(&entry) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_directory = file_type.is_dir();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_audio = name
            .rsplit_once('.')
            .map(|(_, extension)| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "wav" | "mp3" | "flac" | "ogg" | "m4a" | "aac"
                )
            })
            .unwrap_or(false);
        if is_directory || (file_type.is_file() && is_audio) {
            result.push(MediaDirectoryEntry {
                path: entry.path().to_string_lossy().into_owned(),
                name,
                is_directory,
            });
        }
    }
    result.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(result)
}

#[tauri::command]
#[specta::specta]
async fn search_media_directory(
    path: String,
    query: String,
) -> Result<Vec<MediaDirectoryEntry>, EngineError> {
    tauri::async_runtime::spawn_blocking(move || search_media_directory_blocking(&path, &query))
        .await
        .map_err(|error| EngineError::Internal(error.to_string()))?
}

fn search_media_directory_blocking(
    path: &str,
    query: &str,
) -> Result<Vec<MediaDirectoryEntry>, EngineError> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let mut pending = vec![PathBuf::from(path)];
    let mut visited = 0_usize;
    while let Some(folder) = pending.pop() {
        visited += 1;
        if visited > 20_000 || result.len() >= 500 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if is_hidden_media_entry(&entry) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_file()
                && is_supported_audio_name(&name)
                && name.to_lowercase().contains(&needle)
            {
                result.push(MediaDirectoryEntry {
                    path: entry.path().to_string_lossy().into_owned(),
                    name,
                    is_directory: false,
                });
                if result.len() >= 500 {
                    break;
                }
            }
        }
    }
    result.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(result)
}

fn is_supported_audio_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "mp3" | "flac" | "ogg" | "m4a" | "aac"
            )
        })
        .unwrap_or(false)
}

fn is_hidden_media_entry(entry: &std::fs::DirEntry) -> bool {
    if entry.file_name().to_string_lossy().starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if entry
            .metadata()
            .map(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

#[tauri::command]
#[specta::specta]
fn engine_init(state: State<'_, NativeEngineState>) -> Result<(), EngineError> {
    with_engine(&state, NativeEngine::init)
}

#[tauri::command]
#[specta::specta]
fn engine_dispose(state: State<'_, NativeEngineState>) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.dispose();
        Ok(())
    })
}

#[tauri::command]
#[specta::specta]
async fn engine_load_audio_file(
    path: String,
    on_progress: Channel<DecodeProgress>,
    state: State<'_, NativeEngineState>,
) -> Result<NativeAssetInfo, EngineError> {
    let engine = Arc::clone(&state.engine);
    tauri::async_runtime::spawn_blocking(move || {
        let mut engine = engine
            .lock()
            .map_err(|_| EngineError::Internal("native engine control lock is poisoned".into()))?;
        engine
            .load(&path, |progress| {
                let _ = on_progress.send(progress);
            })
            .map_err(EngineError::from)
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?
}

#[tauri::command]
#[specta::specta]
fn engine_unload_asset(
    asset_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.unload(&asset_id);
        Ok(())
    })
}

#[tauri::command]
#[specta::specta]
fn engine_sync_graph(
    mut snapshot: GraphSnapshot,
    state: State<'_, NativeEngineState>,
    plugins: State<'_, PluginRegistryState>,
) -> Result<(), EngineError> {
    resolve_plugin_paths(&mut snapshot, &plugins);
    with_engine(&state, |engine| engine.sync_graph(snapshot))
}

fn resolve_plugin_paths(snapshot: &mut GraphSnapshot, registry: &PluginRegistryState) {
    let paths = registry
        .paths
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let resolve = |reference: &mut audio::types::ExternalPluginRef| {
        if let Some(path) = paths.get(&format!("{}:{}", reference.format, reference.uid)) {
            reference.path.clone_from(path);
        }
    };
    for track in &mut snapshot.tracks {
        if let Some(reference) = track
            .instrument
            .as_mut()
            .and_then(|instrument| instrument.plugin.as_mut())
        {
            resolve(reference);
        }
        for effect in &mut track.effects {
            if let Some(reference) = effect.plugin.as_mut() {
                resolve(reference);
            }
        }
    }
    for bus in &mut snapshot.buses {
        for effect in &mut bus.effects {
            if let Some(reference) = effect.plugin.as_mut() {
                resolve(reference);
            }
        }
    }
    for effect in &mut snapshot.master.effects {
        if let Some(reference) = effect.plugin.as_mut() {
            resolve(reference);
        }
    }
}

#[tauri::command]
#[specta::specta]
fn engine_play(
    from_sec: Option<f64>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.play(from_sec))
}

#[tauri::command]
#[specta::specta]
fn engine_pause(state: State<'_, NativeEngineState>) -> Result<(), EngineError> {
    with_engine(&state, NativeEngine::pause)
}

#[tauri::command]
#[specta::specta]
fn engine_stop(state: State<'_, NativeEngineState>) -> Result<(), EngineError> {
    with_engine(&state, NativeEngine::stop)
}

#[tauri::command]
#[specta::specta]
fn engine_seek(sec: f64, state: State<'_, NativeEngineState>) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.seek(sec))
}

#[tauri::command]
#[specta::specta]
fn engine_set_track_volume(
    track_id: String,
    gain_db: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_track_gain(&track_id, gain_db))
}

#[tauri::command]
#[specta::specta]
fn engine_set_track_pan(
    track_id: String,
    pan: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_track_pan(&track_id, pan))
}

#[tauri::command]
#[specta::specta]
fn engine_set_track_mute(
    track_id: String,
    muted: bool,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_track_mute(&track_id, muted))
}

#[tauri::command]
#[specta::specta]
fn engine_set_track_solo(
    track_id: String,
    solo: bool,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_track_solo(&track_id, solo))
}

#[tauri::command]
#[specta::specta]
fn engine_set_send_level(
    send_id: String,
    gain_db: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_send(&send_id, gain_db))
}

#[tauri::command]
#[specta::specta]
fn engine_set_bus_volume(
    bus_id: String,
    gain_db: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_bus_gain(&bus_id, gain_db))
}

#[tauri::command]
#[specta::specta]
fn engine_set_master_volume(
    gain_db: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_master_gain(gain_db))
}

#[tauri::command]
#[specta::specta]
fn engine_set_effect_param(
    effect_id: String,
    param_id: String,
    value: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.set_effect(&effect_id, &param_id, value)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_set_effect_bypass(
    effect_id: String,
    bypassed: bool,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.set_effect_bypassed(&effect_id, bypassed)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_set_instrument_param(
    track_id: String,
    param_id: String,
    value: f32,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.set_instrument_param(&track_id, &param_id, value)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_midi_note(
    track_id: String,
    note_id: i32,
    pitch: u8,
    velocity: f32,
    note_on: bool,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.midi_note(&track_id, note_id, pitch, velocity, note_on)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_midi_all_notes_off(
    track_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.midi_all_notes_off(&track_id))
}

#[tauri::command]
#[specta::specta]
fn engine_list_midi_inputs(
    state: State<'_, NativeEngineState>,
) -> Result<Vec<MidiInputPortInfo>, EngineError> {
    with_engine(&state, |engine| engine.midi_inputs())
}

#[tauri::command]
#[specta::specta]
fn engine_connect_midi_input(
    port_id: String,
    track_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.connect_midi_input(&port_id, &track_id)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_disconnect_midi_input(
    port_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        engine.disconnect_midi_input(&port_id);
        Ok(())
    })
}

#[tauri::command]
#[specta::specta]
fn engine_poll_state(state: State<'_, NativeEngineState>) -> Result<EngineSnapshot, EngineError> {
    with_engine(&state, |engine| Ok(engine.poll()))
}

#[tauri::command]
#[specta::specta]
fn engine_list_audio_backends(
    state: State<'_, NativeEngineState>,
) -> Result<Vec<AudioBackendInfo>, EngineError> {
    with_engine(&state, |engine| Ok(engine.backends()))
}

#[tauri::command]
#[specta::specta]
fn engine_list_output_devices(
    backend_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<Vec<AudioDeviceInfo>, EngineError> {
    with_engine(&state, |engine| engine.devices(&backend_id))
}

#[tauri::command]
#[specta::specta]
fn engine_get_audio_settings(
    state: State<'_, NativeEngineState>,
) -> Result<AudioSettings, EngineError> {
    with_engine(&state, |engine| Ok(engine.settings()))
}

#[tauri::command]
#[specta::specta]
fn engine_set_audio_settings(
    settings: AudioSettings,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.set_settings(settings))
}

#[tauri::command]
#[specta::specta]
async fn engine_export_project(
    request: ExportRequest,
    on_progress: Channel<ExportProgress>,
    state: State<'_, NativeEngineState>,
) -> Result<ExportResult, EngineError> {
    state.export_cancelled.store(false, Ordering::Relaxed);
    let engine = Arc::clone(&state.engine);
    let cancelled = Arc::clone(&state.export_cancelled);
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine
            .lock()
            .map_err(|_| EngineError::Internal("native engine control lock is poisoned".into()))?;
        engine
            .export(&request, &cancelled, |progress| {
                let _ = on_progress.send(progress);
            })
            .map_err(EngineError::from)
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?
}

#[tauri::command]
#[specta::specta]
fn engine_cancel_export(state: State<'_, NativeEngineState>) -> Result<(), EngineError> {
    state.export_cancelled.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
#[specta::specta]
fn engine_eq_response(
    effect_id: String,
    points: usize,
    state: State<'_, NativeEngineState>,
) -> Result<EqFrequencyResponse, EngineError> {
    with_engine(&state, |engine| {
        engine.eq_response(&effect_id, points.clamp(16, 2048))
    })
}

fn validate_plugin_target_kind(kind: &str) -> Result<(), String> {
    match kind {
        "effect" | "instrument" => Ok(()),
        _ => Err(format!("unknown plug-in target kind: {kind}")),
    }
}

#[tauri::command]
#[specta::specta]
fn engine_open_plugin_editor(
    target_kind: String,
    target_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        validate_plugin_target_kind(&target_kind)?;
        engine.open_plugin_editor(&target_id)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_close_plugin_editor(
    target_kind: String,
    target_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        validate_plugin_target_kind(&target_kind)?;
        engine.close_plugin_editor(&target_id)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_plugin_editor_is_open(
    target_kind: String,
    target_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<bool, EngineError> {
    with_engine(&state, |engine| {
        validate_plugin_target_kind(&target_kind)?;
        Ok(engine.plugin_editor_is_open(&target_id))
    })
}

#[tauri::command]
#[specta::specta]
fn engine_save_plugin_state(
    target_kind: String,
    target_id: String,
    state: State<'_, NativeEngineState>,
) -> Result<Vec<u8>, EngineError> {
    with_engine(&state, |engine| {
        validate_plugin_target_kind(&target_kind)?;
        engine.save_plugin_state(&target_id)
    })
}

#[tauri::command]
#[specta::specta]
fn engine_load_plugin_state(
    target_kind: String,
    target_id: String,
    plugin_state: Vec<u8>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| {
        validate_plugin_target_kind(&target_kind)?;
        engine.load_plugin_state(&target_id, plugin_state)
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OfflineRenderRequest {
    pub plugin_uid: String,
    pub input_wav_path: String,
    pub output_wav_path: String,
    pub sample_rate: u32,
    pub params: HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OfflineRenderResult {
    pub output_wav_path: String,
    pub duration_sec: f64,
    pub peak_db: f32,
}

#[tauri::command]
#[specta::specta]
async fn cached_vst3_plugins(
    app: AppHandle<AppRuntime>,
    registry: State<'_, PluginRegistryState>,
) -> Result<Vec<PluginDescriptor>, EngineError> {
    let cache = read_plugin_cache(&plugin_cache_path(&app)?);
    update_plugin_registry(&registry, &cache);
    Ok(cache_descriptors(&cache))
}

#[tauri::command]
#[specta::specta]
async fn scan_vst3_plugins(
    paths: Vec<String>,
    force: bool,
    app: AppHandle<AppRuntime>,
    registry: State<'_, PluginRegistryState>,
) -> Result<Vec<PluginDescriptor>, EngineError> {
    let cache_path = plugin_cache_path(&app)?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        scan_plugins_incremental(&paths, &cache_path, force)
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))??;
    update_plugin_registry(&registry, &result.1);
    Ok(result.0)
}

fn scan_plugins_incremental(
    paths: &[String],
    cache_path: &Path,
    force: bool,
) -> Result<(Vec<PluginDescriptor>, PluginCacheFile), EngineError> {
    let executable =
        std::env::current_exe().map_err(|error| EngineError::Internal(error.to_string()))?;
    let previous = read_plugin_cache(cache_path);
    let previous = previous
        .entries
        .into_iter()
        .map(|entry| ((entry.format.clone(), entry.path.clone()), entry))
        .collect::<HashMap<_, _>>();
    let binaries = audio::plugin::collect_plugin_binaries(paths);
    let mut entries = Vec::with_capacity(binaries.len());
    let mut changed = Vec::new();
    for (format, path) in binaries {
        let path_text = path.to_string_lossy().into_owned();
        let fingerprint = plugin_fingerprint(&path);
        let key = (format.clone(), path_text.clone());
        if !force {
            if let Some(entry) = previous.get(&key) {
                if entry.fingerprint == fingerprint {
                    entries.push(entry.clone());
                    continue;
                }
            }
        }
        changed.push((format, path, path_text, fingerprint));
    }
    let changed = Arc::new(changed);
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .min(8)
        .min(changed.len().max(1));
    let cursor = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let changed = Arc::clone(&changed);
            let cursor = Arc::clone(&cursor);
            let sender = sender.clone();
            let executable = executable.clone();
            scope.spawn(move || loop {
                let index = cursor.fetch_add(1, Ordering::Relaxed);
                let Some((format, path, path_text, fingerprint)) = changed.get(index) else {
                    break;
                };
                let descriptors = probe_plugin_isolated(&executable, format, path, true);
                let _ = sender.send(PluginCacheEntry {
                    format: format.clone(),
                    path: path_text.clone(),
                    fingerprint: fingerprint.clone(),
                    descriptors,
                });
            });
        }
        drop(sender);
        entries.extend(receiver);
    });
    entries.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.format.cmp(&b.format)));
    let cache = PluginCacheFile {
        version: PLUGIN_CACHE_VERSION,
        entries,
    };
    write_plugin_cache(cache_path, &cache)?;
    let mut plugins = cache_descriptors(&cache);
    plugins.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.uid.cmp(&b.uid))
    });
    plugins.dedup_by(|a, b| a.format == b.format && a.uid == b.uid && a.path == b.path);
    Ok((plugins, cache))
}

fn plugin_cache_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, EngineError> {
    app.path()
        .app_cache_dir()
        .map(|directory| directory.join(PLUGIN_CACHE_FILE))
        .map_err(|error| EngineError::Internal(error.to_string()))
}

fn read_plugin_cache(path: &Path) -> PluginCacheFile {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PluginCacheFile>(&bytes).ok())
        .filter(|cache| cache.version == PLUGIN_CACHE_VERSION)
        .unwrap_or_default()
}

fn write_plugin_cache(path: &Path, cache: &PluginCacheFile) -> Result<(), EngineError> {
    let parent = path
        .parent()
        .ok_or_else(|| EngineError::Internal("invalid plug-in cache path".into()))?;
    fs::create_dir_all(parent).map_err(|error| EngineError::Internal(error.to_string()))?;
    let temporary = path.with_extension("json.tmp");
    let bytes =
        serde_json::to_vec(cache).map_err(|error| EngineError::Internal(error.to_string()))?;
    fs::write(&temporary, bytes).map_err(|error| EngineError::Internal(error.to_string()))?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(temporary, path).map_err(|error| EngineError::Internal(error.to_string()))
}

fn cache_descriptors(cache: &PluginCacheFile) -> Vec<PluginDescriptor> {
    let mut descriptors = cache
        .entries
        .iter()
        .flat_map(|entry| entry.descriptors.clone())
        .collect::<Vec<_>>();
    descriptors.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.format.cmp(&b.format))
            .then_with(|| a.uid.cmp(&b.uid))
    });
    descriptors.dedup_by(|a, b| a.format == b.format && a.uid == b.uid && a.path == b.path);
    descriptors
}

fn update_plugin_registry(registry: &PluginRegistryState, cache: &PluginCacheFile) {
    let mut paths = registry
        .paths
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    paths.clear();
    for descriptor in cache.entries.iter().flat_map(|entry| &entry.descriptors) {
        paths.insert(
            format!("{}:{}", descriptor.format, descriptor.uid),
            descriptor.path.clone(),
        );
    }
}

fn plugin_fingerprint(path: &Path) -> PluginFingerprint {
    fn visit(path: &Path, result: &mut PluginFingerprint, depth: usize) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.is_file() {
            result.files += 1;
            result.bytes = result.bytes.saturating_add(metadata.len());
            if let Ok(modified) = metadata.modified().and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            }) {
                result.modified_millis = result.modified_millis.max(modified.as_millis());
            }
        } else if metadata.is_dir() && depth < 4 {
            if let Ok(children) = fs::read_dir(path) {
                for child in children.flatten() {
                    let child_path = child.path();
                    // VST3 bundles often contain thousands of presets and artwork files.
                    // Only code/metadata directories affect host compatibility; ignoring
                    // Resources keeps an unchanged startup scan effectively O(plug-ins).
                    if child_path
                        .file_name()
                        .is_some_and(|name| name.eq_ignore_ascii_case("Resources"))
                    {
                        continue;
                    }
                    visit(&child_path, result, depth + 1);
                }
            }
        }
    }
    let mut result = PluginFingerprint::default();
    visit(path, &mut result, 0);
    result
}

fn probe_plugin_isolated(
    executable: &std::path::Path,
    format: &str,
    path: &std::path::Path,
    catalog_only: bool,
) -> Vec<PluginDescriptor> {
    let mut child = match Command::new(executable)
        .arg("--minidaw-plugin-probe")
        .arg(format)
        .arg(path)
        .arg(if catalog_only { "catalog" } else { "full" })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };
    let timeout = if catalog_only { 8 } else { 30 };
    let completed = wait_for_probe_child(&mut child, std::time::Duration::from_secs(timeout));
    if !completed {
        return Vec::new();
    }
    if let Ok(output) = child.wait_with_output() {
        if let Ok(descriptors) = serde_json::from_slice::<Vec<PluginDescriptor>>(&output.stdout) {
            return descriptors;
        }
    }
    Vec::new()
}

#[tauri::command]
#[specta::specta]
fn engine_asset_peaks(
    asset_id: String,
    lod: u8,
    state: State<'_, NativeEngineState>,
) -> Result<Vec<f32>, EngineError> {
    with_engine(&state, |engine| {
        engine.asset_peaks(&asset_id, lod).map(ToOwned::to_owned)
    })
}

fn wait_for_probe_child(child: &mut std::process::Child, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod plugin_probe_tests {
    use super::*;

    #[test]
    fn hanging_plugin_probe_is_killed_at_the_deadline() {
        #[cfg(windows)]
        let mut child = Command::new("cmd")
            .args(["/C", "ping 127.0.0.1 -n 20 > nul"])
            .spawn()
            .expect("spawn timeout fixture");
        #[cfg(not(windows))]
        let mut child = Command::new("sh")
            .args(["-c", "sleep 20"])
            .spawn()
            .expect("spawn timeout fixture");
        let started = std::time::Instant::now();
        assert!(!wait_for_probe_child(
            &mut child,
            std::time::Duration::from_millis(40)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn missing_plugin_probe_executable_fails_closed() {
        let result = probe_plugin_isolated(
            std::path::Path::new("definitely-missing-ministudio-probe"),
            "vst3",
            std::path::Path::new("missing.vst3"),
            true,
        );
        assert!(result.is_empty());
    }

    #[test]
    fn plugin_fingerprint_changes_with_bundle_contents() {
        let root =
            std::env::temp_dir().join(format!("ministudio-plugin-cache-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create fingerprint fixture");
        let binary = root.join("plugin.bin");
        fs::write(&binary, b"first").expect("write initial fixture");
        let first = plugin_fingerprint(&root);
        fs::write(&binary, b"a larger replacement").expect("change fixture");
        let second = plugin_fingerprint(&root);
        assert_ne!(first, second);
        fs::remove_dir_all(root).expect("remove fingerprint fixture");
    }
}

/// Runs the disposable native-plugin probe mode and returns true when normal Tauri startup must stop.
pub fn run_plugin_probe_from_args() -> bool {
    let mut args = std::env::args_os();
    let _ = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--minidaw-plugin-probe")) {
        return false;
    }
    let Some(format) = args.next().and_then(|v| v.into_string().ok()) else {
        return true;
    };
    let Some(path) = args.next() else { return true };
    let catalog_only = args.next().as_deref() == Some(std::ffi::OsStr::new("catalog"));
    let result = if catalog_only {
        audio::plugin::probe_plugin_catalog(&format, std::path::Path::new(&path))
    } else {
        audio::plugin::probe_plugin(&format, std::path::Path::new(&path))
    };
    if let Ok(descriptors) = result {
        if let Ok(json) = serde_json::to_string(&descriptors) {
            println!("{json}");
        }
    }
    true
}

#[tauri::command]
#[specta::specta]
async fn inspect_plugin_metadata(
    format: String,
    path: String,
    uid: String,
    app: AppHandle<AppRuntime>,
    registry: State<'_, PluginRegistryState>,
) -> Result<PluginDescriptor, EngineError> {
    let executable =
        std::env::current_exe().map_err(|error| EngineError::Internal(error.to_string()))?;
    let format_for_probe = format.clone();
    let path_for_probe = path.clone();
    let descriptors = tauri::async_runtime::spawn_blocking(move || {
        probe_plugin_isolated(
            &executable,
            &format_for_probe,
            Path::new(&path_for_probe),
            false,
        )
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    let descriptor = descriptors
        .into_iter()
        .find(|item| item.uid == uid)
        .ok_or_else(|| {
            EngineError::Internal(format!("plug-in metadata unavailable: {format}:{uid}"))
        })?;
    let cache_path = plugin_cache_path(&app)?;
    let mut cache = read_plugin_cache(&cache_path);
    if let Some(entry) = cache
        .entries
        .iter_mut()
        .find(|entry| entry.format == format && entry.path == path)
    {
        if let Some(existing) = entry.descriptors.iter_mut().find(|item| item.uid == uid) {
            *existing = descriptor.clone();
        } else {
            entry.descriptors.push(descriptor.clone());
        }
        write_plugin_cache(&cache_path, &cache)?;
        update_plugin_registry(&registry, &cache);
    }
    Ok(descriptor)
}

#[tauri::command]
#[specta::specta]
async fn render_offline_effect(
    _req: OfflineRenderRequest,
) -> Result<OfflineRenderResult, EngineError> {
    Err(EngineError::UnsupportedFormat(
        "VST3 rendering is planned for phase three".into(),
    ))
}

// Raw binary IPC is deliberately outside Specta: little-endian interleaved f32 (min, max).
#[tauri::command]
fn get_asset_peaks(
    asset_id: String,
    lod: u8,
    state: State<'_, NativeEngineState>,
) -> Result<Response, EngineError> {
    #[cfg(not(target_endian = "little"))]
    return Err(EngineError::UnsupportedFormat(
        "peak IPC requires a little-endian target".into(),
    ));

    #[cfg(target_endian = "little")]
    {
        let bytes = with_engine(&state, |engine| {
            engine
                .asset_peaks(&asset_id, lod)
                .map(|peaks| bytemuck::cast_slice::<f32, u8>(peaks).to_vec())
        })?;
        Ok(Response::new(bytes))
    }
}

#[cfg(feature = "desktop")]
fn binary_plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("binary")
        .invoke_handler(tauri::generate_handler![get_asset_peaks])
        .build()
}

fn specta_builder() -> Builder<AppRuntime> {
    Builder::new()
        .dangerously_cast_bigints_to_number()
        .commands(collect_commands![
            list_storage_roots,
            list_media_directory,
            search_media_directory,
            engine_init,
            engine_dispose,
            engine_load_audio_file,
            engine_unload_asset,
            engine_asset_peaks,
            engine_sync_graph,
            engine_play,
            engine_pause,
            engine_stop,
            engine_seek,
            engine_set_track_volume,
            engine_set_track_pan,
            engine_set_track_mute,
            engine_set_track_solo,
            engine_set_send_level,
            engine_set_bus_volume,
            engine_set_master_volume,
            engine_set_effect_param,
            engine_set_effect_bypass,
            engine_set_instrument_param,
            engine_midi_note,
            engine_midi_all_notes_off,
            engine_list_midi_inputs,
            engine_connect_midi_input,
            engine_disconnect_midi_input,
            engine_poll_state,
            engine_list_audio_backends,
            engine_list_output_devices,
            engine_get_audio_settings,
            engine_set_audio_settings,
            engine_export_project,
            engine_cancel_export,
            engine_eq_response,
            engine_open_plugin_editor,
            engine_close_plugin_editor,
            engine_plugin_editor_is_open,
            engine_save_plugin_state,
            engine_load_plugin_state,
            cached_vst3_plugins,
            scan_vst3_plugins,
            inspect_plugin_metadata,
            render_offline_effect,
        ])
}

fn bindings_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/engine/rust/bindings.ts")
}

fn export_bindings_file(builder: &Builder<AppRuntime>) -> Result<(), String> {
    let path = bindings_path();
    builder
        .export(Typescript::default(), &path)
        .map_err(|error| error.to_string())?;
    let generated = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let notice = "// 자동 생성 — 직접 수정 금지. `npm run bindings`로 재생성하세요.\n";
    if !generated.starts_with(notice) {
        std::fs::write(path, format!("{notice}{generated}")).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[cfg(feature = "desktop")]
pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    if let Err(error) = export_bindings_file(&builder) {
        eprintln!("failed to export typed IPC bindings: {error}");
    }
    let invoke_handler = builder.invoke_handler();
    let result = tauri::Builder::default()
        .manage(NativeEngineState::default())
        .manage(PluginRegistryState::default())
        .setup(|app| {
            if let Ok(path) = app.path().app_cache_dir() {
                let cache = read_plugin_cache(&path.join(PLUGIN_CACHE_FILE));
                update_plugin_registry(app.state::<PluginRegistryState>().inner(), &cache);
            }
            Ok(())
        })
        .plugin(binary_plugin())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(invoke_handler)
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("failed to run MiniDAW: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings() {
        if let Err(error) = export_bindings_file(&specta_builder()) {
            panic!("failed to export typed IPC bindings: {error}")
        }
    }
}
