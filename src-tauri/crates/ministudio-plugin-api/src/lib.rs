//! Format-neutral plug-in runtime contracts.
//!
//! This crate deliberately has no dependency on Tauri, a window toolkit, VST3, CLAP, or the
//! MiniStudio project model. It is the dependency boundary shared by the audio graph and the
//! control-plane supervisor.

use serde::{Deserialize, Serialize};

/// Stable project-facing identity for one instantiated plug-in.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginInstanceId(pub String);

/// Generation of a runtime instance or editor session.
///
/// Replies from an older generation must never mutate the current session.
pub type Generation = u64;

/// Supported external plug-in format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    /// Steinberg VST3.
    Vst3,
    /// CLever Audio Plug-in.
    Clap,
}

/// Capabilities discovered for an instantiated plug-in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeCapabilities {
    /// A native editor can be opened.
    pub editor: bool,
    /// Additional input buses can receive sidechain audio.
    pub sidechain: bool,
    /// State can be serialized and restored.
    pub state: bool,
    /// The format exposes parameter automation.
    pub automation: bool,
}

/// Native editor lifecycle owned by the supervisor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditorLifecycle {
    /// No editor exists.
    #[default]
    Closed,
    /// An asynchronous open request is outstanding.
    Opening,
    /// The native editor is visible.
    Open,
    /// An asynchronous close request is outstanding.
    Closing,
    /// The editor failed while audio may still be healthy.
    Unresponsive,
}

/// Runtime health. GUI and realtime health are intentionally independent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHealth {
    /// Realtime endpoint is responding within its deadline.
    pub realtime_healthy: bool,
    /// Control process is alive.
    pub control_healthy: bool,
    /// Native GUI responds to supervisor intents.
    pub editor_healthy: bool,
}

/// Authoritative state for one native editor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSession {
    /// Plug-in owning the editor.
    pub instance_id: PluginInstanceId,
    /// Rejects stale asynchronous replies.
    pub generation: Generation,
    /// Current editor lifecycle.
    pub lifecycle: EditorLifecycle,
    /// Whether the platform window must stay above ordinary windows.
    pub pinned: bool,
    /// Whether this is the last foreground editor request.
    pub foreground: bool,
    /// Independent realtime/control/editor health.
    pub health: RuntimeHealth,
}

/// Placement chosen by the isolation supervisor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum IsolationPlacement {
    /// One helper process for this instance.
    Dedicated,
    /// A bounded helper shared by instances from the same binary fingerprint.
    ModuleGroup {
        /// Stable binary fingerprint, never an absolute path.
        fingerprint: String,
        /// Maximum number of instances admitted to the helper.
        capacity: u8,
    },
}

/// Non-realtime intent submitted to the supervisor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PluginControlIntent {
    /// Open and optionally foreground a native editor.
    OpenEditor { foreground: bool },
    /// Close a native editor.
    CloseEditor,
    /// Apply authoritative pin state.
    SetPinned { pinned: bool },
    /// Save an opaque plug-in state snapshot.
    SaveState,
    /// Restore an opaque plug-in state snapshot.
    LoadState { bytes: Vec<u8> },
    /// Shut down the instance after graph retirement.
    Shutdown,
}

/// Event emitted by a helper or supervisor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PluginEvent {
    /// Editor lifecycle changed.
    EditorLifecycle { lifecycle: EditorLifecycle },
    /// Platform acknowledged a topmost transition.
    PinApplied { pinned: bool },
    /// Helper chrome requested a host action.
    EditorAction { action: String },
    /// GUI is unresponsive without declaring the audio endpoint dead.
    EditorUnresponsive,
    /// Realtime endpoint missed a processing deadline.
    RealtimeDeadlineMiss,
    /// Helper exited or crashed.
    HelperExited { detail: String },
}

/// Timestamped short MIDI packet used by every live-input source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiPacket {
    /// Host sample position at which the event should be applied.
    pub sample_time: u64,
    /// Stable logical input source.
    pub source_id: u32,
    /// Target track index resolved by the MIDI service.
    pub target_track: u16,
    /// MIDI 1.0 status/data bytes.
    pub data: [u8; 3],
}

/// Lock-free counters published from the realtime runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetricsSnapshot {
    /// Audio callback count.
    pub callbacks: u64,
    /// Callback durations in nanoseconds.
    pub callback_p50_ns: u64,
    /// Callback durations in nanoseconds.
    pub callback_p95_ns: u64,
    /// Callback durations in nanoseconds.
    pub callback_p99_ns: u64,
    /// Longest callback duration in nanoseconds.
    pub callback_max_ns: u64,
    /// Audio backend underrun/overrun count.
    pub xruns: u64,
    /// Highest observed command queue occupancy.
    pub command_high_water: u32,
    /// Commands dropped or deferred because a bounded queue was full.
    pub command_overflow: u64,
    /// Realtime plug-in deadline misses.
    pub plugin_deadline_misses: u64,
}
