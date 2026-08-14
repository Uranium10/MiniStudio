// Serializable domain types shared by the engine contract, store, and UI.
export type EffectType =
  | 'builtin:eq'
  | 'builtin:compressor'
  | 'builtin:multiband-compressor'
  | 'builtin:utility'
  | 'builtin:eq8'
  | 'builtin:delay'
  | 'builtin:reverb'
  | 'builtin:waveshaper'
  | 'builtin:distortion'
  | 'builtin:disperser'
  | 'builtin:mastering-limiter'
  | 'builtin:vocoder'
  | 'builtin:lfo-tremolo'
  | 'builtin:clipper'
  | 'builtin:upward-compressor'
  | 'builtin:roboter'
  | 'builtin:resonator'
  | `vst3:${string}`
  | `clap:${string}`

export type PluginFormat = 'vst3' | 'clap'
export type PluginParameterDescriptor = { id: string; name: string; module: string; min: number; max: number; defaultValue: number }
export type ExternalPluginRef = {
  format: PluginFormat
  uid: string
  name: string
  vendor: string
  path: string
  audioInputBuses?: number
  audioOutputBuses?: number
  supportsSidechain?: boolean
  paramCount?: number
  parameters?: PluginParameterDescriptor[]
}

export type AudioAssetInfo = {
  id: string
  path: string
  name: string
  durationSec: number
  sampleRate: number
  numChannels: number
  peaks: Float32Array
}

export type ClipGainPoint = { id: string; timeSec: number; valueDb: number; /** Outgoing segment bend, -1…1. */ curve?: number }

export type Clip = {
  id: string
  assetId: string
  name?: string
  startSec: number
  offsetSec: number
  durationSec: number
  gainDb: number
  fadeInSec: number
  fadeOutSec: number
  muted?: boolean
  /** Varispeed playback multiplier. */
  playbackRate?: number
  pitchSemitones?: number
  fineCents?: number
  reversed?: boolean
  fadeInCurve?: number
  fadeOutCurve?: number
  gainPoints?: ClipGainPoint[]
  warpMode?: 'none' | 'project' | 'half' | 'double'
  warpSourceBpm?: number
}

export type Send = {
  id: string
  targetBusId: string
  gainDb: number
  preFader: boolean
}

export type EffectInstance = {
  id: string
  type: EffectType
  bypassed: boolean
  params: Record<string, number>
  plugin?: ExternalPluginRef
  sidechain?: { enabled: boolean; sourceTrackId: string | null }
}

export type AutomationPoint = { id: string; timeSec: number; value: number; /** Outgoing segment bend, -1…1. */ curve?: number }
export type AutomationLane = {
  id: string
  targetKind: 'track' | 'instrument' | 'effect'
  targetId: string
  parameterId: string
  category: string
  label: string
  min: number
  max: number
  defaultValue: number
  points: AutomationPoint[]
  /** Arrangement-only lane height in CSS pixels. */
  height?: number
}

export const MIDI_PPQ = 960
/** Reserved controller lane id used for the MIDI pitch-bend wheel. */
export const MIDI_PITCH_BEND_LANE = -1
export type TrackKind = 'audio' | 'instrument'
export type MidiNote = { id: string; pitch: number; velocity: number; startTicks: number; lengthTicks: number; releaseVelocity: number; muted: boolean }
export type MidiControlPoint = { ticks: number; value: number }
/** CC 0…127, or MIDI_PITCH_BEND_LANE for the bipolar pitch wheel. */
export type CcLane = { cc: number; points: MidiControlPoint[] }
export type MidiClip = {
  id: string
  name: string
  startSec: number
  durationSec: number
  loopEnabled: boolean
  loopStartTicks: number
  loopLengthTicks: number
  notes: MidiNote[]
  ccLanes: CcLane[]
  transposeSemitones: number
  velocityScale: number
  muted: boolean
  color: string | null
}
export type InstrumentInstance = { id: string; type: 'builtin:testtone' | `vst3:${string}` | `clap:${string}`; params: Record<string, number>; bypassed: boolean; plugin?: ExternalPluginRef }

export type Track = {
  id: string
  kind: TrackKind
  name: string
  color: string
  clips: Clip[]
  midiClips: MidiClip[]
  instrument: InstrumentInstance | null
  volumeDb: number
  pan: number
  muted: boolean
  solo: boolean
  armed: boolean
  effects: EffectInstance[]
  sends: Send[]
  /** Main-output group bus. Null/undefined routes directly to the master. */
  outputBusId?: string | null
  automationOpen?: boolean
  automationLanes?: AutomationLane[]
  /** Arrangement-only per-track height in CSS pixels. */
  height?: number
}

export type Bus = {
  id: string
  name: string
  effects: EffectInstance[]
  volumeDb: number
  muted: boolean
}

export type MasterStrip = {
  volumeDb: number
  effects: EffectInstance[]
  muted: boolean
  dim: boolean
}

export type TimeSignature = { numerator: number; denominator: number }

export type Transport = {
  bpm: number
  timeSignature: TimeSignature
  playheadSec: number
  isPlaying: boolean
  loop: { enabled: boolean; startSec: number; endSec: number }
}

export type ProjectState = {
  formatVersion: 2
  meta: { name: string; sampleRate: number; createdAt: string }
  transport: Transport
  assets: Record<string, AudioAssetInfo>
  tracks: Track[]
  buses: Bus[]
  master: MasterStrip
}

/** Silence floor sent to the native graph for muted strips. */
export const MUTE_GAIN_DB = -144

/** Mute and dim are UI-side gain offsets; the native graph only knows a strip gain. */
export function effectiveBusGainDb(bus: Bus): number {
  return bus.muted ? MUTE_GAIN_DB : bus.volumeDb
}

export function effectiveMasterGainDb(master: MasterStrip): number {
  if (master.muted) return MUTE_GAIN_DB
  return master.dim ? master.volumeDb - 20 : master.volumeDb
}

export function secondsPerBeat(bpm: number): number {
  return 60 / Math.max(1, bpm)
}

export function secondsPerBar(bpm: number, signature: TimeSignature): number {
  return secondsPerBeat(bpm) * signature.numerator * (4 / signature.denominator)
}

export function ticksToSeconds(ticks: number, bpm: number): number {
  return ticks / MIDI_PPQ * secondsPerBeat(bpm)
}

/** Bar / beat / tick, all one-based for display. */
export function toBarsBeats(sec: number, bpm: number, signature: TimeSignature): { bar: number; beat: number; tick: number } {
  const beatsPerBar = signature.numerator * (4 / signature.denominator)
  const beats = Math.max(0, sec) / secondsPerBeat(bpm)
  const bar = Math.floor(beats / beatsPerBar)
  const beatInBar = beats - bar * beatsPerBar
  return { bar: bar + 1, beat: Math.floor(beatInBar) + 1, tick: Math.floor((beatInBar % 1) * MIDI_PPQ) }
}

export type GridOption = { label: string; ticks: number }

/** Shared musical grid used by the toolbar, arrangement snapping, and the piano roll. */
export const GRID_OPTIONS: readonly GridOption[] = [
  { label: '1/1', ticks: 3840 }, { label: '1/2', ticks: 1920 }, { label: '1/4', ticks: 960 },
  { label: '1/8', ticks: 480 }, { label: '1/16', ticks: 240 }, { label: '1/32', ticks: 120 }, { label: '1/64', ticks: 60 },
  { label: '1/4.', ticks: 1440 }, { label: '1/8.', ticks: 720 }, { label: '1/16.', ticks: 360 },
  { label: '1/4T', ticks: 640 }, { label: '1/8T', ticks: 320 }, { label: '1/16T', ticks: 160 }, { label: '1/32T', ticks: 80 },
]

export function gridLabel(ticks: number): string {
  return GRID_OPTIONS.find((option) => option.ticks === ticks)?.label ?? `${ticks}t`
}

export type GraphSnapshot = Pick<ProjectState, 'tracks' | 'buses' | 'master' | 'transport'>

export type PluginDescriptor = {
  format: PluginFormat
  uid: string
  name: string
  vendor: string
  category: string
  path: string
  isInstrument: boolean
  paramCount: number
  hasEditor: boolean
  audioInputBuses: number
  audioOutputBuses: number
  supportsSidechain: boolean
  parameters: PluginParameterDescriptor[]
}

export type OfflineRenderRequest = {
  pluginUid: string
  inputWavPath: string
  outputWavPath: string
  sampleRate: number
  params: Record<string, number>
}

export type OfflineRenderResult = {
  outputWavPath: string
  durationSec: number
  peakDb: number
}

export type EngineCapabilities = {
  supportsExternalPlugins: boolean
  supportsRealtimePluginInsert: boolean
  supportsOfflineRender: boolean
  supportedAudioExtensions: readonly string[]
}

export type Level = { peak: number; rms: number }
export type StereoLevel = { left: number; right: number }
export type MultibandLevels = { low: StereoLevel; mid: StereoLevel; high: StereoLevel }
export type DistortionSpectrum = readonly number[]
export type LimiterMetrics = { inputPeakDb: number; outputPeakDb: number; gainReductionDb: number; truePeakDb: number; momentaryLufs: number; shortTermLufs: number; integratedLufs: number }

export type AudioBackendInfo = { id: string; name: string; available: boolean; asio: boolean }
export type AudioDeviceInfo = {
  id: string
  name: string
  isDefault: boolean
  sampleRates: number[]
  bufferSizes: number[]
  channels: number
}
export type AudioSettings = { backendId: string; deviceId: string; sampleRate: number; bufferSize: number }
export type MidiInputPortInfo = { id: string; name: string; connected: boolean }
export type StreamStatus = { latencyMs: number; xruns: number; running: boolean; error?: string; pdcSamples: number }
export type EqFrequencyResponse = { frequencies: number[]; combinedDb: number[]; bandsDb: number[][] }
export type ExportProgress = { stage: string; renderedFrames: number; totalFrames: number; fraction: number }
export type ExportSettings = { format: 'wav' | 'mp3'; sampleRate: number; bitDepth: 16 | 24 | 32; mp3BitrateKbps: 128 | 192 | 256 | 320; normalize: boolean }

export class NotSupportedError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'NotSupportedError'
  }
}
