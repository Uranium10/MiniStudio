// Stable audio-engine contract consumed by UI-facing hooks and stores.
import type {
  AudioAssetInfo,
  AudioBackendInfo,
  AudioDeviceInfo,
  AudioSettings,
  MidiInputPortInfo,
  EngineCapabilities,
  EqFrequencyResponse,
  GraphSnapshot,
  Level,
  MultibandLevels,
  LimiterMetrics,
  DistortionSpectrum,
  OfflineRenderRequest,
  OfflineRenderResult,
  PluginDescriptor,
  ProjectState,
  StreamStatus,
  ExportProgress,
  ExportSettings,
} from './types'

export interface IAudioEngine {
  init(): Promise<void>
  dispose(): Promise<void>
  loadAudioFile(path: string): Promise<AudioAssetInfo>
  unloadAsset(assetId: string): Promise<void>
  exportProject(project: ProjectState, outputPath: string, settings?: ExportSettings): Promise<void>
  cancelExport(): void
  onExportProgress(cb: (progress: ExportProgress | null) => void): () => void
  syncGraph(snapshot: GraphSnapshot): Promise<void>
  play(fromSec?: number, countInBars?: number): Promise<void>
  setMetronome(enabled: boolean, gainDb: number, bpm: number, numerator: number, denominator: number): Promise<void>
  pause(): Promise<void>
  stop(): Promise<void>
  seek(sec: number): Promise<void>
  getPlayheadSec(): number
  onPlayhead(cb: (sec: number) => void): () => void
  onPluginParameterChanges(cb: (changes: import('./types').PluginParameterChange[]) => void): () => void
  setTrackVolume(trackId: string, gainDb: number): void
  setTrackPan(trackId: string, pan: number): void
  setTrackMute(trackId: string, muted: boolean): void
  setTrackSolo(trackId: string, solo: boolean): void
  setSendLevel(sendId: string, gainDb: number): void
  setBusVolume(busId: string, gainDb: number): void
  setMasterVolume(gainDb: number): void
  setEffectParam(effectId: string, paramId: string, value: number): void
  setEffectBypass(effectId: string, bypassed: boolean): void
  /** Measured EQ magnitude response, or null when the engine cannot supply one. */
  getEqResponse(effectId: string, points: number): Promise<EqFrequencyResponse | null>
  setInstrumentParam(trackId: string, paramId: string, value: number): void
  midiNote(trackId: string, noteId: number, pitch: number, velocity: number, noteOn: boolean): void
  midiAllNotesOff(trackId: string): void
  getActiveVoiceCount(trackId: string): number
  /** Live UI/MIDI gate count, or null when only timeline voice state is known. */
  getMidiGateCount(trackId: string): number | null
  getTrackLevel(trackId: string): Level
  getMasterLevel(): Level
  getMultibandLevels(effectId: string): MultibandLevels
  getLimiterMetrics(effectId: string): LimiterMetrics
  getEffectSpectrum(effectId: string): DistortionSpectrum
  getDistortionSpectrum(effectId: string): DistortionSpectrum
  listAudioBackends(): Promise<AudioBackendInfo[]>
  listOutputDevices(backendId: string): Promise<AudioDeviceInfo[]>
  getAudioSettings(): Promise<AudioSettings>
  setAudioSettings(settings: AudioSettings): Promise<void>
  listMidiInputs(): Promise<MidiInputPortInfo[]>
  connectMidiInput(portId: string, trackId: string): Promise<void>
  disconnectMidiInput(portId: string): Promise<void>
  getStreamStatus(): StreamStatus
  cachedPlugins(): Promise<PluginDescriptor[]>
  scanPlugins(force?: boolean): Promise<PluginDescriptor[]>
  inspectPlugin(plugin: PluginDescriptor): Promise<PluginDescriptor>
  openPluginEditor(targetKind: 'effect' | 'instrument', targetId: string, foreground?: boolean): Promise<void>
  closePluginEditor(targetKind: 'effect' | 'instrument', targetId: string): Promise<void>
  isPluginEditorOpen(targetKind: 'effect' | 'instrument', targetId: string): Promise<boolean>
  setPluginEditorModal(targetId: string, modal: boolean): Promise<void>
  savePluginState(targetKind: 'effect' | 'instrument', targetId: string): Promise<number[]>
  loadPluginState(targetKind: 'effect' | 'instrument', targetId: string, state: number[]): Promise<void>
  renderOffline(req: OfflineRenderRequest): Promise<OfflineRenderResult>
  capabilities(): EngineCapabilities
}
