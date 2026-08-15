// Tauri Specta adapter with revisioned array meters and adaptive native-state polling.
import { Channel } from '@tauri-apps/api/core'
import type { IAudioEngine } from '../IAudioEngine'
import type {
  AudioAssetInfo, AudioBackendInfo, AudioDeviceInfo, AudioSettings, EngineCapabilities, EqFrequencyResponse, MidiInputPortInfo,
  GraphSnapshot, Level, LimiterMetrics, MultibandLevels, OfflineRenderRequest, OfflineRenderResult, PluginDescriptor,
  ExportSettings, ProjectState, StreamStatus,
  ExportProgress as UiExportProgress,
} from '../types'
import { NotSupportedError } from '../types'
import { commands } from './bindings'
import type { DecodeProgress, EngineError, ExportProgress, GraphSnapshot as NativeGraphSnapshot, PluginDescriptor as NativePluginDescriptor } from './bindings'
import { getAssetPeaks } from './binary'

const idleStatus: StreamStatus = { latencyMs: 0, xruns: 0, running: false, pdcSamples: 0 }

export class RustEngine implements IAudioEngine {
  private initialized = false
  private initPromise: Promise<void> | null = null
  private pollTimer: number | null = null
  private playheadSec = 0
  private readonly listeners = new Set<(sec: number) => void>()
  private trackLevels: Record<string, Level> = {}
  private masterLevel: Level = { peak: 0, rms: 0 }
  private streamStatus: StreamStatus = idleStatus
  private graphRevision = -1
  private trackIds: string[] = []
  private activeVoiceCounts: Record<string, number> = {}
  private readonly liveMidiNotes = new Map<string, Set<number>>()
  private multibandLevels: Record<string, MultibandLevels> = {}
  private distortionSpectra: Record<string, readonly number[]> = {}
  private limiterMetrics: Record<string, LimiterMetrics> = {}
  private pendingTrackIds: string[] = []
  private multibandMeterIds: string[] = []
  private pendingMultibandMeterIds: string[] = []
  private distortionMeterIds: string[] = []
  private pendingDistortionMeterIds: string[] = []
  private limiterMeterIds: string[] = []
  private pendingLimiterMeterIds: string[] = []
  private readonly exportListeners = new Set<(progress: UiExportProgress | null) => void>()
  private readonly pendingRealtime = new Map<string, () => Promise<CommandResult<unknown>>>()
  private realtimeFrame = 0

  async init(): Promise<void> {
    if (this.initialized) return
    if (this.initPromise) return this.initPromise
    this.initPromise = this.initialize()
    try {
      await this.initPromise
    } finally {
      this.initPromise = null
    }
  }

  private async initialize(): Promise<void> {
    if (!isTauriRuntime()) {
      this.streamStatus = { ...idleStatus, error: 'Native audio is available in the Tauri desktop app.' }
      this.initialized = true
      return
    }
    try {
      await unwrapCommand(commands.engineInit())
    } catch (error) {
      this.streamStatus = { ...idleStatus, error: describeEngineError(error) }
      throw error
    }
    this.initialized = true
    this.schedulePoll(0)
  }

  async dispose(): Promise<void> {
    if (this.pollTimer !== null) window.clearTimeout(this.pollTimer)
    if (this.realtimeFrame) cancelAnimationFrame(this.realtimeFrame)
    this.pollTimer = null
    this.realtimeFrame = 0
    this.pendingRealtime.clear()
    if (this.initialized) await unwrapCommand(commands.engineDispose()).catch(() => undefined)
    this.initialized = false
  }

  async loadAudioFile(path: string): Promise<AudioAssetInfo> {
    const progress = new Channel<DecodeProgress>()
    progress.onmessage = () => undefined
    const asset = await unwrapCommand(commands.engineLoadAudioFile(path, progress))
    const peaks = await getAssetPeaks(asset.id, 0).catch(async () => {
      const values = await unwrapCommand(commands.engineAssetPeaks(asset.id, 0)).catch(() => [])
      return Float32Array.from(values)
    })
    return { ...asset, durationSec: finite(asset.durationSec), peaks }
  }

  async unloadAsset(assetId: string): Promise<void> {
    await unwrapCommand(commands.engineUnloadAsset(assetId))
  }

  async exportProject(_project: ProjectState, outputPath: string, settings?: ExportSettings): Promise<void> {
    const progress = new Channel<ExportProgress>()
    progress.onmessage = (value) => this.publishExportProgress({ ...value, fraction: finite(value.fraction) })
    const audioSettings = await unwrapCommand(commands.engineGetAudioSettings())
    const sampleRate = settings?.sampleRate ?? audioSettings.sampleRate
    try {
      await unwrapCommand(commands.engineExportProject({
        outputPath,
        format: settings?.format ?? 'wav',
        bitDepth: settings?.bitDepth ?? 24,
        mp3BitrateKbps: settings?.mp3BitrateKbps ?? 320,
        sampleRate,
        normalize: settings?.normalize ?? false,
      }, progress))
    } finally {
      this.publishExportProgress(null)
    }
  }

  cancelExport(): void { this.send(commands.engineCancelExport()) }
  onExportProgress(cb: (progress: UiExportProgress | null) => void): () => void {
    this.exportListeners.add(cb)
    return () => this.exportListeners.delete(cb)
  }

  async syncGraph(snapshot: GraphSnapshot): Promise<void> {
    try {
      await this.init()
      if (!isTauriRuntime()) return
      this.pendingTrackIds = snapshot.tracks.map((track) => track.id)
      this.pendingMultibandMeterIds = [
        ...snapshot.tracks.flatMap((track) => track.effects),
        ...snapshot.buses.flatMap((bus) => bus.effects),
        ...snapshot.master.effects,
      ].filter((effect) => effect.type === 'builtin:multiband-compressor').map((effect) => effect.id)
      this.pendingDistortionMeterIds = [
        ...snapshot.tracks.flatMap((track) => track.effects),
        ...snapshot.buses.flatMap((bus) => bus.effects),
        ...snapshot.master.effects,
      ].filter((effect) => ['builtin:eq', 'builtin:eq8', 'builtin:distortion', 'builtin:disperser', 'builtin:clipper', 'builtin:roboter', 'builtin:resonator'].includes(effect.type)).map((effect) => effect.id)
      this.pendingLimiterMeterIds = [
        ...snapshot.tracks.flatMap((track) => track.effects),
        ...snapshot.buses.flatMap((bus) => bus.effects),
        ...snapshot.master.effects,
      ].filter((effect) => effect.type === 'builtin:mastering-limiter').map((effect) => effect.id)
      await unwrapCommand(commands.engineSyncGraph(toNativeSnapshot(snapshot)))
    } catch (error) {
      this.streamStatus = { ...this.streamStatus, error: describeEngineError(error) }
      throw error
    }
  }

  async play(fromSec?: number): Promise<void> { await this.init(); if (isTauriRuntime()) await unwrapCommand(commands.enginePlay(fromSec ?? null)) }
  async pause(): Promise<void> { await this.init(); if (isTauriRuntime()) await unwrapCommand(commands.enginePause()) }
  async stop(): Promise<void> { await this.init(); if (isTauriRuntime()) await unwrapCommand(commands.engineStop()) }
  async seek(sec: number): Promise<void> { await this.init(); if (isTauriRuntime()) await unwrapCommand(commands.engineSeek(sec)) }
  getPlayheadSec(): number { return this.playheadSec }

  onPlayhead(cb: (sec: number) => void): () => void {
    this.listeners.add(cb)
    cb(this.playheadSec)
    return () => this.listeners.delete(cb)
  }

  setTrackVolume(trackId: string, gainDb: number): void { this.queueRealtime(`volume:${trackId}`, () => commands.engineSetTrackVolume(trackId, gainDb)) }
  setTrackPan(trackId: string, pan: number): void { this.queueRealtime(`pan:${trackId}`, () => commands.engineSetTrackPan(trackId, pan)) }
  setTrackMute(trackId: string, muted: boolean): void { this.send(commands.engineSetTrackMute(trackId, muted)) }
  setTrackSolo(trackId: string, solo: boolean): void { this.send(commands.engineSetTrackSolo(trackId, solo)) }
  setSendLevel(sendId: string, gainDb: number): void { this.queueRealtime(`send:${sendId}`, () => commands.engineSetSendLevel(sendId, gainDb)) }
  setBusVolume(busId: string, gainDb: number): void { this.queueRealtime(`bus:${busId}`, () => commands.engineSetBusVolume(busId, gainDb)) }
  setMasterVolume(gainDb: number): void { this.queueRealtime('master', () => commands.engineSetMasterVolume(gainDb)) }
  setEffectParam(effectId: string, paramId: string, value: number): void { this.queueRealtime(`effect:${effectId}:${paramId}`, () => commands.engineSetEffectParam(effectId, paramId, value)) }
  setEffectBypass(effectId: string, bypassed: boolean): void { this.send(commands.engineSetEffectBypass(effectId, bypassed)) }

  async getEqResponse(effectId: string, points: number): Promise<EqFrequencyResponse | null> {
    if (!isTauriRuntime()) return null
    try {
      const response = await unwrapCommand(commands.engineEqResponse(effectId, points))
      return {
        frequencies: response.frequencies.map((value) => finite(value)),
        combinedDb: response.combinedDb.map((value) => finite(value)),
        bandsDb: response.bandsDb.map((band) => band.map((value) => finite(value))),
      }
    } catch { return null }
  }

  setInstrumentParam(trackId: string, paramId: string, value: number): void { this.queueRealtime(`instrument:${trackId}:${paramId}`, () => commands.engineSetInstrumentParam(trackId, paramId, value)) }
  midiNote(trackId: string, noteId: number, pitch: number, velocity: number, noteOn: boolean): void {
    const notes = this.liveMidiNotes.get(trackId) ?? new Set<number>()
    if (noteOn && velocity > 0) notes.add(noteId); else notes.delete(noteId)
    this.liveMidiNotes.set(trackId, notes)
    this.send(commands.engineMidiNote(trackId, noteId, pitch, velocity, noteOn))
  }
  midiAllNotesOff(trackId: string): void { this.liveMidiNotes.set(trackId, new Set()); this.send(commands.engineMidiAllNotesOff(trackId)) }
  getActiveVoiceCount(trackId: string): number { return Math.max(this.activeVoiceCounts[trackId] ?? 0, this.liveMidiNotes.get(trackId)?.size ?? 0) }
  getMidiGateCount(trackId: string): number | null { return this.liveMidiNotes.get(trackId)?.size ?? null }
  getTrackLevel(trackId: string): Level { return this.trackLevels[trackId] ?? { peak: 0, rms: 0 } }
  getMasterLevel(): Level { return this.masterLevel }
  getMultibandLevels(effectId: string): MultibandLevels { return this.multibandLevels[effectId] ?? EMPTY_MULTIBAND_LEVELS }
  getLimiterMetrics(effectId: string): LimiterMetrics { return this.limiterMetrics[effectId] ?? EMPTY_LIMITER_METRICS }
  getEffectSpectrum(effectId: string): readonly number[] { return this.distortionSpectra[effectId] ?? EMPTY_DISTORTION_SPECTRUM }
  getDistortionSpectrum(effectId: string): readonly number[] { return this.distortionSpectra[effectId] ?? EMPTY_DISTORTION_SPECTRUM }

  listAudioBackends(): Promise<AudioBackendInfo[]> { return unwrapCommand(commands.engineListAudioBackends()) }
  listOutputDevices(backendId: string): Promise<AudioDeviceInfo[]> { return unwrapCommand(commands.engineListOutputDevices(backendId)) }
  getAudioSettings(): Promise<AudioSettings> { return unwrapCommand(commands.engineGetAudioSettings()) }
  async setAudioSettings(settings: AudioSettings): Promise<void> { await unwrapCommand(commands.engineSetAudioSettings(settings)) }
  listMidiInputs(): Promise<MidiInputPortInfo[]> { return unwrapCommand(commands.engineListMidiInputs()) }
  async connectMidiInput(portId: string, trackId: string): Promise<void> { await unwrapCommand(commands.engineConnectMidiInput(portId, trackId)) }
  async disconnectMidiInput(portId: string): Promise<void> { await unwrapCommand(commands.engineDisconnectMidiInput(portId)) }
  getStreamStatus(): StreamStatus { return this.streamStatus }
  async cachedPlugins(): Promise<PluginDescriptor[]> {
    return this.normalizePlugins(await unwrapCommand(commands.cachedVst3Plugins()))
  }
  async scanPlugins(force = false): Promise<PluginDescriptor[]> {
    return this.normalizePlugins(await unwrapCommand(commands.scanVst3Plugins([], force)))
  }
  async inspectPlugin(plugin: PluginDescriptor): Promise<PluginDescriptor> {
    return this.normalizePlugins([await unwrapCommand(commands.inspectPluginMetadata(plugin.format, plugin.path, plugin.uid))])[0] ?? plugin
  }
  async openPluginEditor(targetKind: 'effect' | 'instrument', targetId: string, foreground = true): Promise<void> {
    await this.init()
    if (isTauriRuntime()) await unwrapCommand(commands.engineOpenPluginEditor(targetKind, targetId, foreground))
  }
  async closePluginEditor(targetKind: 'effect' | 'instrument', targetId: string): Promise<void> {
    if (isTauriRuntime()) await unwrapCommand(commands.engineClosePluginEditor(targetKind, targetId))
  }
  async isPluginEditorOpen(targetKind: 'effect' | 'instrument', targetId: string): Promise<boolean> {
    return isTauriRuntime() ? unwrapCommand(commands.enginePluginEditorIsOpen(targetKind, targetId)) : false
  }
  async savePluginState(targetKind: 'effect' | 'instrument', targetId: string): Promise<number[]> {
    return isTauriRuntime() ? unwrapCommand(commands.engineSavePluginState(targetKind, targetId)) : []
  }
  async loadPluginState(targetKind: 'effect' | 'instrument', targetId: string, state: number[]): Promise<void> {
    if (isTauriRuntime()) await unwrapCommand(commands.engineLoadPluginState(targetKind, targetId, state))
  }
  async renderOffline(_req: OfflineRenderRequest): Promise<OfflineRenderResult> { throw new NotSupportedError('VST3 rendering is available in phase three.') }

  capabilities(): EngineCapabilities {
    return { supportsExternalPlugins: true, supportsRealtimePluginInsert: true, supportsOfflineRender: true, supportedAudioExtensions: ['wav', 'mp3', 'flac', 'ogg', 'm4a', 'aac'] }
  }

  private normalizePlugins(plugins: NativePluginDescriptor[]): PluginDescriptor[] {
    return plugins
      .filter((plugin) => plugin.format === 'vst3' || plugin.format === 'clap')
      .map((plugin) => ({ ...plugin, format: plugin.format === 'clap' ? 'clap' : 'vst3', parameters: plugin.parameters.map((parameter) => ({ ...parameter, min: finite(parameter.min), max: finite(parameter.max, 1), defaultValue: finite(parameter.defaultValue) })) }))
  }

  private send(command: Promise<CommandResult<unknown>>): void {
    void unwrapCommand(command).catch((error) => { this.streamStatus = { ...this.streamStatus, error: describeEngineError(error) } })
  }

  private queueRealtime(key: string, command: () => Promise<CommandResult<unknown>>): void {
    this.pendingRealtime.set(key, command)
    if (this.realtimeFrame) return
    this.realtimeFrame = requestAnimationFrame(() => {
      this.realtimeFrame = 0
      const pending = [...this.pendingRealtime.values()]
      this.pendingRealtime.clear()
      for (const invoke of pending) this.send(invoke())
    })
  }

  private schedulePoll(delay: number): void {
    if (!this.initialized) return
    this.pollTimer = window.setTimeout(() => { void this.pollNativeState() }, delay)
  }

  private publishExportProgress(progress: UiExportProgress | null): void {
    for (const listener of this.exportListeners) listener(progress)
  }

  private async pollNativeState(): Promise<void> {
    let playing = false
    try {
      const state = await unwrapCommand(commands.enginePollState())
      playing = state.playing
      this.playheadSec = finite(state.playheadSec, this.playheadSec)
      if (state.graphRevision !== this.graphRevision) {
        this.graphRevision = state.graphRevision
        this.trackIds = [...this.pendingTrackIds]
        this.multibandMeterIds = [...this.pendingMultibandMeterIds]
        this.distortionMeterIds = [...this.pendingDistortionMeterIds]
        this.limiterMeterIds = [...this.pendingLimiterMeterIds]
      }
      for (let index = 0; index < this.trackIds.length; index += 1) {
        const id = this.trackIds[index]
        if (!id) continue
        const source = state.trackLevels[index]
        const level = this.trackLevels[id] ?? { peak: 0, rms: 0 }
        level.peak = finite(source?.peak ?? null)
        level.rms = finite(source?.rms ?? null)
        this.trackLevels[id] = level
        this.activeVoiceCounts[id] = state.activeVoiceCounts[index] ?? 0
        if (this.activeVoiceCounts[id] === 0 && this.liveMidiNotes.get(id)?.size === 0) this.liveMidiNotes.delete(id)
      }
      this.masterLevel.peak = finite(state.masterLevel.peak)
      this.masterLevel.rms = finite(state.masterLevel.rms)
      for (let index = 0; index < this.multibandMeterIds.length; index += 1) {
        const id = this.multibandMeterIds[index]
        const source = state.multibandLevels[index]
        if (!id || !source) continue
        this.multibandLevels[id] = {
          low: { left: finite(source.low.left), right: finite(source.low.right) },
          mid: { left: finite(source.mid.left), right: finite(source.mid.right) },
          high: { left: finite(source.high.left), right: finite(source.high.right) },
        }
      }
      for (let index = 0; index < this.distortionMeterIds.length; index += 1) {
        const id = this.distortionMeterIds[index]
        const source = state.distortionSpectra[index]
        if (!id || !source) continue
        let values = this.distortionSpectra[id] as number[] | undefined
        if (!values || values.length !== source.length) values = Array.from({ length: source.length }, () => 0)
        for (let bin = 0; bin < source.length; bin += 1) {
          const target = finite(source[bin] ?? null)
          const current = values[bin] ?? 0
          values[bin] = current + (target - current) * (target > current ? .68 : .36)
        }
        this.distortionSpectra[id] = values
      }
      for (let index = 0; index < this.limiterMeterIds.length; index += 1) {
        const id = this.limiterMeterIds[index]
        const source = state.limiterMetrics[index]
        if (!id || !source) continue
        this.limiterMetrics[id] = {
          inputPeakDb: finite(source.inputPeakDb, -120),
          outputPeakDb: finite(source.outputPeakDb, -120),
          gainReductionDb: finite(source.gainReductionDb),
          truePeakDb: finite(source.truePeakDb, -120),
          momentaryLufs: finite(source.momentaryLufs, -120),
          shortTermLufs: finite(source.shortTermLufs, -120),
          integratedLufs: finite(source.integratedLufs, -120),
        }
      }
      this.streamStatus.latencyMs = finite(state.stream.latencyMs)
      this.streamStatus.xruns = state.stream.xruns
      this.streamStatus.running = state.stream.running
      this.streamStatus.error = state.stream.error ?? undefined
      this.streamStatus.pdcSamples = state.stream.pdcSamples
      for (const listener of this.listeners) listener(this.playheadSec)
    } catch (error) {
      this.streamStatus = { ...this.streamStatus, running: false, error: describeEngineError(error) }
    } finally {
      // Analyzer buffers are produced by this snapshot. Poll at display rate
      // while transport is active; canvas consumers update imperatively and do
      // not cause React/store renders.
      const liveMidi = this.liveMidiNotes.size > 0 || Object.values(this.activeVoiceCounts).some((count) => count > 0)
      this.schedulePoll(playing || liveMidi ? 1000 / 60 : 1000 / 15)
    }
  }
}

const EMPTY_MULTIBAND_LEVELS: MultibandLevels = Object.freeze({
  low: Object.freeze({ left: 0, right: 0 }),
  mid: Object.freeze({ left: 0, right: 0 }),
  high: Object.freeze({ left: 0, right: 0 }),
})
const EMPTY_DISTORTION_SPECTRUM: readonly number[] = Object.freeze(Array.from({ length: 48 }, () => 0))
const EMPTY_LIMITER_METRICS: LimiterMetrics = Object.freeze({ inputPeakDb: -120, outputPeakDb: -120, gainReductionDb: 0, truePeakDb: -120, momentaryLufs: -120, shortTermLufs: -120, integratedLufs: -120 })

export function describeEngineError(error: unknown): string {
  if (error && typeof error === 'object' && 'kind' in error) {
    const typed = error as { kind: string; detail?: unknown }
    return typed.detail ? `${typed.kind}: ${String(typed.detail)}` : typed.kind
  }
  return error instanceof Error ? error.message : String(error)
}

type CommandResult<T> = { status: 'ok'; data: T } | { status: 'error'; error: EngineError }

async function unwrapCommand<T>(command: Promise<CommandResult<T>>): Promise<T> {
  const result = await command
  if (result.status === 'error') throw result.error
  return result.data
}

function finite(value: number | null, fallback = 0): number {
  return value !== null && Number.isFinite(value) ? value : fallback
}

function isTauriRuntime(): boolean { return '__TAURI_INTERNALS__' in window }

function toNativeSnapshot(snapshot: GraphSnapshot): NativeGraphSnapshot {
  return {
    tracks: snapshot.tracks.map((track) => ({
      id: track.id,
      kind: track.kind,
      name: track.name,
      clips: track.clips.map((clip) => {
        const sourceBpm = Math.max(20, clip.warpSourceBpm ?? snapshot.transport.bpm)
        const warp = clip.warpMode === 'project' ? snapshot.transport.bpm / sourceBpm : clip.warpMode === 'half' ? snapshot.transport.bpm / sourceBpm * .5 : clip.warpMode === 'double' ? snapshot.transport.bpm / sourceBpm * 2 : 1
        return { ...clip, muted: clip.muted ?? false, playbackRate: (clip.playbackRate ?? 1) * warp, pitchSemitones: clip.pitchSemitones ?? 0, fineCents: clip.fineCents ?? 0, reversed: clip.reversed ?? false, fadeInCurve: clip.fadeInCurve ?? 0, fadeOutCurve: clip.fadeOutCurve ?? 0, gainPoints: (clip.gainPoints ?? []).map((point) => ({ timeSec: point.timeSec, valueDb: point.valueDb, curve: point.curve ?? 0 })) }
      }),
      midiClips: track.midiClips.map((clip) => ({ id: clip.id, name: clip.name, startSec: clip.startSec, durationSec: clip.durationSec, loopEnabled: clip.loopEnabled, loopStartTicks: clip.loopStartTicks, loopLengthTicks: clip.loopLengthTicks, notes: clip.notes, ccLanes: clip.ccLanes, transposeSemitones: clip.transposeSemitones, velocityScale: clip.velocityScale, muted: clip.muted })),
      instrument: track.instrument,
      volumeDb: track.volumeDb,
      pan: track.pan,
      muted: track.muted,
      solo: track.solo,
      effects: track.effects,
      sends: track.sends,
      outputBusId: track.outputBusId ?? null,
    })),
    // Every native field is picked explicitly so UI-only state (bus mute, master
    // dim, time signature) never rides along into the Rust deserializer.
    buses: snapshot.buses.map((bus) => ({ id: bus.id, name: bus.name, volumeDb: bus.volumeDb, effects: bus.effects })),
    master: { volumeDb: snapshot.master.volumeDb, effects: snapshot.master.effects },
    transport: {
      bpm: snapshot.transport.bpm,
      playheadSec: snapshot.transport.playheadSec,
      isPlaying: snapshot.transport.isPlaying,
      loop: snapshot.transport.loop,
    },
  }
}
