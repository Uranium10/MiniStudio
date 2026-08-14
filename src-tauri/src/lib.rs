// Typed Tauri IPC surface for the native audio control plane and phase-three stubs.
mod audio;

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
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tauri::{
    ipc::{Channel, Response},
    State,
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
    snapshot: GraphSnapshot,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    with_engine(&state, |engine| engine.sync_graph(snapshot))
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
async fn scan_vst3_plugins(paths: Vec<String>) -> Result<Vec<PluginDescriptor>, EngineError> {
    tauri::async_runtime::spawn_blocking(move || scan_plugins_isolated(&paths))
        .await
        .map_err(|error| EngineError::Internal(error.to_string()))?
}

fn scan_plugins_isolated(paths: &[String]) -> Result<Vec<PluginDescriptor>, EngineError> {
    let executable =
        std::env::current_exe().map_err(|error| EngineError::Internal(error.to_string()))?;
    let binaries = Arc::new(audio::plugin::collect_plugin_binaries(paths));
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .min(4)
        .min(binaries.len().max(1));
    let cursor = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut plugins = Vec::new();
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let binaries = Arc::clone(&binaries);
            let cursor = Arc::clone(&cursor);
            let sender = sender.clone();
            let executable = executable.clone();
            scope.spawn(move || loop {
                let index = cursor.fetch_add(1, Ordering::Relaxed);
                let Some((format, path)) = binaries.get(index) else {
                    break;
                };
                let _ = sender.send(probe_plugin_isolated(&executable, format, path));
            });
        }
        drop(sender);
        for mut descriptors in receiver {
            plugins.append(&mut descriptors);
        }
    });
    plugins.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.uid.cmp(&b.uid))
    });
    plugins.dedup_by(|a, b| a.format == b.format && a.uid == b.uid && a.path == b.path);
    Ok(plugins)
}

fn probe_plugin_isolated(
    executable: &std::path::Path,
    format: &str,
    path: &std::path::Path,
) -> Vec<PluginDescriptor> {
    let mut child = match Command::new(executable)
        .arg("--minidaw-plugin-probe")
        .arg(format)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let completed = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
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
    if let Ok(descriptors) = audio::plugin::probe_plugin(&format, std::path::Path::new(&path)) {
        if let Ok(json) = serde_json::to_string(&descriptors) {
            println!("{json}");
        }
    }
    true
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
            engine_init,
            engine_dispose,
            engine_load_audio_file,
            engine_unload_asset,
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
            scan_vst3_plugins,
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
