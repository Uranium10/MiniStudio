// Serializable IPC/domain structures with sample time only inside the native boundary.
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub backend_id: String,
    pub device_id: String,
    pub sample_rate: u32,
    pub buffer_size: u32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            backend_id: "default".into(),
            device_id: "default".into(),
            sample_rate: 48_000,
            buffer_size: 256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioBackendInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub asio: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub sample_rates: Vec<u32>,
    pub buffer_sizes: Vec<u32>,
    pub channels: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputPortInfo {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    pub peak: f32,
    pub rms: f32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StereoLevel {
    pub left: f32,
    pub right: f32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MultibandLevels {
    pub low: StereoLevel,
    pub mid: StereoLevel,
    pub high: StereoLevel,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamStatus {
    pub latency_ms: f64,
    pub xruns: u64,
    pub running: bool,
    pub error: Option<String>,
    pub pdc_samples: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub playhead_sec: f64,
    pub track_levels: Vec<Level>,
    pub master_level: Level,
    pub stream: StreamStatus,
    pub graph_revision: u64,
    pub playing: bool,
    pub active_voice_counts: Vec<u32>,
    pub multiband_levels: Vec<MultibandLevels>,
    pub distortion_spectra: Vec<Vec<f32>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    pub tracks: Vec<TrackSpec>,
    pub buses: Vec<BusSpec>,
    pub master: MasterSpec,
    pub transport: TransportSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TransportSpec {
    pub bpm: f64,
    pub playhead_sec: f64,
    pub is_playing: bool,
    #[serde(rename = "loop")]
    pub loop_: LoopSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoopSpec {
    pub enabled: bool,
    pub start_sec: f64,
    pub end_sec: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrackSpec {
    pub id: String,
    #[serde(default = "default_track_kind")]
    pub kind: String,
    pub name: String,
    pub clips: Vec<ClipSpec>,
    #[serde(default)]
    pub midi_clips: Vec<MidiClipSpec>,
    #[serde(default)]
    pub instrument: Option<InstrumentSpec>,
    pub volume_db: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub effects: Vec<EffectSpec>,
    pub sends: Vec<SendSpec>,
    #[serde(default)]
    pub output_bus_id: Option<String>,
}
fn default_track_kind() -> String {
    "audio".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MidiClipSpec {
    pub id: String,
    pub name: String,
    pub start_sec: f64,
    pub duration_sec: f64,
    pub loop_enabled: bool,
    pub loop_start_ticks: u64,
    pub loop_length_ticks: u64,
    pub notes: Vec<MidiNoteSpec>,
    pub transpose_semitones: i16,
    pub velocity_scale: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MidiNoteSpec {
    pub id: String,
    pub pitch: u8,
    pub velocity: u8,
    pub start_ticks: u64,
    pub length_ticks: u64,
    pub release_velocity: u8,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InstrumentSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub params: HashMap<String, f32>,
    pub bypassed: bool,
    #[serde(default)]
    pub plugin: Option<ExternalPluginRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipSpec {
    pub id: String,
    pub asset_id: String,
    pub start_sec: f64,
    pub offset_sec: f64,
    pub duration_sec: f64,
    pub gain_db: f32,
    pub fade_in_sec: f64,
    pub fade_out_sec: f64,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SendSpec {
    pub id: String,
    pub target_bus_id: String,
    pub gain_db: f32,
    pub pre_fader: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BusSpec {
    pub id: String,
    pub name: String,
    pub effects: Vec<EffectSpec>,
    pub volume_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MasterSpec {
    pub volume_db: f32,
    pub effects: Vec<EffectSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EffectSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub bypassed: bool,
    pub params: HashMap<String, f32>,
    #[serde(default)]
    pub plugin: Option<ExternalPluginRef>,
    #[serde(default)]
    pub sidechain: Option<SidechainSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPluginRef {
    pub format: String,
    pub uid: String,
    pub name: String,
    pub vendor: String,
    pub path: String,
    #[serde(default)]
    pub audio_input_buses: u32,
    #[serde(default)]
    pub audio_output_buses: u32,
    #[serde(default)]
    pub supports_sidechain: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SidechainSpec {
    pub enabled: bool,
    pub source_track_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NativeAssetInfo {
    pub id: String,
    pub path: String,
    pub name: String,
    pub duration_sec: f64,
    pub sample_rate: u32,
    pub num_channels: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EqFrequencyResponse {
    pub frequencies: Vec<f32>,
    pub combined_db: Vec<f32>,
    pub bands_db: Vec<Vec<f32>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DecodeProgress {
    pub stage: String,
    pub fraction: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub output_path: String,
    pub bit_depth: u16,
    pub sample_rate: u32,
    pub normalize: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub stage: String,
    pub rendered_frames: u64,
    pub total_frames: u64,
    pub fraction: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub output_path: String,
    pub peak_db: f32,
    pub clipped: bool,
    pub cancelled: bool,
}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

pub fn sec_to_samples(sec: f64, sample_rate: u32) -> u64 {
    (sec.max(0.0) * f64::from(sample_rate)).round() as u64
}
