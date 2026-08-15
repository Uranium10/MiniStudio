// Typed Tauri IPC surface for the native audio control plane and phase-three stubs.
use ministudio_audio::audio;

use audio::{
    engine::{EmbeddedPluginEditor, NativeEngine},
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
    cell::RefCell,
    collections::{HashMap, HashSet},
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
    webview::WebviewBuilder,
    window::WindowBuilder,
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewUrl, WindowEvent,
};
#[cfg(feature = "desktop")]
use tauri::{plugin::TauriPlugin, Runtime};
use tauri_specta::{collect_commands, Builder};

#[cfg(not(feature = "desktop"))]
type AppRuntime = tauri::test::MockRuntime;
#[cfg(feature = "desktop")]
type AppRuntime = tauri::Wry;

thread_local! {
    /// Native editor views are strictly owned by Tauri's UI thread. A TLS
    /// registry makes that ownership explicit and prevents accidental access
    /// from Tokio, plug-in control, or audio threads.
    static EMBEDDED_PLUGIN_EDITORS: RefCell<HashMap<String, EmbeddedPluginEditorSlot>> =
        RefCell::new(HashMap::new());
    static PINNED_PLUGIN_EDITORS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

struct EmbeddedPluginEditorSlot {
    window_label: String,
    editor: Box<dyn EmbeddedPluginEditor>,
}

fn close_embedded_plugin_editor(target_id: &str) {
    // Never run third-party GUI teardown while the TLS registry is borrowed.
    // VST3 `removed()` may synchronously call back into its host (including a
    // resize), and that re-entry must be able to inspect the registry safely.
    let editor = EMBEDDED_PLUGIN_EDITORS.with(|editors| editors.borrow_mut().remove(target_id));
    drop(editor);
}

fn resize_embedded_plugin_editor(target_id: &str, x: f32, y: f32, width: f32, height: f32) {
    EMBEDDED_PLUGIN_EDITORS.with(|editors| {
        if let Some(editor) = editors.borrow_mut().get_mut(target_id) {
            editor.editor.set_rect(x, y, width, height);
        }
    });
}

pub const WINDOWS_VST3_PATHS: &[&str] = &[r"C:\Program Files\Common Files\VST3"];
pub const MACOS_VST3_PATHS: &[&str] = &[
    "/Library/Audio/Plug-Ins/VST3",
    "~/Library/Audio/Plug-Ins/VST3",
];
pub const LINUX_VST3_PATHS: &[&str] = &["~/.vst3", "/usr/lib/vst3"];

struct NativeEngineState {
    engine: Arc<Mutex<NativeEngine>>,
    export_cancelled: Arc<AtomicBool>,
    /// Monotonic unpinned-open token. Every ordinary editor request supersedes
    /// the previous one; pinned editors are the only intentional exception.
    editor_open_generation: Arc<AtomicUsize>,
    /// Thread-safe mirror of the UI-thread pin registry, used only to exempt
    /// pinned resync requests from last-foreground-wins cancellation.
    pinned_editor_targets: Mutex<HashSet<String>>,
    /// WebView2 has a fragile destroy/focus edge on Windows. Editors that
    /// reject embedding keep their hidden shell alive and use the established
    /// standalone path for the rest of the process lifetime.
    floating_editor_targets: Mutex<HashSet<String>>,
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
            editor_open_generation: Arc::new(AtomicUsize::new(0)),
            pinned_editor_targets: Mutex::new(HashSet::new()),
            floating_editor_targets: Mutex::new(HashSet::new()),
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
        // `DirEntry::file_type` reports Windows junctions and directory symlinks
        // as links. Follow metadata here so sample-library subfolders remain
        // expandable instead of disappearing from the browser tree.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let is_directory = metadata.is_dir();
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
        if is_directory || (metadata.is_file() && is_audio) {
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
    let mut visited_paths = HashSet::new();
    let mut visited = 0_usize;
    while let Some(folder) = pending.pop() {
        if let Ok(canonical) = folder.canonicalize() {
            if !visited_paths.insert(canonical) {
                continue;
            }
        }
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
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if metadata.is_file()
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
async fn engine_dispose(
    app: AppHandle<AppRuntime>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    close_embedded_editors_on_main_thread(&app, None).await?;
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
async fn engine_sync_graph(
    mut snapshot: GraphSnapshot,
    app: AppHandle<AppRuntime>,
    state: State<'_, NativeEngineState>,
    plugins: State<'_, PluginRegistryState>,
) -> Result<(), EngineError> {
    // Invalidate editor attachments aimed at the graph that is about to be
    // retired. Shell resync requests receive a fresh generation afterward.
    state.editor_open_generation.fetch_add(1, Ordering::AcqRel);
    // A graph replacement can destroy the plug-in instance backing a native
    // editor. Finish detaching every UI-thread-owned view before touching the
    // graph, otherwise deleting a track leaves vendor GUI code with a stale
    // instance pointer and can terminate the process.
    close_embedded_editors_on_main_thread(&app, None).await?;
    resolve_plugin_paths(&mut snapshot, &plugins);
    state
        .floating_editor_targets
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
    with_engine(&state, |engine| {
        let result = engine.sync_graph(snapshot);
        // Requests that started while waiting for the engine lock must not
        // attach a view to either the retired or half-rebuilt graph. Publish a
        // second fence while still holding that lock; the frontend will issue
        // restoration only after this command has returned.
        state.editor_open_generation.fetch_add(1, Ordering::AcqRel);
        result
    })
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
async fn engine_set_audio_settings(
    settings: AudioSettings,
    app: AppHandle<AppRuntime>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    close_embedded_editors_on_main_thread(&app, None).await?;
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

const PLUGIN_SHELL_TOOLBAR_HEIGHT: f32 = 52.0;
const PLUGIN_SHELL_CLOSED_EVENT: &str = "ministudio:plugin-shell-closed";
const PLUGIN_SHELL_REACTIVATE_EVENT: &str = "ministudio:plugin-shell-reactivate";
const PLUGIN_SHELL_CONNECTION_EVENT: &str = "ministudio:plugin-shell-connection";
const PLUGIN_SHELL_REQUEST_EVENT: &str = "ministudio:plugin-shell-request";

/// Embedded commercial plug-in HWNDs are still an experimental host path.
/// The production-safe default uses each format's established, message-pumped
/// native window and never destroys an unrelated editor while opening another.
/// Developers can opt back into the shell while working on process-isolated
/// editor hosting without making vendor-specific allow/deny lists.
fn embedded_plugin_shells_enabled() -> bool {
    std::env::var("MINISTUDIO_EXPERIMENTAL_EMBEDDED_EDITORS")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "on" | "ON"))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginShellConnection {
    status: &'static str,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginShellTargetPayload {
    target_kind: String,
    target_id: String,
    window_label: String,
}

/// WebView2 controller creation is substantially slower than creating the
/// native owner window or attaching most VST3 views. Build the toolbar after
/// one paint frame so it never gates time-to-first-plug-in-pixel.
fn spawn_plugin_shell_toolbar(window: tauri::Window<AppRuntime>, label: String, url: String) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(16));
        if window
            .webviews()
            .iter()
            .any(|webview| webview.label() == label)
        {
            return;
        }
        let size = window.inner_size().ok();
        let scale = window.scale_factor().unwrap_or(1.0).max(0.5);
        let width = size.map(|size| size.width as f64 / scale).unwrap_or(920.0);
        if let Err(error) = window.add_child(
            WebviewBuilder::new(&label, WebviewUrl::App(url.into())),
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(width, PLUGIN_SHELL_TOOLBAR_HEIGHT as f64),
        ) {
            eprintln!("failed to create deferred plug-in toolbar {label}: {error}");
        }
    });
}

fn plugin_shell_label(target_kind: &str, target_id: &str) -> String {
    let safe = target_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("plugin-{target_kind}-{safe}")
}

/// Drops embedded editor handles on the UI thread that owns their HWNDs.
/// Passing `None` closes every editor before a graph-wide mutation.
async fn close_embedded_editors_on_main_thread(
    app: &AppHandle<AppRuntime>,
    target_id: Option<String>,
) -> Result<(), EngineError> {
    let (reply, receive) = std::sync::mpsc::sync_channel(1);
    let ui_app = app.clone();
    app.run_on_main_thread(move || {
        let editors = if let Some(target_id) = target_id {
            EMBEDDED_PLUGIN_EDITORS.with(|editors| {
                editors
                    .borrow_mut()
                    .remove(&target_id)
                    .map(|slot| vec![slot])
                    .unwrap_or_default()
            })
        } else {
            EMBEDDED_PLUGIN_EDITORS.with(|editors| {
                editors
                    .borrow_mut()
                    .drain()
                    .map(|(_, slot)| slot)
                    .collect::<Vec<_>>()
            })
        };
        // A graph rebuild temporarily detaches every native view. Hide the
        // owner windows as part of the same UI-thread transaction; surviving
        // targets are reopened by the shell bridge after graph sync, while a
        // deleted track can never leave a stale blank host window behind.
        for slot in editors {
            if let Some(window) = ui_app.get_window(&slot.window_label) {
                let _ = window.hide();
            }
            // Hiding the owner first prevents vendor paint/focus callbacks
            // from racing the detach. The registry borrow ended above, so a
            // synchronous VST3 host callback cannot trigger a RefCell panic.
            drop(slot);
        }
        let _ = reply.send(());
    })
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| EngineError::Internal("native editor UI-thread detach timed out".into()))
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?
}

/// Keeps at most one ordinary hosted editor visible. Pinned editors are
/// excluded, and the target currently being opened is never disturbed.
async fn close_other_unpinned_editors_on_main_thread(
    app: &AppHandle<AppRuntime>,
    current_target_id: String,
    cancellation: Option<(Arc<AtomicUsize>, usize)>,
) -> Result<bool, EngineError> {
    let (reply, receive) = std::sync::mpsc::sync_channel(1);
    let ui_app = app.clone();
    app.run_on_main_thread(move || {
        if editor_open_is_superseded(&cancellation) {
            let _ = reply.send(false);
            return;
        }
        let pinned = PINNED_PLUGIN_EDITORS.with(|targets| targets.borrow().clone());
        let closing = EMBEDDED_PLUGIN_EDITORS.with(|editors| {
            let mut editors = editors.borrow_mut();
            let target_ids = editors
                .keys()
                .filter(|target_id| {
                    target_id.as_str() != current_target_id.as_str() && !pinned.contains(*target_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            target_ids
                .into_iter()
                .filter_map(|target_id| editors.remove(&target_id).map(|slot| (target_id, slot)))
                .collect::<Vec<_>>()
        });
        for (_, slot) in closing {
            let label = slot.window_label.clone();
            if let Some(window) = ui_app.get_window(&label) {
                let _ = window.hide();
            }
            // Drop only after the registry borrow and window-hide operation
            // have both completed; vendor `removed()` is allowed to re-enter.
            drop(slot);
            let _ = ui_app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, label);
        }
        let _ = reply.send(true);
    })
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| EngineError::Internal("native editor switch timed out".into()))
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?
}

#[tauri::command]
#[specta::specta]
async fn engine_set_plugin_editor_pinned(
    target_id: String,
    pinned: bool,
    app: AppHandle<AppRuntime>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    {
        let mut targets = state
            .pinned_editor_targets
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pinned {
            targets.insert(target_id.clone());
        } else {
            targets.remove(&target_id);
        }
    }
    let (reply, receive) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        PINNED_PLUGIN_EDITORS.with(|targets| {
            let mut targets = targets.borrow_mut();
            if pinned {
                targets.insert(target_id);
            } else {
                targets.remove(&target_id);
            }
        });
        let _ = reply.send(());
    })
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| EngineError::Internal("native editor pin update timed out".into()))
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?
}

fn editor_open_is_superseded(cancellation: &Option<(Arc<AtomicUsize>, usize)>) -> bool {
    cancellation
        .as_ref()
        .is_some_and(|(generation, expected)| generation.load(Ordering::Acquire) != *expected)
}

/// Opens a VST3 view on its message-pumped GUI worker without blocking Tauri's
/// event loop, then installs only the Send proxy in the UI-thread registry.
/// Registry mutation remains on Tauri's thread; third-party GUI construction
/// and destruction remain on the dedicated thread that owns their HWNDs.
async fn attach_embedded_editor(
    app: &AppHandle<AppRuntime>,
    engine: Arc<Mutex<NativeEngine>>,
    target_id: String,
    window_label: String,
    parent: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    cancellation: Option<(Arc<AtomicUsize>, usize)>,
) -> Result<Result<bool, String>, EngineError> {
    // Remove the previous proxy first. Its CloseEmbedded message is queued
    // before the new OpenEmbedded request on the same plug-in GUI worker.
    let (detach_reply, detach_receive) = std::sync::mpsc::sync_channel(1);
    let detach_target = target_id.clone();
    let detach_cancellation = cancellation.clone();
    app.run_on_main_thread(move || {
        if editor_open_is_superseded(&detach_cancellation) {
            let _ = detach_reply.send(false);
            return;
        }
        close_embedded_plugin_editor(&detach_target);
        let _ = detach_reply.send(true);
    })
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    let detached = tauri::async_runtime::spawn_blocking(move || {
        detach_receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| EngineError::Internal("native editor proxy detach timed out".into()))
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))??;
    if !detached || editor_open_is_superseded(&cancellation) {
        return Ok(Ok(false));
    }

    let open_cancellation = cancellation.clone();
    let open_target = target_id.clone();
    let opened = tauri::async_runtime::spawn_blocking(move || {
        if editor_open_is_superseded(&open_cancellation) {
            return Ok(None);
        }
        // Hold the global engine mutex only long enough to clone this target's
        // control Arc. Commercial GUI startup never runs under that lock.
        let control = engine
            .lock()
            .map_err(|_| "native engine control lock is poisoned".to_owned())
            .and_then(|engine| engine.plugin_control(&open_target))?;
        if editor_open_is_superseded(&open_cancellation) {
            return Ok(None);
        }
        control
            .open_editor_embedded(parent, x, y, width, height)
            .map(Some)
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    let editor = match opened {
        Ok(Some(editor)) => editor,
        Ok(None) => return Ok(Ok(false)),
        Err(error) => return Ok(Err(error)),
    };

    let (reply, receive) = std::sync::mpsc::sync_channel(1);
    let install_cancellation = cancellation.clone();
    app.run_on_main_thread(move || {
        if editor_open_is_superseded(&install_cancellation) {
            drop(editor);
            let _ = reply.send(false);
            return;
        }
        EMBEDDED_PLUGIN_EDITORS.with(|editors| {
            editors.borrow_mut().insert(
                target_id,
                EmbeddedPluginEditorSlot {
                    window_label,
                    editor,
                },
            );
        });
        let _ = reply.send(true);
    })
    .map_err(|error| EngineError::Internal(error.to_string()))?;
    let installed = tauri::async_runtime::spawn_blocking(move || {
        receive
            .recv_timeout(std::time::Duration::from_secs(15))
            .map_err(|_| EngineError::Internal("native editor proxy install timed out".into()))
    })
    .await
    .map_err(|error| EngineError::Internal(error.to_string()))??;
    Ok(Ok(installed))
}

#[tauri::command]
#[specta::specta]
async fn engine_open_plugin_editor(
    target_kind: String,
    target_id: String,
    foreground: bool,
    app: AppHandle<AppRuntime>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    validate_plugin_target_kind(&target_kind).map_err(EngineError::from)?;
    if !embedded_plugin_shells_enabled() {
        // Clone the per-plug-in control under the engine mutex, then release
        // the global lock before entering vendor GUI code. Standalone VST3 and
        // CLAP windows own a dedicated message-pumped thread, matching the
        // stable pre-shell behavior and allowing several editors to coexist.
        let control = state
            .engine
            .lock()
            .map_err(|_| EngineError::Internal("native engine control lock is poisoned".into()))?
            .plugin_control(&target_id)
            .map_err(EngineError::from)?;
        return tauri::async_runtime::spawn_blocking(move || {
            control.open_editor().map_err(EngineError::from)
        })
        .await
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    }
    let pinned = state
        .pinned_editor_targets
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains(&target_id);
    let request_generation = if pinned {
        state.editor_open_generation.load(Ordering::Acquire)
    } else {
        state
            .editor_open_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    };
    // Retained in IPC as an explicit distinction for diagnostics and future
    // scheduling policy; request order already makes foreground opens win.
    let _intent_is_foreground = foreground;
    let cancellation = (!pinned).then(|| {
        (
            Arc::clone(&state.editor_open_generation),
            request_generation,
        )
    });
    if !close_other_unpinned_editors_on_main_thread(&app, target_id.clone(), cancellation.clone())
        .await?
        || editor_open_is_superseded(&cancellation)
    {
        return Ok(());
    }
    // Standalone fallback editors (for example CLAP editors that cannot be
    // embedded safely) have no MiniStudio toolbar, and therefore cannot be
    // pinned. Apply the same single-active-editor policy to them explicitly.
    let floating_to_close = {
        let mut targets = state
            .floating_editor_targets
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let closing = targets
            .iter()
            .filter(|candidate| candidate.as_str() != target_id.as_str())
            .cloned()
            .collect::<Vec<_>>();
        for candidate in &closing {
            targets.remove(candidate);
        }
        closing
    };
    for candidate in floating_to_close {
        let _ = with_engine(&state, |engine| engine.close_plugin_editor(&candidate));
        for kind in ["instrument", "effect"] {
            let old_label = plugin_shell_label(kind, &candidate);
            if let Some(window) = app.get_window(&old_label) {
                let _ = window.hide();
            }
            let _ = app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, old_label);
        }
    }
    let label = plugin_shell_label(&target_kind, &target_id);
    if let Some(window) = app.get_window(&label) {
        let is_open = with_engine(
            &state,
            |engine| Ok(engine.plugin_editor_is_open(&target_id)),
        )?;
        let floating = state
            .floating_editor_targets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&target_id);
        if floating {
            if !is_open {
                with_engine(&state, |engine| engine.open_plugin_editor(&target_id))?;
            }
            return Ok(());
        }
        if !is_open {
            // Show WebView2 before parenting the native editor. Showing the
            // shell after embedding raises wry's browser HWND above the VST3
            // sibling and leaves a connected editor hidden behind the page.
            let _ = window.unminimize();
            window
                .show()
                .map_err(|error| EngineError::Internal(error.to_string()))?;
            let size = window
                .inner_size()
                .map_err(|error| EngineError::Internal(error.to_string()))?;
            let scale = window.scale_factor().unwrap_or(1.0).max(0.5) as f32;
            #[cfg(target_os = "windows")]
            let parent = window
                .hwnd()
                .map_err(|error| EngineError::Internal(error.to_string()))?
                .0 as usize;
            #[cfg(not(target_os = "windows"))]
            let parent = 0usize;
            let _ = app.emit_to(
                &label,
                PLUGIN_SHELL_CONNECTION_EVENT,
                PluginShellConnection {
                    status: "connecting",
                    error: None,
                },
            );
            let result = attach_embedded_editor(
                &app,
                Arc::clone(&state.engine),
                target_id.clone(),
                label.clone(),
                parent,
                0.0,
                PLUGIN_SHELL_TOOLBAR_HEIGHT,
                size.width as f32 / scale,
                (size.height as f32 / scale - PLUGIN_SHELL_TOOLBAR_HEIGHT).max(1.0),
                cancellation.clone(),
            )
            .await;
            if editor_open_is_superseded(&cancellation) {
                close_embedded_editors_on_main_thread(&app, Some(target_id.clone())).await?;
                let _ = window.hide();
                return Ok(());
            }
            match result {
                Ok(Ok(true)) => {
                    let _ = app.emit_to(
                        &label,
                        PLUGIN_SHELL_CONNECTION_EVENT,
                        PluginShellConnection {
                            status: "ready",
                            error: None,
                        },
                    );
                }
                Ok(Ok(false)) => {
                    let _ = window.hide();
                    return Ok(());
                }
                Ok(Err(embed_error)) => {
                    let _ = window.hide();
                    state
                        .floating_editor_targets
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(target_id.clone());
                    let _ = app.emit_to(
                        &label,
                        PLUGIN_SHELL_CONNECTION_EVENT,
                        PluginShellConnection {
                            status: "failed",
                            error: Some(embed_error.clone()),
                        },
                    );
                    let _ = app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, &label);
                    return with_engine(&state, |engine| engine.open_plugin_editor(&target_id))
                        .map_err(|fallback| {
                            EngineError::Internal(format!(
                                "{embed_error}; fallback failed: {fallback}"
                            ))
                        });
                }
                Err(error) => {
                    let _ = window.hide();
                    let _ = app.emit_to(
                        &label,
                        PLUGIN_SHELL_CONNECTION_EVENT,
                        PluginShellConnection {
                            status: "failed",
                            error: Some(error.to_string()),
                        },
                    );
                    let _ = app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, &label);
                    return Err(error);
                }
            }
        }
        let _ = window.unminimize();
        window
            .show()
            .map_err(|error| EngineError::Internal(error.to_string()))?;
        let size = window
            .inner_size()
            .map_err(|error| EngineError::Internal(error.to_string()))?;
        let scale = window.scale_factor().unwrap_or(1.0).max(0.5) as f32;
        // `show` may raise WebView2's browser child. Re-applying the native
        // rectangle also restores the editor container's sibling z-order.
        resize_embedded_plugin_editor(
            &target_id,
            0.0,
            PLUGIN_SHELL_TOOLBAR_HEIGHT,
            size.width as f32 / scale,
            (size.height as f32 / scale - PLUGIN_SHELL_TOOLBAR_HEIGHT).max(1.0),
        );
        let _ = app.emit_to(&label, PLUGIN_SHELL_REACTIVATE_EVENT, ());
        return Ok(());
    }

    let url = format!(
        "index.html?pluginShell=1&targetKind={target_kind}&targetId={target_id}&windowLabel={label}"
    );
    let mut builder = WindowBuilder::new(&app, &label)
        .title("MiniStudio Plug-in")
        .inner_size(920.0, 680.0)
        .min_inner_size(420.0, 260.0)
        .resizable(true)
        .focused(false)
        .visible(false);
    if let Some(main) = app.get_window("main") {
        builder = builder
            .parent(&main)
            .map_err(|error| EngineError::Internal(error.to_string()))?;
    }
    let window = builder
        .build()
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    // Register the target before the deferred toolbar is alive. Graph rebuilds
    // can therefore preserve/reopen this editor without waiting for WebView2.
    let _ = app.emit_to(
        "main",
        PLUGIN_SHELL_REQUEST_EVENT,
        PluginShellTargetPayload {
            target_kind: target_kind.clone(),
            target_id: target_id.clone(),
            window_label: label.clone(),
        },
    );
    // Never destroy a secondary WebView2 toolbar during an editor transition.
    // Hiding retains its initialized controller and is cheaper on reopen.
    let lifecycle_engine = Arc::clone(&state.engine);
    let lifecycle_id = target_id.clone();
    let event_window = window.clone();
    let lifecycle_app = app.clone();
    let lifecycle_label = label.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            let scale = event_window.scale_factor().unwrap_or(1.0).max(0.5) as f32;
            if let Some(toolbar) = lifecycle_app.get_webview(&lifecycle_label) {
                let _ = toolbar.set_size(LogicalSize::new(
                    size.width as f64 / scale as f64,
                    PLUGIN_SHELL_TOOLBAR_HEIGHT as f64,
                ));
            }
            resize_embedded_plugin_editor(
                &lifecycle_id,
                0.0,
                PLUGIN_SHELL_TOOLBAR_HEIGHT,
                size.width as f32 / scale,
                (size.height as f32 / scale - PLUGIN_SHELL_TOOLBAR_HEIGHT).max(1.0),
            );
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = event_window.hide();
            let _ = lifecycle_app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, &lifecycle_label);
            close_embedded_plugin_editor(&lifecycle_id);
            let engine = Arc::clone(&lifecycle_engine);
            let target_id = lifecycle_id.clone();
            let _ = std::thread::Builder::new()
                .name("ministudio-plugin-editor-close".into())
                .spawn(move || {
                    if let Ok(engine) = engine.lock() {
                        let _ = engine.close_plugin_editor(&target_id);
                    }
                });
        }
        WindowEvent::Destroyed => {
            close_embedded_plugin_editor(&lifecycle_id);
        }
        _ => {}
    });
    // Show the lightweight native owner immediately. The slower toolbar
    // WebView2 controller is deliberately created only after attachment.
    window
        .show()
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    let size = window
        .inner_size()
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    let scale = window.scale_factor().unwrap_or(1.0).max(0.5) as f32;
    #[cfg(target_os = "windows")]
    let parent = window
        .hwnd()
        .map_err(|error| EngineError::Internal(error.to_string()))?
        .0 as usize;
    #[cfg(not(target_os = "windows"))]
    let parent = 0usize;
    let _ = app.emit_to(
        &label,
        PLUGIN_SHELL_CONNECTION_EVENT,
        PluginShellConnection {
            status: "connecting",
            error: None,
        },
    );
    let open_result = match attach_embedded_editor(
        &app,
        Arc::clone(&state.engine),
        target_id.clone(),
        label.clone(),
        parent,
        0.0,
        PLUGIN_SHELL_TOOLBAR_HEIGHT,
        size.width as f32 / scale,
        (size.height as f32 / scale - PLUGIN_SHELL_TOOLBAR_HEIGHT).max(1.0),
        cancellation.clone(),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let _ = window.hide();
            let _ = app.emit_to(
                &label,
                PLUGIN_SHELL_CONNECTION_EVENT,
                PluginShellConnection {
                    status: "failed",
                    error: Some(error.to_string()),
                },
            );
            let _ = app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, &label);
            return Err(error);
        }
    };

    if editor_open_is_superseded(&cancellation) {
        close_embedded_editors_on_main_thread(&app, Some(target_id.clone())).await?;
        let _ = window.hide();
        return Ok(());
    }
    let attached = match open_result {
        Ok(attached) => attached,
        Err(embed_error) => {
            let _ = window.hide();
            state
                .floating_editor_targets
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(target_id.clone());
            let _ = app.emit_to(
                &label,
                PLUGIN_SHELL_CONNECTION_EVENT,
                PluginShellConnection {
                    status: "failed",
                    error: Some(embed_error.clone()),
                },
            );
            let _ = app.emit_to("main", PLUGIN_SHELL_CLOSED_EVENT, &label);
            // Fixed-size or legacy plug-ins may reject parenting. Keep the proven
            // standalone editor path as a compatibility fallback.
            return with_engine(&state, |engine| engine.open_plugin_editor(&target_id)).map_err(
                |fallback| {
                    EngineError::Internal(format!("{embed_error}; fallback failed: {fallback}"))
                },
            );
        }
    };
    if !attached {
        let _ = window.hide();
        return Ok(());
    }
    state
        .floating_editor_targets
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&target_id);
    window
        .show()
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    let size = window
        .inner_size()
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    let scale = window.scale_factor().unwrap_or(1.0).max(0.5) as f32;
    resize_embedded_plugin_editor(
        &target_id,
        0.0,
        PLUGIN_SHELL_TOOLBAR_HEIGHT,
        size.width as f32 / scale,
        (size.height as f32 / scale - PLUGIN_SHELL_TOOLBAR_HEIGHT).max(1.0),
    );
    let _ = app.emit_to(
        &label,
        PLUGIN_SHELL_CONNECTION_EVENT,
        PluginShellConnection {
            status: "ready",
            error: None,
        },
    );
    let _ = app.emit_to(&label, PLUGIN_SHELL_REACTIVATE_EVENT, ());
    spawn_plugin_shell_toolbar(window.clone(), label, url);
    Ok(())
}

#[tauri::command]
#[specta::specta]
async fn engine_close_plugin_editor(
    target_kind: String,
    target_id: String,
    app: AppHandle<AppRuntime>,
    state: State<'_, NativeEngineState>,
) -> Result<(), EngineError> {
    close_embedded_editors_on_main_thread(&app, Some(target_id.clone())).await?;
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
        .arg("--ministudio-plugin-probe")
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

/// Runs the private VST3 host-process mode before Tauri/WebView startup.
pub fn run_plugin_host_from_args() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new("--ministudio-vst3-host"))
    {
        return false;
    }
    audio::plugin::run_vst3_helper_process();
    true
}

/// Runs the disposable native-plugin probe mode and returns true when normal Tauri startup must stop.
pub fn run_plugin_probe_from_args() -> bool {
    let mut args = std::env::args_os();
    let _ = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--ministudio-plugin-probe")) {
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
            engine_set_plugin_editor_pinned,
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
        eprintln!("failed to run MiniStudio: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReentrantEditor;

    impl EmbeddedPluginEditor for ReentrantEditor {
        fn set_rect(&mut self, _x: f32, _y: f32, _width: f32, _height: f32) {}
    }

    impl Drop for ReentrantEditor {
        fn drop(&mut self) {
            // Models a vendor VST3 view calling back into the host from
            // IPlugView::removed(). This must not overlap a mutable registry
            // borrow or the real UI thread will panic during editor switching.
            EMBEDDED_PLUGIN_EDITORS.with(|editors| {
                let _ = editors.borrow().len();
            });
        }
    }

    #[test]
    fn export_bindings() {
        if let Err(error) = export_bindings_file(&specta_builder()) {
            panic!("failed to export typed IPC bindings: {error}")
        }
    }

    #[test]
    fn embedded_editor_teardown_allows_vendor_reentry() {
        let target_id = "reentrant-editor-regression".to_owned();
        EMBEDDED_PLUGIN_EDITORS.with(|editors| {
            editors.borrow_mut().insert(
                target_id.clone(),
                EmbeddedPluginEditorSlot {
                    window_label: "reentrant-editor-window".to_owned(),
                    editor: Box::new(ReentrantEditor),
                },
            );
        });

        close_embedded_plugin_editor(&target_id);

        EMBEDDED_PLUGIN_EDITORS.with(|editors| {
            assert!(!editors.borrow().contains_key(&target_id));
        });
    }

    #[test]
    fn stale_background_editor_request_yields_to_foreground_generation() {
        let generation = Arc::new(AtomicUsize::new(4));
        let request = Some((Arc::clone(&generation), 4));
        assert!(!editor_open_is_superseded(&request));

        generation.fetch_add(1, Ordering::AcqRel);
        assert!(editor_open_is_superseded(&request));
        assert!(!editor_open_is_superseded(&None));
    }
}
