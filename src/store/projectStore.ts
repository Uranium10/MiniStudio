// Central Zustand project, selection, view, and undo/redo state.
import { create } from 'zustand'
import type { AudioAssetInfo, AudioSourceRef, AutomationLane, Clip, EffectInstance, ExternalPluginRef, MidiClip, MidiControlPoint, MidiNote, ProjectState, TimeSignature, Track } from '../engine'
import { MIDI_PITCH_BEND_LANE, MIDI_PPQ, normalizeTempoMap, secondsPerBeat, TempoMap, type TempoMapData } from '../engine'
import { createEmptyProject } from './demoProject'

export type LowerTab = 'mixer' | 'effects'
export type EditFocus = 'arrangement' | 'pianoRoll'
export type RackTarget = { kind: 'track' | 'bus' | 'master'; id: string }
export type BrowserDock = 'left' | 'right'
export type HistoryEntry = { id: string; label: string; timestamp: number }
export type AutomationOption = Omit<AutomationLane, 'id' | 'points' | 'height' | 'mode'>
export type ClipboardEntry = { trackOffset: number; kind: 'audio'; clip: Clip } | { trackOffset: number; kind: 'midi'; clip: MidiClip }
export type Clipboard = { originSec: number; entries: ClipboardEntry[] }
export type AutomationPointRef = { trackId: string; laneId: string; pointId: string }
export type ClipGainPointRef = { trackId: string; clipId: string; pointId: string }
export type AutomationClipboard = { sourceTrackId: string; sourceLaneId: string; originSec: number; points: Array<{ timeOffset: number; value: number; curve?: number }> }

type ProjectStore = {
  project: ProjectState
  playheadSec: number
  selectedTrackId: string | null
  selectedTrackIds: string[]
  selectedClipIds: string[]
  editorClip: { trackId: string; clipId: string } | null
  selectedNoteIds: string[]
  selectedAutomationPoints: AutomationPointRef[]
  selectedClipGainPoint: ClipGainPointRef | null
  editFocus: EditFocus
  editorMaximized: boolean
  pianoRollOpen: boolean
  pianoRollHeight: number
  inspectorVisible: boolean
  browserVisible: boolean
  browserDock: BrowserDock
  recordingEnabled: boolean
  metronomeEnabled: boolean
  metronomeVolumeDb: number
  countInBars: 0 | 1 | 2
  countInActive: boolean
  overdubMode: 'merge' | 'new'
  pixelsPerSecond: number
  pianoRollZoom: number
  trackHeight: number
  snapEnabled: boolean
  pianoSnapEnabled: boolean
  gridTicks: number
  pianoGridTicks: number
  arrangementSwing: number
  pianoSwing: number
  followPlayhead: boolean
  lowerPanelHeight: number
  mixerPanelHeight: number
  effectsPanelHeight: number
  lowerPanelCollapsed: boolean
  lowerTab: LowerTab
  toast: string | null
  clipboard: Clipboard | null
  automationClipboard: AutomationClipboard | null
  shortcutsOpen: boolean
  missingAssets: Array<{ id: string; name: string }>
  audioSettingsOpen: boolean
  exportDialogOpen: boolean
  virtualPianoOpen: boolean
  virtualPianoOctave: number
  virtualPianoVelocity: number
  rackTarget: RackTarget
  focusedEffectId: string | null
  past: ProjectState[]
  future: ProjectState[]
  history: HistoryEntry[]
  futureHistory: HistoryEntry[]
  setProject(project: ProjectState): void
  newProject(): void
  setMissingAssets(assets: Array<{ id: string; name: string }>): void
  setAudioSettingsOpen(open: boolean): void
  setExportDialogOpen(open: boolean): void
  setVirtualPianoOpen(open: boolean): void
  setVirtualPianoOctave(octave: number): void
  setVirtualPianoVelocity(velocity: number): void
  setRackTarget(target: RackTarget): void
  replaceAsset(oldId: string, asset: AudioAssetInfo): void
  setPlayhead(sec: number): void
  setPlaying(value: boolean): void
  setMetronomeEnabled(enabled: boolean): void
  setMetronomeVolumeDb(gainDb: number): void
  setCountInBars(bars: 0 | 1 | 2): void
  setCountInActive(active: boolean): void
  selectTrack(id: string | null, range?: boolean): void
  setEditFocus(focus: EditFocus): void
  selectClip(id: string, additive?: boolean): void
  clearClipSelection(): void
  addTrack(): void
  addInstrumentTrack(plugin?: ExternalPluginRef, insertIndex?: number): string
  replaceTrackInstrument(trackId: string, plugin?: ExternalPluginRef): void
  removeSelectedTrack(): void
  removeTrack(trackId: string): void
  duplicateTrack(trackId: string): void
  updateTrack(trackId: string, patch: Partial<Omit<Track, 'id' | 'clips' | 'effects' | 'sends'>>): void
  updateTrackVolumes(updates: Array<{ id: string; volumeDb: number }>): void
  createBusFromSelectedTracks(): string | null
  setTrackAutomationOpen(trackId: string, open: boolean): void
  addAutomationLane(trackId: string, lane: Omit<AutomationLane, 'id' | 'points' | 'height' | 'mode'>): void
  removeAutomationLane(trackId: string, laneId: string): void
  replaceAutomationLane(trackId: string, laneId: string, lane: AutomationOption): void
  setAutomationLaneMode(trackId: string, laneId: string, mode: NonNullable<AutomationLane['mode']>): void
  setAutomationLaneHeight(trackId: string, laneId: string, value: number): void
  upsertAutomationPoint(trackId: string, laneId: string, point: { id?: string; timeSec: number; value: number }): string | null
  setAutomationCurve(trackId: string, laneId: string, pointId: string, curve: number): void
  removeAutomationPoint(trackId: string, laneId: string, pointId: string): void
  selectAutomationPoint(point: AutomationPointRef, additive?: boolean): void
  clearAutomationPointSelection(): void
  copySelectedAutomationPoints(): void
  pasteAutomationPoints(atSec?: number): void
  duplicateSelectedAutomationPoints(): void
  deleteSelectedAutomationPoints(): void
  updateSend(trackId: string, sendId: string, gainDb: number): void
  addSend(trackId: string, busId: string): void
  removeSend(trackId: string, sendId: string): void
  updateSendRoute(trackId: string, sendId: string, busId: string): void
  updateBusVolume(busId: string, gainDb: number): void
  updateMasterVolume(gainDb: number): void
  toggleBusMute(busId: string): void
  toggleMasterMute(): void
  toggleMasterDim(): void
  reorderTrack(fromIndex: number, toIndex: number): void
  updateClip(trackId: string, clipId: string, patch: Partial<Omit<Clip, 'id' | 'audioSourceRefId'>>): void
  makeAudioSourceUnique(trackId: string, clipId: string): void
  upsertClipGainPoint(trackId: string, clipId: string, point: { id?: string; timeSec: number; valueDb: number; curve?: number }): string | null
  removeClipGainPoint(trackId: string, clipId: string, pointId: string): void
  selectClipGainPoint(point: ClipGainPointRef | null): void
  moveClipToTrack(sourceTrackId: string, targetTrackId: string, clipId: string): boolean
  splitClip(trackId: string, clipId: string, atSec: number): void
  deleteSelectedClips(): void
  duplicateSelectedClips(): void
  duplicateSelectedClipsSmart(): void
  copySelectedClips(): void
  cutSelectedClips(): void
  pasteClipboard(atSec?: number): void
  toggleSelectedClipsMuted(): void
  updateClipGain(trackId: string, clipId: string, gainDb: number): void
  duplicateClip(trackId: string, clipId: string): string | null
  addSilentClip(trackId: string, startSec: number, durationSec: number): void
  addAssetAsTrack(asset: AudioAssetInfo): void
  insertAudioAsset(asset: AudioAssetInfo, startSec: number, targetTrackId?: string, insertIndex?: number): void
  addMidiClip(trackId: string, startSec: number, durationSec: number): string | null
  openMidiEditor(trackId: string, clipId: string): void
  selectMidiNote(noteId: string, additive?: boolean): void
  clearNoteSelection(): void
  addMidiNote(trackId: string, clipId: string, note: Omit<MidiNote, 'id'>): string | null
  updateMidiNotes(trackId: string, clipId: string, noteIds: string[], patch: Partial<Omit<MidiNote, 'id'>>): void
  updateMidiNoteBatch(trackId: string, clipId: string, updates: Array<{ id: string; patch: Partial<Omit<MidiNote, 'id'>> }>): void
  upsertMidiControlPoints(trackId: string, clipId: string, cc: number, points: MidiControlPoint[]): void
  removeMidiControlPoint(trackId: string, clipId: string, cc: number, ticks: number): void
  duplicateMidiNotes(trackId: string, clipId: string, noteIds: string[], deltaTicks?: number, deltaPitch?: number): string[]
  duplicateSelectedMidiNotes(): void
  deleteMidiNotes(trackId: string, clipId: string, noteIds: string[]): void
  updateMidiClip(trackId: string, clipId: string, patch: Partial<Omit<MidiClip, 'id' | 'notes' | 'ccLanes'>>): void
  toggleEditorMaximized(): void
  togglePianoRoll(): void
  setPianoRollHeight(value: number): void
  toggleInspector(): void
  toggleBrowser(): void
  setBrowserDock(dock: BrowserDock): void
  setRecordingEnabled(enabled: boolean): void
  updateEffect(trackId: string, effectId: string, params: Record<string, number>): void
  toggleEffectBypass(trackId: string, effectId: string): void
  addEffect(trackId: string, type: EffectInstance['type'], plugin?: ExternalPluginRef): void
  removeEffect(trackId: string, effectId: string): void
  reorderEffect(trackId: string, fromIndex: number, toIndex: number): void
  updateTargetEffect(target: RackTarget, effectId: string, params: Record<string, number>): void
  updateInstrument(trackId: string, params: Record<string, number>): void
  toggleInstrumentBypass(trackId: string): void
  setTrackInstrumentPlugin(trackId: string, plugin: ExternalPluginRef): void
  toggleTargetEffect(target: RackTarget, effectId: string): void
  addTargetEffect(target: RackTarget, type: EffectInstance['type'], plugin?: ExternalPluginRef): void
  duplicateTargetEffect(target: RackTarget, effectId: string): void
  clearFocusedEffect(): void
  removeTargetEffect(target: RackTarget, effectId: string): void
  reorderTargetEffect(target: RackTarget, fromIndex: number, toIndex: number): void
  setTargetEffectSidechain(target: RackTarget, effectId: string, enabled: boolean, sourceId: string | null): void
  setBpm(bpm: number): void
  setTimeSignature(signature: TimeSignature): void
  toggleLoop(): void
  setLoopRange(startSec: number, endSec: number): void
  setLoopToSelection(): void
  setZoom(value: number): void
  setPianoRollZoom(value: number): void
  setTrackHeight(value: number): void
  setTrackViewHeight(trackId: string, value: number): void
  resizeSelectedTracks(delta: number): void
  resizeAllTracks(delta: number): void
  toggleSnap(): void
  togglePianoSnap(): void
  setGridTicks(ticks: number): void
  setPianoGridTicks(ticks: number): void
  setArrangementSwing(value: number): void
  setPianoSwing(value: number): void
  toggleFollowPlayhead(): void
  setLowerPanelHeight(value: number): void
  toggleLowerPanel(): void
  setLowerTab(tab: LowerTab): void
  setShortcutsOpen(open: boolean): void
  showToast(message: string): void
  clearToast(): void
  undo(): void
  redo(): void
}

/** Snap step in seconds for the shared musical grid. */
export function snapSeconds(gridTicks: number, bpm: number): number {
  return gridTicks / MIDI_PPQ * secondsPerBeat(bpm)
}

/** Quantize a tick position while delaying every second subdivision by swing %. */
export function snapTicksWithSwing(ticks: number, gridTicks: number, swing = 0): number {
  const size = Math.max(1, Math.round(gridTicks))
  const step = Math.round(ticks / size)
  const delayed = step % 2 !== 0 ? size * Math.max(0, Math.min(100, swing)) / 200 : 0
  return Math.max(0, Math.round(step * size + delayed))
}

export function snapTimeWithSwing(seconds: number, gridTicks: number, bpm: number, swing = 0, tempoMap?: TempoMapData): number {
  if (tempoMap) {
    const map = preparedTempoMap(tempoMap)
    return map.ticksToSeconds(snapTicksWithSwing(map.secondsToTicks(seconds), gridTicks, swing))
  }
  const ticks = seconds / secondsPerBeat(bpm) * MIDI_PPQ
  return snapTicksWithSwing(ticks, gridTicks, swing) / MIDI_PPQ * secondsPerBeat(bpm)
}

const tempoMapCache = new WeakMap<TempoMapData, TempoMap>()
function preparedTempoMap(data: TempoMapData): TempoMap {
  const existing = tempoMapCache.get(data)
  if (existing) return existing
  const map = new TempoMap(data)
  tempoMapCache.set(data, map)
  return map
}

export function clipSourceStep(clip: Clip, projectBpm: number, tempoMapData?: TempoMapData): number {
  if (tempoMapData) {
    const map = preparedTempoMap(tempoMapData)
    projectBpm = map.bpmAtTick(map.secondsToTicks(clip.startSec))
  }
  const sourceBpm = Math.max(20, clip.warpSourceBpm ?? projectBpm)
  const warp = clip.warpMode === 'project' ? projectBpm / sourceBpm : clip.warpMode === 'half' ? projectBpm / sourceBpm * .5 : clip.warpMode === 'double' ? projectBpm / sourceBpm * 2 : 1
  return (clip.playbackRate ?? 1) * warp * 2 ** (((clip.pitchSemitones ?? 0) + (clip.fineCents ?? 0) / 100) / 12)
}

/**
 * Chooses the next musical block for smart MIDI duplication.
 * Up to half a bar advances by half a bar, up to a bar by one bar, then the
 * distance doubles so the copied phrase always starts after the selection.
 */
export function musicalDuplicateStepTicks(extentTicks: number, signature: TimeSignature): number {
  if (extentTicks <= 0) return 0
  const barTicks = Math.max(1, Math.round(MIDI_PPQ * 4 * signature.numerator / signature.denominator))
  let step = Math.max(1, Math.round(barTicks / 2))
  while (step < extentTicks) step *= 2
  return step
}

export function midiDuplicateStepTicks(notes: readonly Pick<MidiNote, 'startTicks' | 'lengthTicks'>[], signature: TimeSignature): number {
  if (!notes.length) return 0
  const start = Math.min(...notes.map((note) => note.startTicks))
  const end = Math.max(...notes.map((note) => note.startTicks + note.lengthTicks))
  return musicalDuplicateStepTicks(Math.max(1, end - start), signature)
}

/**
 * MIDI edits may extend past the current event edge. Grow the arrangement item
 * to keep that material visible; explicit arrangement trimming still owns
 * shrinking behavior.
 */
function extendMidiClipToContent(clip: MidiClip, bpm: number, tempoMapData?: TempoMapData): MidiClip {
  const noteEnd = clip.notes.reduce((end, note) => Math.max(end, note.startTicks + note.lengthTicks), 0)
  const controllerEnd = clip.ccLanes.reduce((end, lane) => lane.points.reduce((laneEnd, point) => Math.max(laneEnd, point.ticks + 1), end), 0)
  const contentEnd = Math.max(noteEnd, controllerEnd)
  const map = tempoMapData ? preparedTempoMap(tempoMapData) : null
  const startTick = map?.secondsToTicks(clip.startSec) ?? 0
  const durationTicks = map ? Math.max(1, map.secondsToTicks(clip.startSec + clip.durationSec) - startTick) : Math.max(1, Math.round(clip.durationSec / secondsPerBeat(bpm) * MIDI_PPQ))
  if (contentEnd <= durationTicks) return clip
  return { ...clip, durationSec: map ? map.ticksToSeconds(startTick + contentEnd) - clip.startSec : contentEnd / MIDI_PPQ * secondsPerBeat(bpm), loopLengthTicks: Math.max(clip.loopLengthTicks, contentEnd) }
}

// Audio peaks are large immutable Float32Arrays. Project edits clone only the
// mutable graph structure and share assets/peaks between undo snapshots.
const cloneProject = (project: ProjectState, copyAssets = false): ProjectState => ({
  ...project,
  meta: { ...project.meta },
  transport: { ...project.transport, loop: { ...project.transport.loop }, tempoMap: { tempoPoints: project.transport.tempoMap.tempoPoints.map((point) => ({ ...point })), timeSignatures: project.transport.tempoMap.timeSignatures.map((point) => ({ ...point })) } },
  assets: copyAssets ? { ...project.assets } : project.assets,
  audioSourceRefs: Object.fromEntries(Object.entries(project.audioSourceRefs).map(([id, ref]) => [id, { ...ref }])),
  tracks: project.tracks.map((track) => ({
    ...track,
    clips: track.clips.map((clip) => ({ ...clip, gainPoints: clip.gainPoints?.map((point) => ({ ...point })) })),
    midiClips: track.midiClips.map((clip) => ({ ...clip, notes: clip.notes.map((note) => ({ ...note })), ccLanes: clip.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) })),
    instrument: track.instrument ? { ...track.instrument, params: { ...track.instrument.params } } : null,
    effects: track.effects.map((effect) => ({ ...effect, params: { ...effect.params }, sidechain: effect.sidechain ? { ...effect.sidechain } : undefined })),
    sends: track.sends.map((send) => ({ ...send })),
    automationLanes: track.automationLanes?.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })),
  })),
  buses: project.buses.map((bus) => ({
    ...bus,
    effects: bus.effects.map((effect) => ({ ...effect, params: { ...effect.params }, sidechain: effect.sidechain ? { ...effect.sidechain } : undefined })),
  })),
  master: {
    ...project.master,
    effects: project.master.effects.map((effect) => ({ ...effect, params: { ...effect.params }, sidechain: effect.sidechain ? { ...effect.sidechain } : undefined })),
  },
})

function createAudioSourceRef(project: ProjectState, asset: AudioAssetInfo, name = asset.name): AudioSourceRef {
  const id = `source-${crypto.randomUUID()}`
  const matching = Object.values(project.assets).find((candidate) => candidate.path && candidate.path.localeCompare(asset.path, undefined, { sensitivity: 'accent' }) === 0)
  const resolved = matching ?? asset
  project.assets[resolved.id] = resolved
  const ref = { id, assetId: resolved.id, name, modificationId: null }
  project.audioSourceRefs[id] = ref
  return ref
}

function collectUnusedAudioSources(project: ProjectState): void {
  const usedRefs = new Set(project.tracks.flatMap((track) => track.clips.map((clip) => clip.audioSourceRefId)))
  for (const id of Object.keys(project.audioSourceRefs)) if (!usedRefs.has(id)) delete project.audioSourceRefs[id]
  const usedAssets = new Set(Object.values(project.audioSourceRefs).map((ref) => ref.assetId))
  for (const id of Object.keys(project.assets)) if (!usedAssets.has(id)) delete project.assets[id]
}

function describeHistory(key: string): string {
  if (!key) return '프로젝트 편집'
  if (key.startsWith('track-height')) return '트랙 높이 변경'
  if (key.startsWith('track-volume')) return '트랙 볼륨 변경'
  if (key.startsWith('track:')) return '트랙 설정 변경'
  if (key.startsWith('automation-height')) return '오토메이션 레인 높이 변경'
  if (key.startsWith('automation-curve')) return '오토메이션 곡선 변경'
  if (key.startsWith('automation')) return '오토메이션 편집'
  if (key.startsWith('instrument')) return '악기 파라미터 변경'
  if (key.startsWith('effect')) return '이펙트 파라미터 변경'
  if (key.startsWith('midi')) return 'MIDI 편집'
  if (key.startsWith('clip-gain')) return '클립 게인 변경'
  if (key.startsWith('clip')) return '클립 편집'
  if (key.startsWith('send')) return '센드 레벨 변경'
  if (key.startsWith('bus-volume')) return '버스 볼륨 변경'
  if (key.startsWith('master-volume')) return '마스터 볼륨 변경'
  if (key.startsWith('transport')) return '프로젝트 템포 변경'
  return '프로젝트 편집'
}

export const useProjectStore = create<ProjectStore>((set, get) => {
  let lastHistoryKey = ''
  let lastHistoryAt = 0
  let toastTimer = 0
  const commitProject = (current: ProjectState, next: ProjectState, historyKey = '', historyLabel?: string) => {
    const now = performance.now()
    const coalesce = Boolean(historyKey && historyKey === lastHistoryKey && now - lastHistoryAt < 750)
    lastHistoryKey = historyKey
    lastHistoryAt = now
    const entry: HistoryEntry = { id: crypto.randomUUID(), label: historyLabel ?? describeHistory(historyKey), timestamp: Date.now() }
    set((state) => ({
      project: next,
      past: coalesce ? state.past : [...state.past.slice(-49), current],
      future: [],
      history: coalesce ? [...state.history.slice(0, -1), entry] : [...state.history.slice(-49), entry],
      futureHistory: [],
    }))
  }
  const mutateProject = (mutator: (draft: ProjectState) => void, options?: { copyAssets?: boolean; historyKey?: string; historyLabel?: string }) => {
    const current = get().project
    const next = cloneProject(current, options?.copyAssets)
    mutator(next)
    commitProject(current, next, options?.historyKey, options?.historyLabel)
  }
  const replaceMidiClip = (current: ProjectState, trackIndex: number, clipIndex: number, clip: MidiClip, historyKey = '') => {
    const sourceTrack = current.tracks[trackIndex]!
    const midiClips = sourceTrack.midiClips.slice()
    midiClips[clipIndex] = extendMidiClipToContent(clip, current.transport.bpm, current.transport.tempoMap)
    const tracks = current.tracks.slice()
    tracks[trackIndex] = { ...sourceTrack, midiClips }
    commitProject(current, { ...current, tracks }, historyKey)
  }

  const initialProject = createEmptyProject()
  return {
    project: initialProject,
    playheadSec: initialProject.transport.playheadSec,
    selectedTrackId: 'track-0',
    selectedTrackIds: ['track-0'],
    selectedClipIds: [],
    editorClip: null,
    selectedNoteIds: [],
    selectedAutomationPoints: [],
    selectedClipGainPoint: null,
    editFocus: 'arrangement',
    editorMaximized: false,
    pianoRollOpen: false,
    pianoRollHeight: 300,
    inspectorVisible: true,
    browserVisible: false,
    browserDock: 'right',
    recordingEnabled: false,
    metronomeEnabled: false,
    metronomeVolumeDb: -12,
    countInBars: 0,
    countInActive: false,
    overdubMode: 'merge',
    pixelsPerSecond: 30,
    pianoRollZoom: 66,
    trackHeight: 72,
    snapEnabled: true,
    pianoSnapEnabled: true,
    gridTicks: 960,
    pianoGridTicks: 240,
    arrangementSwing: 0,
    pianoSwing: 0,
    followPlayhead: true,
    lowerPanelHeight: 350,
    mixerPanelHeight: 350,
    effectsPanelHeight: 350,
    lowerPanelCollapsed: false,
    lowerTab: 'mixer',
    toast: null,
    clipboard: null,
    automationClipboard: null,
    shortcutsOpen: false,
    missingAssets: [],
    audioSettingsOpen: false,
    exportDialogOpen: false,
    virtualPianoOpen: false,
    virtualPianoOctave: 4,
    virtualPianoVelocity: 100,
    rackTarget: { kind: 'track', id: 'track-0' },
    focusedEffectId: null,
    past: [],
    future: [],
    history: [],
    futureHistory: [],
    setProject: (project) => {
      lastHistoryKey = ''
      const migrated: ProjectState = {
        ...project,
        formatVersion: 3 as const,
        transport: (() => {
          const timeSignature = project.transport.timeSignature ?? { numerator: 4, denominator: 4 }
          const tempoMap = normalizeTempoMap(project.transport.tempoMap, project.transport.bpm, timeSignature.numerator, timeSignature.denominator)
          return { ...project.transport, bpm: tempoMap.tempoPoints[0]!.bpm, timeSignature, tempoMap }
        })(),
        tracks: project.tracks.map((track) => ({ ...track, kind: track.kind ?? 'audio', clips: (track.clips ?? []).map((clip) => ({ ...clip, fadeInCurve: clip.fadeInCurve ?? 0, fadeOutCurve: clip.fadeOutCurve ?? 0, gainPoints: (clip.gainPoints ?? []).map((point) => ({ ...point })), warpMode: clip.warpMode ?? 'none', warpSourceBpm: clip.warpSourceBpm ?? project.transport.bpm })), midiClips: (track.midiClips ?? []).map((clip) => ({ ...clip, notes: clip.notes ?? [], ccLanes: clip.ccLanes ?? [] })), instrument: track.instrument ?? null, outputBusId: track.outputBusId ?? null, automationOpen: track.automationOpen ?? false, automationLanes: (track.automationLanes ?? []).map((lane) => ({ ...lane, mode: lane.mode ?? 'read', height: lane.height == null ? undefined : Math.max(54, Math.min(240, lane.height)), points: lane.points ?? [] })), height: track.height == null ? undefined : Math.max(46, Math.min(240, track.height)) })),
        buses: project.buses.map((bus) => ({ ...bus, muted: bus.muted ?? false })),
        master: { ...project.master, muted: project.master.muted ?? false, dim: project.master.dim ?? false },
        audioSourceRefs: Object.fromEntries(Object.entries(project.audioSourceRefs ?? {}).map(([id, ref]) => [id, { ...ref, modificationId: ref.modificationId ?? null }])),
      }
      const selectedTrackId = migrated.tracks[0]?.id ?? null
      set({ project: migrated, playheadSec: migrated.transport.playheadSec, past: [], future: [], history: [], futureHistory: [], selectedTrackId, selectedTrackIds: selectedTrackId ? [selectedTrackId] : [], selectedClipIds: [], selectedNoteIds: [], selectedAutomationPoints: [], selectedClipGainPoint: null, editFocus: 'arrangement', editorClip: null, pianoRollOpen: false, editorMaximized: false, virtualPianoOpen: false, focusedEffectId: null })
    },
    newProject: () => get().setProject(createEmptyProject()),
    setMissingAssets: (assets) => set({ missingAssets: assets }),
    setAudioSettingsOpen: (open) => set({ audioSettingsOpen: open }),
    setExportDialogOpen: (open) => set({ exportDialogOpen: open }),
    setVirtualPianoOpen: (open) => set({ virtualPianoOpen: open }),
    setVirtualPianoOctave: (octave) => set({ virtualPianoOctave: Math.max(0, Math.min(8, Math.round(octave))) }),
    setVirtualPianoVelocity: (velocity) => set({ virtualPianoVelocity: Math.max(1, Math.min(127, Math.round(velocity))) }),
    setRackTarget: (target) => set({ rackTarget: target, lowerTab: 'effects', lowerPanelCollapsed: false }),
    replaceAsset: (oldId, asset) => mutateProject((project) => {
      delete project.assets[oldId]
      project.assets[asset.id] = asset
      for (const ref of Object.values(project.audioSourceRefs)) if (ref.assetId === oldId) ref.assetId = asset.id
      queueMicrotask(() => set((state) => ({ missingAssets: state.missingAssets.filter((missing) => missing.id !== oldId) })))
    }, { copyAssets: true }),
    setPlayhead: (sec) => set({ playheadSec: Math.max(0, sec) }),
    setPlaying: (value) => set((state) => ({ project: { ...state.project, transport: { ...state.project.transport, isPlaying: value } } })),
    setMetronomeEnabled: (enabled) => set({ metronomeEnabled: enabled }),
    setMetronomeVolumeDb: (gainDb) => set({ metronomeVolumeDb: Math.max(-60, Math.min(6, gainDb)) }),
    setCountInBars: (bars) => set({ countInBars: bars }),
    setCountInActive: (active) => { if (get().countInActive !== active) set({ countInActive: active }) },
    selectTrack: (id, range = false) => set((state) => {
      if (!id) return { selectedTrackId: null, selectedTrackIds: [], selectedAutomationPoints: [], selectedClipGainPoint: null }
      if (!range || !state.selectedTrackId) return { selectedTrackId: id, selectedTrackIds: [id], selectedClipIds: [], selectedAutomationPoints: [], selectedClipGainPoint: null, editFocus: 'arrangement' as const, rackTarget: { kind: 'track' as const, id } }
      const anchor = state.project.tracks.findIndex((track) => track.id === state.selectedTrackId)
      const target = state.project.tracks.findIndex((track) => track.id === id)
      if (anchor < 0 || target < 0) return { selectedTrackId: id, selectedTrackIds: [id], selectedClipIds: [], selectedAutomationPoints: [], selectedClipGainPoint: null, editFocus: 'arrangement' as const, rackTarget: { kind: 'track' as const, id } }
      const [from, to] = anchor < target ? [anchor, target] : [target, anchor]
      return { selectedTrackId: id, selectedTrackIds: state.project.tracks.slice(from, to + 1).map((track) => track.id), selectedClipIds: [], selectedAutomationPoints: [], selectedClipGainPoint: null, editFocus: 'arrangement' as const, rackTarget: { kind: 'track' as const, id } }
    }),
    setEditFocus: (focus) => set({ editFocus: focus }),
    selectClip: (id, additive = false) => set((state) => ({ selectedClipIds: additive ? [...new Set([...state.selectedClipIds, id])] : [id], selectedAutomationPoints: [], selectedClipGainPoint: null, editFocus: 'arrangement' })),
    clearClipSelection: () => set({ selectedClipIds: [] }),
    addTrack: () => mutateProject((project) => {
      const id = `track-${crypto.randomUUID()}`
      project.tracks.push({ id, kind: 'audio', name: `Audio ${project.tracks.length + 1}`, color: '#5ba8ff', clips: [], midiClips: [], instrument: null, volumeDb: 0, pan: 0, muted: false, solo: false, armed: false, effects: [], sends: project.buses.map((bus) => ({ id: crypto.randomUUID(), targetBusId: bus.id, gainDb: -60, preFader: false })), outputBusId: null, automationOpen: false, automationLanes: [], height: get().trackHeight })
      queueMicrotask(() => set({ selectedTrackId: id, selectedTrackIds: [id] }))
    }, { historyLabel: '오디오 트랙 추가' }),
    addInstrumentTrack: (plugin, insertIndex) => {
      const id = `instrument-${crypto.randomUUID()}`
      mutateProject((project) => {
        const instrumentNumber = project.tracks.filter((track) => track.kind === 'instrument').length + 1
        const track: Track = { id, kind: 'instrument', name: plugin?.name ?? `Instrument ${instrumentNumber}`, color: '#66d3ff', clips: [], midiClips: [], instrument: createInstrumentInstance(plugin), volumeDb: 0, pan: 0, muted: false, solo: false, armed: true, effects: [], sends: [], outputBusId: null, automationOpen: false, automationLanes: [], height: get().trackHeight }
        const index = insertIndex == null ? project.tracks.length : Math.max(0, Math.min(project.tracks.length, Math.round(insertIndex)))
        project.tracks.splice(index, 0, track)
      }, { historyLabel: `${plugin?.name ?? 'DefaultSynth'} 트랙 추가` })
      set({ selectedTrackId: id, selectedTrackIds: [id] })
      return id
    },
    replaceTrackInstrument: (trackId, plugin) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId && candidate.kind === 'instrument')
      if (!track) return
      track.instrument = createInstrumentInstance(plugin)
      track.automationLanes = (track.automationLanes ?? []).filter((lane) => lane.targetKind !== 'instrument')
    }, { historyLabel: `${plugin?.name ?? 'DefaultSynth'} 악기로 교체` }),
    removeSelectedTrack: () => {
      const id = get().selectedTrackId
      if (id) get().removeTrack(id)
    },
    removeTrack: (trackId) => {
      const removedName = get().project.tracks.find((track) => track.id === trackId)?.name ?? '트랙'
      mutateProject((project) => { project.tracks = project.tracks.filter((track) => track.id !== trackId); collectUnusedAudioSources(project) }, { copyAssets: true, historyLabel: `${removedName} 삭제` })
      const remaining = get().project.tracks
      set((state) => ({
        selectedTrackId: state.selectedTrackId === trackId ? remaining[0]?.id ?? null : state.selectedTrackId,
        selectedTrackIds: (() => { const ids = state.selectedTrackIds.filter((id) => id !== trackId); return ids.length ? ids : remaining[0] ? [remaining[0].id] : [] })(),
        selectedClipIds: [],
        editorClip: state.editorClip?.trackId === trackId ? null : state.editorClip,
        rackTarget: state.rackTarget.kind === 'track' && state.rackTarget.id === trackId ? { kind: 'track' as const, id: remaining[0]?.id ?? '' } : state.rackTarget,
      }))
    },
    duplicateTrack: (trackId) => {
      const id = `${trackId.startsWith('instrument-') ? 'instrument' : 'track'}-${crypto.randomUUID()}`
      let created = false
      mutateProject((project) => {
        const index = project.tracks.findIndex((track) => track.id === trackId)
        const source = project.tracks[index]
        if (!source) return
        project.tracks.splice(index + 1, 0, {
          ...source,
          id,
          name: `${source.name} copy`,
          clips: source.clips.map((clip) => ({ ...clip, id: crypto.randomUUID() })),
          midiClips: source.midiClips.map((clip) => ({ ...clip, id: crypto.randomUUID(), notes: clip.notes.map((note) => ({ ...note, id: crypto.randomUUID() })), ccLanes: clip.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) })),
          instrument: source.instrument ? { ...source.instrument, id: crypto.randomUUID(), params: { ...source.instrument.params } } : null,
          effects: source.effects.map((effect) => ({ ...effect, id: crypto.randomUUID(), params: { ...effect.params } })),
          sends: source.sends.map((send) => ({ ...send, id: crypto.randomUUID() })),
          automationOpen: false,
          automationLanes: [],
        })
        created = true
      })
      if (created) set({ selectedTrackId: id, selectedTrackIds: [id], rackTarget: { kind: 'track', id } })
    },
    updateTrack: (trackId, patch) => {
      const current = get().project
      const index = current.tracks.findIndex((track) => track.id === trackId)
      if (index < 0) return
      const tracks = current.tracks.slice()
      let updated = { ...tracks[index]!, ...patch }
      if (typeof patch.volumeDb === 'number') updated = recordArmedAutomation(updated, get().playheadSec, 'track', trackId, 'volumeDb', patch.volumeDb, current.transport.isPlaying)
      if (typeof patch.pan === 'number') updated = recordArmedAutomation(updated, get().playheadSec, 'track', trackId, 'pan', patch.pan, current.transport.isPlaying)
      tracks[index] = updated
      commitProject(current, { ...current, tracks }, `track:${trackId}:${Object.keys(patch).sort().join(',')}`)
    },
    updateTrackVolumes: (updates) => {
      const current = get().project
      const values = new Map(updates.map((update) => [update.id, Math.max(-60, Math.min(12, update.volumeDb))]))
      if (!values.size) return
      const tracks = current.tracks.map((track) => values.has(track.id) ? recordArmedAutomation({ ...track, volumeDb: values.get(track.id)! }, get().playheadSec, 'track', track.id, 'volumeDb', values.get(track.id)!, current.transport.isPlaying) : track)
      commitProject(current, { ...current, tracks }, `track-volumes:${[...values.keys()].sort().join(',')}`)
    },
    createBusFromSelectedTracks: () => {
      const selected = new Set(get().selectedTrackIds)
      if (!selected.size) return null
      const id = `bus-${crypto.randomUUID()}`
      const current = get().project
      const number = current.buses.length + 1
      const next = cloneProject(current)
      next.buses.push({ id, name: `Bus ${number}`, volumeDb: 0, muted: false, effects: [] })
      for (const track of next.tracks) if (selected.has(track.id)) track.outputBusId = id
      commitProject(current, next)
      set({ rackTarget: { kind: 'bus', id }, lowerTab: 'mixer', lowerPanelCollapsed: false })
      return id
    },
    setTrackAutomationOpen: (trackId, open) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (track) track.automationOpen = open
    }),
    addAutomationLane: (trackId, lane) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (!track) return
      track.automationLanes ??= []
      if (track.automationLanes.some((item) => item.targetKind === lane.targetKind && item.targetId === lane.targetId && item.parameterId === lane.parameterId)) return
      track.automationLanes.push({ ...lane, id: crypto.randomUUID(), points: [], mode: 'read' })
      track.automationOpen = true
    }),
    removeAutomationLane: (trackId, laneId) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (track) track.automationLanes = (track.automationLanes ?? []).filter((lane) => lane.id !== laneId)
    }),
    replaceAutomationLane: (trackId, laneId, lane) => mutateProject((project) => {
      const lanes = project.tracks.find((candidate) => candidate.id === trackId)?.automationLanes
      const index = lanes?.findIndex((candidate) => candidate.id === laneId) ?? -1
      if (!lanes || index < 0) return
      const previous = lanes[index]!
      lanes[index] = { ...lane, id: previous.id, points: [], height: previous.height, mode: previous.mode ?? 'read' }
    }),
    setAutomationLaneMode: (trackId, laneId, mode) => mutateProject((project) => {
      const lane = project.tracks.find((candidate) => candidate.id === trackId)?.automationLanes?.find((candidate) => candidate.id === laneId)
      if (lane) lane.mode = mode
    }),
    setAutomationLaneHeight: (trackId, laneId, value) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const laneIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.automationLanes?.findIndex((lane) => lane.id === laneId) ?? -1
      if (laneIndex < 0) return
      const tracks = current.tracks.slice()
      const track = tracks[trackIndex]!
      const lanes = (track.automationLanes ?? []).slice()
      lanes[laneIndex] = { ...lanes[laneIndex]!, height: Math.max(54, Math.min(240, value)) }
      tracks[trackIndex] = { ...track, automationLanes: lanes }
      commitProject(current, { ...current, tracks }, `automation-height:${trackId}:${laneId}`)
    },
    upsertAutomationPoint: (trackId, laneId, point) => {
      const id = point.id ?? crypto.randomUUID()
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const track = current.tracks[trackIndex]
      const laneIndex = track?.automationLanes?.findIndex((item) => item.id === laneId) ?? -1
      if (!track || laneIndex < 0) return null
      const lane = track.automationLanes![laneIndex]!
      const pointIndex = lane.points.findIndex((item) => item.id === id)
      const value = Math.max(lane.min, Math.min(lane.max, point.value))
      const next = { id, timeSec: Math.max(0, point.timeSec), value, ...(pointIndex >= 0 && lane.points[pointIndex]!.curve != null ? { curve: lane.points[pointIndex]!.curve } : {}) }
      const points = lane.points.slice()
      if (pointIndex >= 0) points[pointIndex] = next; else points.push(next)
      points.sort((left, right) => left.timeSec - right.timeSec)
      const lanes = track.automationLanes!.slice()
      lanes[laneIndex] = { ...lane, points }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = { ...track, automationLanes: lanes }
      commitProject(current, { ...current, tracks }, `automation:${trackId}:${laneId}:${id}`)
      return id
    },
    setAutomationCurve: (trackId, laneId, pointId, curve) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const track = current.tracks[trackIndex]
      const laneIndex = track?.automationLanes?.findIndex((lane) => lane.id === laneId) ?? -1
      const lane = laneIndex >= 0 ? track!.automationLanes![laneIndex]! : undefined
      const pointIndex = lane?.points.findIndex((item) => item.id === pointId) ?? -1
      if (!track || !lane || pointIndex < 0) return
      const points = lane.points.slice()
      points[pointIndex] = { ...points[pointIndex]!, curve: Math.max(-1, Math.min(1, curve)) }
      const lanes = track.automationLanes!.slice()
      lanes[laneIndex] = { ...lane, points }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = { ...track, automationLanes: lanes }
      commitProject(current, { ...current, tracks }, `automation-curve:${trackId}:${laneId}:${pointId}`)
    },
    removeAutomationPoint: (trackId, laneId, pointId) => mutateProject((project) => {
      const lane = project.tracks.find((track) => track.id === trackId)?.automationLanes?.find((item) => item.id === laneId)
      if (lane) lane.points = lane.points.filter((point) => point.id !== pointId)
    }),
    selectAutomationPoint: (point, additive = false) => set((state) => {
      const sameLane = state.selectedAutomationPoints.every((item) => item.trackId === point.trackId && item.laneId === point.laneId)
      const points = additive && sameLane ? [...new Map([...state.selectedAutomationPoints, point].map((item) => [item.pointId, item])).values()] : [point]
      return { selectedAutomationPoints: points, selectedClipGainPoint: null, selectedClipIds: [], editFocus: 'arrangement' as const }
    }),
    clearAutomationPointSelection: () => set({ selectedAutomationPoints: [] }),
    copySelectedAutomationPoints: () => {
      const state = get()
      const refs = state.selectedAutomationPoints
      if (!refs.length) return
      const first = refs[0]!
      const lane = state.project.tracks.find((track) => track.id === first.trackId)?.automationLanes?.find((item) => item.id === first.laneId)
      const selected = new Set(refs.map((item) => item.pointId))
      const points = lane?.points.filter((point) => selected.has(point.id)) ?? []
      if (!points.length) return
      const originSec = Math.min(...points.map((point) => point.timeSec))
      set({ automationClipboard: { sourceTrackId: first.trackId, sourceLaneId: first.laneId, originSec, points: points.map((point) => ({ timeOffset: point.timeSec - originSec, value: point.value, ...(point.curve == null ? {} : { curve: point.curve }) })) } })
    },
    pasteAutomationPoints: (atSec) => {
      const state = get(); const clipboard = state.automationClipboard
      if (!clipboard?.points.length) return
      const ids: AutomationPointRef[] = []
      mutateProject((project) => {
        const lane = project.tracks.find((track) => track.id === clipboard.sourceTrackId)?.automationLanes?.find((item) => item.id === clipboard.sourceLaneId)
        if (!lane) return
        for (const source of clipboard.points) { const id = crypto.randomUUID(); lane.points.push({ id, timeSec: Math.max(0, (atSec ?? state.playheadSec) + source.timeOffset), value: Math.max(lane.min, Math.min(lane.max, source.value)), ...(source.curve == null ? {} : { curve: source.curve }) }); ids.push({ trackId: clipboard.sourceTrackId, laneId: clipboard.sourceLaneId, pointId: id }) }
        lane.points.sort((left, right) => left.timeSec - right.timeSec)
      }, { historyLabel: '오토메이션 포인트 붙여넣기' })
      if (ids.length) set({ selectedAutomationPoints: ids })
    },
    duplicateSelectedAutomationPoints: () => {
      const state = get(); const refs = state.selectedAutomationPoints
      if (!refs.length) return
      const first = refs[0]!; const selected = new Set(refs.map((item) => item.pointId))
      const lane = state.project.tracks.find((track) => track.id === first.trackId)?.automationLanes?.find((item) => item.id === first.laneId)
      const points = lane?.points.filter((point) => selected.has(point.id)) ?? []
      if (!points.length) return
      const firstSec = Math.min(...points.map((point) => point.timeSec))
      const lastSec = Math.max(...points.map((point) => point.timeSec))
      const map = new TempoMap(state.project.transport.tempoMap)
      const firstTick = map.secondsToTicks(firstSec)
      const deltaTicks = studioOneDuplicateStepTicks(Math.max(1, map.secondsToTicks(lastSec) - firstTick), state.gridTicks, state.snapEnabled)
      const ids: AutomationPointRef[] = []
      mutateProject((project) => {
        const target = project.tracks.find((track) => track.id === first.trackId)?.automationLanes?.find((item) => item.id === first.laneId)
        if (!target) return
        for (const point of points) { const id = crypto.randomUUID(); target.points.push({ ...point, id, timeSec: map.ticksToSeconds(map.secondsToTicks(point.timeSec) + deltaTicks) }); ids.push({ ...first, pointId: id }) }
        target.points.sort((left, right) => left.timeSec - right.timeSec)
      }, { historyLabel: '오토메이션 포인트 복제' })
      if (ids.length) set({ selectedAutomationPoints: ids })
    },
    deleteSelectedAutomationPoints: () => {
      const refs = get().selectedAutomationPoints
      if (!refs.length) return
      const selected = new Set(refs.map((item) => `${item.trackId}:${item.laneId}:${item.pointId}`))
      mutateProject((project) => { for (const track of project.tracks) for (const lane of track.automationLanes ?? []) lane.points = lane.points.filter((point) => !selected.has(`${track.id}:${lane.id}:${point.id}`)) }, { historyLabel: '오토메이션 포인트 삭제' })
      set({ selectedAutomationPoints: [] })
    },
    updateSend: (trackId, sendId, gainDb) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const sendIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.sends.findIndex((send) => send.id === sendId)
      if (sendIndex < 0) return
      const track = current.tracks[trackIndex]!
      const sends = track.sends.slice()
      sends[sendIndex] = { ...sends[sendIndex]!, gainDb }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = recordArmedAutomation({ ...track, sends }, get().playheadSec, 'send', sendId, 'gainDb', gainDb, current.transport.isPlaying)
      commitProject(current, { ...current, tracks }, `send:${trackId}:${sendId}`)
    },
    addSend: (trackId, busId) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (track && !track.sends.some((send) => send.targetBusId === busId)) track.sends.push({ id: crypto.randomUUID(), targetBusId: busId, gainDb: -60, preFader: false })
    }),
    removeSend: (trackId, sendId) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (track) track.sends = track.sends.filter((send) => send.id !== sendId)
    }),
    updateSendRoute: (trackId, sendId, busId) => mutateProject((project) => {
      const send = project.tracks.find((track) => track.id === trackId)?.sends.find((candidate) => candidate.id === sendId)
      if (send) send.targetBusId = busId
    }),
    updateBusVolume: (busId, gainDb) => {
      const current = get().project
      const index = current.buses.findIndex((bus) => bus.id === busId)
      if (index < 0) return
      const buses = current.buses.slice()
      buses[index] = { ...buses[index]!, volumeDb: gainDb }
      commitProject(current, { ...current, buses }, `bus-volume:${busId}`)
    },
    updateMasterVolume: (gainDb) => {
      const current = get().project
      commitProject(current, { ...current, master: { ...current.master, volumeDb: gainDb } }, 'master-volume')
    },
    toggleBusMute: (busId) => {
      const current = get().project
      const index = current.buses.findIndex((bus) => bus.id === busId)
      if (index < 0) return
      const buses = current.buses.slice()
      buses[index] = { ...buses[index]!, muted: !buses[index]!.muted }
      commitProject(current, { ...current, buses })
    },
    toggleMasterMute: () => {
      const current = get().project
      commitProject(current, { ...current, master: { ...current.master, muted: !current.master.muted } })
    },
    toggleMasterDim: () => {
      const current = get().project
      commitProject(current, { ...current, master: { ...current.master, dim: !current.master.dim } })
    },
    reorderTrack: (fromIndex, toIndex) => mutateProject((project) => {
      if (!Number.isInteger(fromIndex) || !Number.isInteger(toIndex) || fromIndex < 0 || fromIndex >= project.tracks.length) return
      const target = Math.max(0, Math.min(project.tracks.length - 1, toIndex))
      if (fromIndex === target) return
      const [track] = project.tracks.splice(fromIndex, 1)
      if (track) project.tracks.splice(target, 0, track)
    }),
    updateClip: (trackId, clipId, patch) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.clips.findIndex((clip) => clip.id === clipId)
      const track = current.tracks[trackIndex]!
      if (clipIndex < 0) {
        const midiIndex = track?.midiClips.findIndex((clip) => clip.id === clipId) ?? -1
        if (midiIndex < 0) return
        const midiClips = track.midiClips.slice()
        const source = midiClips[midiIndex]!
        let notes = source.notes
        if (patch.startSec !== undefined && patch.durationSec !== undefined && patch.startSec !== source.startSec) {
          const map = new TempoMap(current.transport.tempoMap)
          const deltaTicks = map.secondsToTicks(patch.startSec) - map.secondsToTicks(source.startSec)
          notes = source.notes.flatMap((note) => {
            const end = note.startTicks + note.lengthTicks - deltaTicks
            if (end <= 0) return []
            const startTicks = Math.max(0, note.startTicks - deltaTicks)
            return [{ ...note, startTicks, lengthTicks: Math.max(1, end - startTicks) }]
          })
        }
        midiClips[midiIndex] = { ...source, startSec: patch.startSec ?? source.startSec, durationSec: patch.durationSec ?? source.durationSec, muted: patch.muted ?? source.muted, notes }
        const tracks = current.tracks.slice()
        tracks[trackIndex] = { ...track, midiClips }
        commitProject(current, { ...current, tracks }, `midi-clip:${trackId}:${clipId}:arrange`)
        return
      }
      const clips = track.clips.slice()
      clips[clipIndex] = { ...clips[clipIndex]!, ...patch }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = { ...track, clips }
      commitProject(current, { ...current, tracks }, `clip:${trackId}:${clipId}:${Object.keys(patch).sort().join(',')}`)
    },
    upsertClipGainPoint: (trackId, clipId, point) => {
      const id = point.id ?? crypto.randomUUID()
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const track = current.tracks[trackIndex]
      const clipIndex = track?.clips.findIndex((clip) => clip.id === clipId) ?? -1
      if (!track || clipIndex < 0) return null
      const clip = track.clips[clipIndex]!
      const gainPoints = clip.gainPoints ?? []
      const pointIndex = gainPoints.findIndex((item) => item.id === id)
      const curve = point.curve ?? (pointIndex >= 0 ? gainPoints[pointIndex]!.curve : undefined)
      const next = { id, timeSec: Math.max(0, Math.min(clip.durationSec, point.timeSec)), valueDb: Math.max(-60, Math.min(12, point.valueDb)), ...(curve == null ? {} : { curve: Math.max(-1, Math.min(1, curve)) }) }
      const points = gainPoints.slice()
      if (pointIndex >= 0) points[pointIndex] = next; else points.push(next)
      points.sort((left, right) => left.timeSec - right.timeSec)
      const clips = track.clips.slice()
      clips[clipIndex] = { ...clip, gainPoints: points }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = { ...track, clips }
      commitProject(current, { ...current, tracks }, `clip-gain-point:${trackId}:${clipId}:${id}`)
      return id
    },
    removeClipGainPoint: (trackId, clipId, pointId) => {
      mutateProject((project) => { const clip = project.tracks.find((track) => track.id === trackId)?.clips.find((item) => item.id === clipId); if (clip) clip.gainPoints = (clip.gainPoints ?? []).filter((point) => point.id !== pointId) }, { historyLabel: '클립 게인 포인트 삭제' })
      set((state) => ({ selectedClipGainPoint: state.selectedClipGainPoint?.pointId === pointId ? null : state.selectedClipGainPoint }))
    },
    selectClipGainPoint: (point) => set({ selectedClipGainPoint: point, selectedAutomationPoints: [], selectedClipIds: [], editFocus: 'arrangement' }),
    moveClipToTrack: (sourceTrackId, targetTrackId, clipId) => {
      if (sourceTrackId === targetTrackId) return true
      const current = get().project
      const sourceIndex = current.tracks.findIndex((track) => track.id === sourceTrackId)
      const targetIndex = current.tracks.findIndex((track) => track.id === targetTrackId)
      if (sourceIndex < 0 || targetIndex < 0) return false
      const source = current.tracks[sourceIndex]!
      const target = current.tracks[targetIndex]!
      const audioIndex = source.clips.findIndex((clip) => clip.id === clipId)
      const midiIndex = source.midiClips.findIndex((clip) => clip.id === clipId)
      if ((audioIndex >= 0 && target.kind !== 'audio') || (midiIndex >= 0 && target.kind !== 'instrument')) return false
      if (audioIndex < 0 && midiIndex < 0) return false

      const tracks = current.tracks.slice()
      if (audioIndex >= 0) {
        const sourceClips = source.clips.slice()
        const [clip] = sourceClips.splice(audioIndex, 1)
        if (!clip) return false
        tracks[sourceIndex] = { ...source, clips: sourceClips }
        tracks[targetIndex] = { ...target, clips: [...target.clips, clip] }
      } else {
        const sourceClips = source.midiClips.slice()
        const [clip] = sourceClips.splice(midiIndex, 1)
        if (!clip) return false
        tracks[sourceIndex] = { ...source, midiClips: sourceClips }
        tracks[targetIndex] = { ...target, midiClips: [...target.midiClips, clip] }
      }
      commitProject(current, { ...current, tracks }, `clip-track:${clipId}`)
      set((state) => ({
        selectedTrackId: targetTrackId,
        selectedTrackIds: [targetTrackId],
        editorClip: state.editorClip?.clipId === clipId ? { trackId: targetTrackId, clipId } : state.editorClip,
      }))
      return true
    },
    splitClip: (trackId, clipId, atSec) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      const source = track?.clips.find((candidate) => candidate.id === clipId)
      if (!track) return
      if (!source) {
        const midi = track.midiClips.find((candidate) => candidate.id === clipId)
        if (!midi || atSec <= midi.startSec + 0.05 || atSec >= midi.startSec + midi.durationSec - 0.05) return
        const leftDuration = atSec - midi.startSec
        const rightDuration = midi.startSec + midi.durationSec - atSec
        const map = new TempoMap(project.transport.tempoMap)
        const midiStartTick = map.secondsToTicks(midi.startSec)
        const splitTicks = map.secondsToTicks(atSec) - midiStartTick
        const originalEnd = map.secondsToTicks(midi.startSec + midi.durationSec) - midiStartTick
        const rightNotes = midi.notes.flatMap((note) => {
          const end = note.startTicks + note.lengthTicks
          if (end <= splitTicks) return []
          const startTicks = Math.max(0, note.startTicks - splitTicks)
          return [{ ...note, id: crypto.randomUUID(), startTicks, lengthTicks: Math.max(1, Math.min(end, originalEnd) - splitTicks - startTicks) }]
        })
        midi.notes = midi.notes.flatMap((note) => note.startTicks >= splitTicks ? [] : [{ ...note, lengthTicks: Math.max(1, Math.min(note.lengthTicks, splitTicks - note.startTicks)) }])
        midi.durationSec = leftDuration
        track.midiClips.push({ ...midi, id: crypto.randomUUID(), name: `${midi.name} B`, startSec: atSec, durationSec: Math.max(.05, rightDuration), notes: rightNotes })
        return
      }
      if (atSec <= source.startSec + 0.05 || atSec >= source.startSec + source.durationSec - 0.05) return
      const leftDuration = atSec - source.startSec
      const rightDuration = source.durationSec - leftDuration
      const sourceStep = clipSourceStep(source, project.transport.bpm, project.transport.tempoMap)
      const right = { ...source, id: crypto.randomUUID(), startSec: atSec, offsetSec: source.reversed ? source.offsetSec : source.offsetSec + leftDuration * sourceStep, durationSec: rightDuration, fadeInSec: 0, gainPoints: (source.gainPoints ?? []).filter((point) => point.timeSec > leftDuration).map((point) => ({ ...point, id: crypto.randomUUID(), timeSec: point.timeSec - leftDuration })) }
      if (source.reversed) source.offsetSec += rightDuration * sourceStep
      source.gainPoints = (source.gainPoints ?? []).filter((point) => point.timeSec <= leftDuration)
      source.durationSec = leftDuration
      source.fadeOutSec = 0
      track.clips.push(right)
    }),
    deleteSelectedClips: () => {
      const selected = new Set(get().selectedClipIds)
      mutateProject((project) => { for (const track of project.tracks) { track.clips = track.clips.filter((clip) => !selected.has(clip.id)); track.midiClips = track.midiClips.filter((clip) => !selected.has(clip.id)) } collectUnusedAudioSources(project) }, { copyAssets: true })
      set({ selectedClipIds: [] })
    },
    duplicateSelectedClips: () => {
      const selected = new Set(get().selectedClipIds)
      const nextIds: string[] = []
      mutateProject((project) => {
        for (const track of project.tracks) {
          const copies = track.clips.filter((clip) => selected.has(clip.id)).map((clip) => {
            const id = crypto.randomUUID(); nextIds.push(id); return { ...clip, id, startSec: clip.startSec + clip.durationSec }
          })
          track.clips.push(...copies)
          const midiCopies = track.midiClips.filter((clip) => selected.has(clip.id)).map((clip) => {
            const id = crypto.randomUUID(); nextIds.push(id); return { ...clip, id, startSec: clip.startSec + clip.durationSec, notes: clip.notes.map((note) => ({ ...note, id: crypto.randomUUID() })), ccLanes: clip.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) }
          })
          track.midiClips.push(...midiCopies)
        }
      })
      set({ selectedClipIds: nextIds })
    },
    duplicateSelectedClipsSmart: () => {
      const state = get()
      const selected = new Set(state.selectedClipIds)
      const clips = state.project.tracks.flatMap((track) => [...track.clips, ...track.midiClips]).filter((clip) => selected.has(clip.id))
      if (!clips.length) return
      const startSec = Math.min(...clips.map((clip) => clip.startSec))
      const endSec = Math.max(...clips.map((clip) => clip.startSec + clip.durationSec))
      const tempoMap = new TempoMap(state.project.transport.tempoMap)
      const startTick = tempoMap.secondsToTicks(startSec)
      const extentTicks = Math.max(1, tempoMap.secondsToTicks(endSec) - startTick)
      const deltaTicks = studioOneDuplicateStepTicks(extentTicks, state.gridTicks, state.snapEnabled)
      const nextIds: string[] = []
      // This command used to clone the entire project (assets, effects and all
      // tracks) before adding a handful of clips. Preserve unaffected track
      // references and only copy arrays that actually receive duplicates.
      const tracks = state.project.tracks.map((track) => {
        const audioCopies = track.clips.filter((clip) => selected.has(clip.id)).map((clip) => { const id = crypto.randomUUID(); nextIds.push(id); return { ...clip, id, startSec: tempoMap.ticksToSeconds(tempoMap.secondsToTicks(clip.startSec) + deltaTicks), gainPoints: clip.gainPoints?.map((point) => ({ ...point, id: crypto.randomUUID() })) } })
        const midiCopies = track.midiClips.filter((clip) => selected.has(clip.id)).map((clip) => { const id = crypto.randomUUID(); nextIds.push(id); return { ...clip, id, startSec: tempoMap.ticksToSeconds(tempoMap.secondsToTicks(clip.startSec) + deltaTicks), notes: clip.notes.map((note) => ({ ...note, id: crypto.randomUUID() })), ccLanes: clip.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) } })
        if (!audioCopies.length && !midiCopies.length) return track
        return {
          ...track,
          clips: audioCopies.length ? [...track.clips, ...audioCopies] : track.clips,
          midiClips: midiCopies.length ? [...track.midiClips, ...midiCopies] : track.midiClips,
        }
      })
      commitProject(state.project, { ...state.project, tracks }, '', '클립 복제')
      set({ selectedClipIds: nextIds })
    },
    copySelectedClips: () => {
      const state = get()
      const selected = new Set(state.selectedClipIds)
      const entries: ClipboardEntry[] = []
      let originSec = Number.POSITIVE_INFINITY
      let anchorTrack = Number.POSITIVE_INFINITY
      for (const [index, track] of state.project.tracks.entries()) {
        for (const clip of track.clips) if (selected.has(clip.id)) { entries.push({ trackOffset: index, kind: 'audio', clip: { ...clip } }); originSec = Math.min(originSec, clip.startSec); anchorTrack = Math.min(anchorTrack, index) }
        for (const clip of track.midiClips) if (selected.has(clip.id)) { entries.push({ trackOffset: index, kind: 'midi', clip: { ...clip, notes: clip.notes.map((note) => ({ ...note })), ccLanes: clip.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) } }); originSec = Math.min(originSec, clip.startSec); anchorTrack = Math.min(anchorTrack, index) }
      }
      if (!entries.length) return
      // Offsets are stored relative to the earliest clip so a paste keeps the
      // original spacing across both time and tracks.
      set({ clipboard: { originSec, entries: entries.map((entry) => ({ ...entry, trackOffset: entry.trackOffset - anchorTrack })) } })
      get().showToast(`${entries.length}개 클립을 복사했습니다`)
    },
    cutSelectedClips: () => { get().copySelectedClips(); if (get().clipboard) get().deleteSelectedClips() },
    pasteClipboard: (atSec) => {
      const state = get()
      const clipboard = state.clipboard
      if (!clipboard?.entries.length) return
      const startSec = Math.max(0, atSec ?? state.playheadSec)
      const baseTrack = Math.max(0, state.project.tracks.findIndex((track) => track.id === state.selectedTrackId))
      const nextIds: string[] = []
      mutateProject((project) => {
        for (const entry of clipboard.entries) {
          const track = project.tracks[Math.min(project.tracks.length - 1, baseTrack + entry.trackOffset)]
          if (!track) continue
          const id = crypto.randomUUID()
          const shifted = startSec + (entry.clip.startSec - clipboard.originSec)
          if (entry.kind === 'audio') {
            if (track.kind !== 'audio') continue
            track.clips.push({ ...entry.clip, id, startSec: shifted })
          } else {
            if (track.kind !== 'instrument') continue
            track.midiClips.push({ ...entry.clip, id, startSec: shifted, notes: entry.clip.notes.map((note) => ({ ...note, id: crypto.randomUUID() })), ccLanes: entry.clip.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) })
          }
          nextIds.push(id)
        }
      })
      if (nextIds.length) set({ selectedClipIds: nextIds })
      else get().showToast('붙여넣을 수 있는 트랙 종류가 아닙니다')
    },
    toggleSelectedClipsMuted: () => {
      const selected = new Set(get().selectedClipIds)
      if (!selected.size) return
      mutateProject((project) => {
        for (const track of project.tracks) {
          for (const clip of track.clips) if (selected.has(clip.id)) clip.muted = !clip.muted
          for (const clip of track.midiClips) if (selected.has(clip.id)) clip.muted = !clip.muted
        }
      })
    },
    updateClipGain: (trackId, clipId, gainDb) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.clips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      const track = current.tracks[trackIndex]!
      const clips = track.clips.slice()
      const source = clips[clipIndex]!
      const clipped = Math.max(-60, Math.min(12, gainDb))
      const delta = clipped - source.gainDb
      clips[clipIndex] = { ...source, gainDb: clipped, gainPoints: source.gainPoints?.map((point) => ({ ...point, valueDb: Math.max(-60, Math.min(12, point.valueDb + delta)) })) }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = { ...track, clips }
      commitProject(current, { ...current, tracks }, `clip-gain:${trackId}:${clipId}`)
    },
    duplicateClip: (trackId, clipId) => {
      const id = crypto.randomUUID()
      const current = get().project
      const trackIndex = current.tracks.findIndex((candidate) => candidate.id === trackId)
      if (trackIndex < 0) return null
      const track = current.tracks[trackIndex]!
      const clip = track.clips.find((candidate) => candidate.id === clipId)
      const midi = track.midiClips.find((candidate) => candidate.id === clipId)
      if (!clip && !midi) return null
      const tracks = current.tracks.slice()
      tracks[trackIndex] = clip
        ? { ...track, clips: [...track.clips, { ...clip, id, gainPoints: clip.gainPoints?.map((point) => ({ ...point, id: crypto.randomUUID() })) }] }
        : { ...track, midiClips: [...track.midiClips, { ...midi!, id, notes: midi!.notes.map((note) => ({ ...note, id: crypto.randomUUID() })), ccLanes: midi!.ccLanes.map((lane) => ({ ...lane, points: lane.points.map((point) => ({ ...point })) })) }] }
      commitProject(current, { ...current, tracks }, '', '클립 복제')
      set({ selectedClipIds: [id] })
      return id
    },
    makeAudioSourceUnique: (trackId, clipId) => mutateProject((project) => {
      const clip = project.tracks.find((track) => track.id === trackId)?.clips.find((candidate) => candidate.id === clipId)
      if (!clip) return
      const source = project.audioSourceRefs[clip.audioSourceRefId]
      if (!source) return
      const id = `source-${crypto.randomUUID()}`
      project.audioSourceRefs[id] = { ...source, id, modificationId: null }
      clip.audioSourceRefId = id
    }, { historyLabel: '독립된 오디오 소스 만들기' }),
    addSilentClip: (trackId, startSec, durationSec) => mutateProject((project) => {
      const sourceId = `source-${crypto.randomUUID()}`
      project.audioSourceRefs[sourceId] = { id: sourceId, assetId: '', name: 'Empty clip', modificationId: null }
      project.tracks.find((track) => track.id === trackId)?.clips.push({ id: crypto.randomUUID(), audioSourceRefId: sourceId, name: 'Empty clip', startSec, offsetSec: 0, durationSec, gainDb: 0, fadeInSec: 0, fadeOutSec: 0 })
    }),
    addAssetAsTrack: (asset) => {
      const playheadSec = get().playheadSec
      mutateProject((project) => {
      const source = createAudioSourceRef(project, asset)
      const id = crypto.randomUUID()
      project.tracks.push({ id, kind: 'audio', name: asset.name.replace(/\.[^.]+$/, ''), color: '#43c6ac', clips: [{ id: crypto.randomUUID(), audioSourceRefId: source.id, name: asset.name, startSec: playheadSec, offsetSec: 0, durationSec: asset.durationSec, gainDb: 0, fadeInSec: 0, fadeOutSec: 0 }], midiClips: [], instrument: null, volumeDb: 0, pan: 0, muted: false, solo: false, armed: false, effects: [], sends: [], height: get().trackHeight })
      queueMicrotask(() => set({ selectedTrackId: id, selectedTrackIds: [id] }))
      }, { copyAssets: true })
    },
    insertAudioAsset: (asset, startSec, targetTrackId, insertIndex) => {
      let selectedId = targetTrackId ?? ''
      mutateProject((project) => {
        const source = createAudioSourceRef(project, asset)
        const clip: Clip = { id: crypto.randomUUID(), audioSourceRefId: source.id, name: asset.name, startSec: Math.max(0, startSec), offsetSec: 0, durationSec: asset.durationSec, gainDb: 0, fadeInSec: 0, fadeOutSec: 0, playbackRate: 1, pitchSemitones: 0, fineCents: 0, reversed: false }
        const target = targetTrackId ? project.tracks.find((track) => track.id === targetTrackId && track.kind === 'audio') : undefined
        if (target) { target.clips.push(clip); selectedId = target.id; return }
        selectedId = crypto.randomUUID()
        const track: Track = { id: selectedId, kind: 'audio', name: asset.name.replace(/\.[^.]+$/, ''), color: '#43c6ac', clips: [clip], midiClips: [], instrument: null, volumeDb: 0, pan: 0, muted: false, solo: false, armed: false, effects: [], sends: project.buses.map((bus) => ({ id: crypto.randomUUID(), targetBusId: bus.id, gainDb: -60, preFader: false })), outputBusId: null, automationOpen: false, automationLanes: [], height: get().trackHeight }
        project.tracks.splice(Math.max(0, Math.min(project.tracks.length, insertIndex ?? project.tracks.length)), 0, track)
      }, { copyAssets: true, historyLabel: '오디오 파일 삽입' })
      queueMicrotask(() => set({ selectedTrackId: selectedId, selectedTrackIds: [selectedId] }))
    },
    addMidiClip: (trackId, startSec, durationSec) => {
      const id = crypto.randomUUID()
      let created = false
      mutateProject((project) => {
        const track = project.tracks.find((candidate) => candidate.id === trackId && candidate.kind === 'instrument')
        if (!track) return
        const clippedDuration = Math.max(0.25, durationSec)
        const map = new TempoMap(project.transport.tempoMap)
        const loopLengthTicks = Math.max(1, map.secondsToTicks(Math.max(0, startSec) + clippedDuration) - map.secondsToTicks(Math.max(0, startSec)))
        track.midiClips.push({ id, name: 'MIDI Clip', startSec: Math.max(0, startSec), durationSec: clippedDuration, loopEnabled: false, loopStartTicks: 0, loopLengthTicks, notes: [], ccLanes: [], transposeSemitones: 0, velocityScale: 1, muted: false, color: null })
        created = true
      })
      if (created) queueMicrotask(() => get().openMidiEditor(trackId, id))
      return created ? id : null
    },
    openMidiEditor: (trackId, clipId) => set({ editorClip: { trackId, clipId }, selectedTrackId: trackId, selectedTrackIds: [trackId], selectedNoteIds: [], pianoRollOpen: true }),
    selectMidiNote: (noteId, additive = false) => set((state) => ({ selectedNoteIds: additive ? [...new Set([...state.selectedNoteIds, noteId])] : [noteId], editFocus: 'pianoRoll' })),
    clearNoteSelection: () => set({ selectedNoteIds: [] }),
    addMidiNote: (trackId, clipId, note) => {
      const id = crypto.randomUUID()
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return null
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      const notes = [...source.notes, { ...note, id, pitch: Math.max(0, Math.min(127, Math.round(note.pitch))), velocity: Math.max(1, Math.min(127, Math.round(note.velocity))), startTicks: Math.max(0, Math.round(note.startTicks)), lengthTicks: Math.max(1, Math.round(note.lengthTicks)) }].sort((a, b) => a.startTicks - b.startTicks || a.pitch - b.pitch)
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, notes })
      set({ selectedNoteIds: [id] })
      return id
    },
    updateMidiNotes: (trackId, clipId, noteIds, patch) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      const selected = new Set(noteIds)
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      const notes = source.notes.map((note) => selected.has(note.id) ? { ...note, ...patch, pitch: Math.max(0, Math.min(127, Math.round(patch.pitch ?? note.pitch))), velocity: Math.max(1, Math.min(127, Math.round(patch.velocity ?? note.velocity))), startTicks: Math.max(0, Math.round(patch.startTicks ?? note.startTicks)), lengthTicks: Math.max(1, Math.round(patch.lengthTicks ?? note.lengthTicks)) } : note).sort((a, b) => a.startTicks - b.startTicks || a.pitch - b.pitch)
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, notes }, `midi-notes:${trackId}:${clipId}:${Object.keys(patch).sort().join(',')}`)
    },
    updateMidiNoteBatch: (trackId, clipId, updates) => {
      if (!updates.length) return
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      const byId = new Map(updates.map((update) => [update.id, update.patch]))
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      const notes = source.notes.map((note) => {
        const patch = byId.get(note.id)
        return patch ? { ...note, ...patch, pitch: Math.max(0, Math.min(127, Math.round(patch.pitch ?? note.pitch))), velocity: Math.max(1, Math.min(127, Math.round(patch.velocity ?? note.velocity))), startTicks: Math.max(0, Math.round(patch.startTicks ?? note.startTicks)), lengthTicks: Math.max(1, Math.round(patch.lengthTicks ?? note.lengthTicks)) } : note
      }).sort((a, b) => a.startTicks - b.startTicks || a.pitch - b.pitch)
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, notes }, `midi-batch:${trackId}:${clipId}`)
    },
    upsertMidiControlPoints: (trackId, clipId, cc, points) => {
      if (!points.length || !Number.isInteger(cc) || cc < MIDI_PITCH_BEND_LANE || cc > 127) return
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      const maximum = cc === MIDI_PITCH_BEND_LANE ? 8191 : 127
      const minimum = cc === MIDI_PITCH_BEND_LANE ? -8192 : 0
      const merged = new Map((source.ccLanes.find((lane) => lane.cc === cc)?.points ?? []).map((point) => [point.ticks, point]))
      for (const point of points) {
        const ticks = Math.max(0, Math.round(point.ticks))
        merged.set(ticks, { ticks, value: Math.max(minimum, Math.min(maximum, Math.round(point.value))) })
      }
      const lane = { cc, points: [...merged.values()].sort((left, right) => left.ticks - right.ticks) }
      const ccLanes = source.ccLanes.some((candidate) => candidate.cc === cc)
        ? source.ccLanes.map((candidate) => candidate.cc === cc ? lane : candidate)
        : [...source.ccLanes, lane]
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, ccLanes }, `midi-controller:${trackId}:${clipId}:${cc}`)
    },
    removeMidiControlPoint: (trackId, clipId, cc, ticks) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      const ccLanes = source.ccLanes
        .map((lane) => lane.cc === cc ? { ...lane, points: lane.points.filter((point) => point.ticks !== ticks) } : lane)
        .filter((lane) => lane.points.length)
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, ccLanes }, `midi-controller:${trackId}:${clipId}:${cc}`)
    },
    duplicateMidiNotes: (trackId, clipId, noteIds, deltaTicks = 0, deltaPitch = 0) => {
      if (!noteIds.length) return []
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return []
      const selected = new Set(noteIds)
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      const copies = source.notes.filter((note) => selected.has(note.id)).map((note) => ({
        ...note,
        id: crypto.randomUUID(),
        startTicks: Math.max(0, Math.round(note.startTicks + deltaTicks)),
        pitch: Math.max(0, Math.min(127, Math.round(note.pitch + deltaPitch))),
      }))
      if (!copies.length) return []
      const notes = [...source.notes, ...copies].sort((a, b) => a.startTicks - b.startTicks || a.pitch - b.pitch)
      const copiedEndTicks = Math.max(...copies.map((note) => note.startTicks + note.lengthTicks))
      const map = new TempoMap(current.transport.tempoMap)
      const clipStartTick = map.secondsToTicks(source.startSec)
      const requiredDurationSec = map.ticksToSeconds(clipStartTick + copiedEndTicks) - source.startSec
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, notes, loopLengthTicks: Math.max(source.loopLengthTicks, copiedEndTicks), durationSec: Math.max(source.durationSec, requiredDurationSec) })
      const ids = copies.map((note) => note.id)
      set({ selectedNoteIds: ids })
      return ids
    },
    duplicateSelectedMidiNotes: () => {
      const state = get()
      const editor = state.editorClip
      if (!editor || !state.selectedNoteIds.length) return
      const clip = state.project.tracks.find((track) => track.id === editor.trackId)?.midiClips.find((item) => item.id === editor.clipId)
      if (!clip) return
      const selected = new Set(state.selectedNoteIds)
      const notes = clip.notes.filter((note) => selected.has(note.id))
      const noteStart = Math.min(...notes.map((note) => note.startTicks))
      const noteEnd = Math.max(...notes.map((note) => note.startTicks + note.lengthTicks))
      const deltaTicks = studioOneDuplicateStepTicks(Math.max(1, noteEnd - noteStart), state.pianoGridTicks, state.pianoSnapEnabled)
      if (deltaTicks > 0) get().duplicateMidiNotes(editor.trackId, editor.clipId, state.selectedNoteIds, deltaTicks, 0)
    },
    deleteMidiNotes: (trackId, clipId, noteIds) => {
      const selected = new Set(noteIds)
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      const source = current.tracks[trackIndex]!.midiClips[clipIndex]!
      replaceMidiClip(current, trackIndex, clipIndex, { ...source, notes: source.notes.filter((note) => !selected.has(note.id)) })
      set({ selectedNoteIds: [] })
    },
    updateMidiClip: (trackId, clipId, patch) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const clipIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.midiClips.findIndex((clip) => clip.id === clipId)
      if (clipIndex < 0) return
      replaceMidiClip(current, trackIndex, clipIndex, { ...current.tracks[trackIndex]!.midiClips[clipIndex]!, ...patch }, `midi-clip:${trackId}:${clipId}:${Object.keys(patch).sort().join(',')}`)
    },
    toggleEditorMaximized: () => set((state) => ({ editorMaximized: !state.editorMaximized, pianoRollOpen: true })),
    togglePianoRoll: () => set((state) => ({ pianoRollOpen: !state.pianoRollOpen, editorMaximized: state.pianoRollOpen ? false : state.editorMaximized })),
    setPianoRollHeight: (value) => set({ pianoRollHeight: Math.max(180, Math.min(720, value)), pianoRollOpen: true }),
    toggleInspector: () => set((state) => ({ inspectorVisible: !state.inspectorVisible })),
    toggleBrowser: () => set((state) => ({ browserVisible: !state.browserVisible })),
    setBrowserDock: (dock) => set({ browserDock: dock, browserVisible: true }),
    setRecordingEnabled: (enabled) => set({ recordingEnabled: enabled }),
    updateEffect: (trackId, effectId, params) => {
      const current = get().project
      const trackIndex = current.tracks.findIndex((track) => track.id === trackId)
      const effectIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.effects.findIndex((effect) => effect.id === effectId)
      if (effectIndex < 0) return
      const track = current.tracks[trackIndex]!
      const effects = track.effects.slice()
      effects[effectIndex] = { ...effects[effectIndex]!, params: { ...effects[effectIndex]!.params, ...params } }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = Object.entries(params).reduce<Track>((updated, [parameterId, value]) => recordArmedAutomation(updated, get().playheadSec, 'effect', effectId, parameterId, value, current.transport.isPlaying), { ...track, effects })
      commitProject(current, { ...current, tracks }, `effect:${trackId}:${effectId}:${Object.keys(params).sort().join(',')}`)
    },
    toggleEffectBypass: (trackId, effectId) => mutateProject((project) => {
      const effect = project.tracks.find((track) => track.id === trackId)?.effects.find((item) => item.id === effectId)
      if (effect) effect.bypassed = !effect.bypassed
    }),
    addEffect: (trackId, type, plugin) => {
      const id = crypto.randomUUID()
      mutateProject((project) => {
        project.tracks.find((track) => track.id === trackId)?.effects.push({ id, type, bypassed: false, params: plugin ? Object.fromEntries((plugin.parameters ?? []).map((parameter) => [parameter.id, parameter.defaultValue])) : effectDefaults(type), plugin })
      })
      set({ focusedEffectId: id, rackTarget: { kind: 'track', id: trackId }, lowerTab: 'effects', lowerPanelCollapsed: false })
    },
    removeEffect: (trackId, effectId) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (track) {
        track.effects = track.effects.filter((effect) => effect.id !== effectId)
        track.automationLanes = (track.automationLanes ?? []).filter((lane) => !(lane.targetKind === 'effect' && lane.targetId === effectId))
      }
    }),
    reorderEffect: (trackId, fromIndex, toIndex) => mutateProject((project) => {
      const effects = project.tracks.find((track) => track.id === trackId)?.effects
      if (!effects) return
      const [effect] = effects.splice(fromIndex, 1)
      if (effect) effects.splice(toIndex, 0, effect)
    }),
    updateTargetEffect: (target, effectId, params) => {
      const current = get().project
      const key = `effect:${target.kind}:${target.id}:${effectId}:${Object.keys(params).sort().join(',')}`
      if (target.kind === 'master') {
        const index = current.master.effects.findIndex((effect) => effect.id === effectId)
        if (index < 0) return
        const effects = current.master.effects.slice()
        effects[index] = { ...effects[index]!, params: { ...effects[index]!.params, ...params } }
        commitProject(current, { ...current, master: { ...current.master, effects } }, key)
        return
      }
      if (target.kind === 'bus') {
        const busIndex = current.buses.findIndex((bus) => bus.id === target.id)
        const effectIndex = busIndex < 0 ? -1 : current.buses[busIndex]!.effects.findIndex((effect) => effect.id === effectId)
        if (effectIndex < 0) return
        const bus = current.buses[busIndex]!
        const effects = bus.effects.slice()
        effects[effectIndex] = { ...effects[effectIndex]!, params: { ...effects[effectIndex]!.params, ...params } }
        const buses = current.buses.slice()
        buses[busIndex] = { ...bus, effects }
        commitProject(current, { ...current, buses }, key)
        return
      }
      const trackIndex = current.tracks.findIndex((track) => track.id === target.id)
      const effectIndex = trackIndex < 0 ? -1 : current.tracks[trackIndex]!.effects.findIndex((effect) => effect.id === effectId)
      if (effectIndex < 0) return
      const track = current.tracks[trackIndex]!
      const effects = track.effects.slice()
      effects[effectIndex] = { ...effects[effectIndex]!, params: { ...effects[effectIndex]!.params, ...params } }
      const tracks = current.tracks.slice()
      tracks[trackIndex] = Object.entries(params).reduce<Track>((updated, [parameterId, value]) => recordArmedAutomation(updated, get().playheadSec, 'effect', effectId, parameterId, value, current.transport.isPlaying), { ...track, effects })
      commitProject(current, { ...current, tracks }, key)
    },
    updateInstrument: (trackId, params) => {
      const current = get().project
      const index = current.tracks.findIndex((track) => track.id === trackId)
      const source = index < 0 ? null : current.tracks[index]!
      if (!source?.instrument) return
      const tracks = current.tracks.slice()
      tracks[index] = Object.entries(params).reduce<Track>((updated, [parameterId, value]) => recordArmedAutomation(updated, get().playheadSec, 'instrument', trackId, parameterId, value, current.transport.isPlaying), { ...source, instrument: { ...source.instrument, params: { ...source.instrument.params, ...params } } })
      commitProject(current, { ...current, tracks }, `instrument:${trackId}:${Object.keys(params).sort().join(',')}`)
    },
    toggleInstrumentBypass: (trackId) => mutateProject((project) => {
      const instrument = project.tracks.find((track) => track.id === trackId)?.instrument
      if (instrument) instrument.bypassed = !instrument.bypassed
    }),
    setTrackInstrumentPlugin: (trackId, plugin) => mutateProject((project) => {
      const track = project.tracks.find((candidate) => candidate.id === trackId)
      if (!track || track.kind !== 'instrument') return
      track.instrument = createInstrumentInstance(plugin)
      track.automationLanes = (track.automationLanes ?? []).filter((lane) => lane.targetKind !== 'instrument')
    }),
    toggleTargetEffect: (target, effectId) => mutateProject((project) => {
      const effect = targetEffects(project, target)?.find((item) => item.id === effectId)
      if (effect) effect.bypassed = !effect.bypassed
    }),
    addTargetEffect: (target, type, plugin) => {
      const id = crypto.randomUUID()
      mutateProject((project) => {
        targetEffects(project, target)?.push({ id, type, bypassed: false, params: plugin ? Object.fromEntries((plugin.parameters ?? []).map((parameter) => [parameter.id, parameter.defaultValue])) : effectDefaults(type), plugin })
      })
      set({ focusedEffectId: id, rackTarget: target, lowerTab: 'effects', lowerPanelCollapsed: false })
    },
    duplicateTargetEffect: (target, effectId) => {
      const id = crypto.randomUUID()
      let duplicated = false
      mutateProject((project) => {
        const effects = targetEffects(project, target)
        const index = effects?.findIndex((effect) => effect.id === effectId) ?? -1
        if (!effects || index < 0) return
        const source = effects[index]!
        effects.splice(index + 1, 0, {
          ...source,
          id,
          params: { ...source.params },
          plugin: source.plugin ? { ...source.plugin, parameters: source.plugin.parameters?.map((parameter) => ({ ...parameter })), state: source.plugin.state?.slice() } : undefined,
          sidechain: source.sidechain ? { ...source.sidechain } : undefined,
        })
        duplicated = true
      })
      if (duplicated) set({ focusedEffectId: id, rackTarget: target, lowerTab: 'effects', lowerPanelCollapsed: false })
    },
    clearFocusedEffect: () => set({ focusedEffectId: null }),
    removeTargetEffect: (target, effectId) => mutateProject((project) => {
      const effects = targetEffects(project, target)
      if (!effects) return
      const index = effects.findIndex((effect) => effect.id === effectId)
      if (index >= 0) effects.splice(index, 1)
      if (target.kind === 'track') {
        const track = project.tracks.find((candidate) => candidate.id === target.id)
        if (track) track.automationLanes = (track.automationLanes ?? []).filter((lane) => !(lane.targetKind === 'effect' && lane.targetId === effectId))
      }
    }),
    reorderTargetEffect: (target, fromIndex, toIndex) => mutateProject((project) => {
      const effects = targetEffects(project, target)
      if (!effects || !Number.isInteger(fromIndex) || !Number.isInteger(toIndex) || fromIndex < 0 || fromIndex >= effects.length) return
      const targetIndex = Math.max(0, Math.min(effects.length - 1, toIndex))
      if (fromIndex === targetIndex) return
      const [effect] = effects.splice(fromIndex, 1)
      if (effect) effects.splice(targetIndex, 0, effect)
    }),
    setTargetEffectSidechain: (target, effectId, enabled, sourceId) => {
      const project = get().project
      if (enabled && sourceId && createsSidechainCycle(project, rackNodeKey(target), sidechainSourceKey(sourceId))) {
        get().showToast('사이드체인 순환 참조는 연결할 수 없습니다.')
        return
      }
      mutateProject((draft) => {
        const effect = targetEffects(draft, target)?.find((item) => item.id === effectId)
        if (effect) effect.sidechain = { enabled, sourceTrackId: sourceId }
      })
    },
    setBpm: (bpm) => { if (Number.isFinite(bpm)) mutateProject((project) => { const value = Math.round(Math.max(20, Math.min(300, bpm)) * 100) / 100; project.transport.bpm = value; project.transport.tempoMap.tempoPoints[0] = { ...(project.transport.tempoMap.tempoPoints[0] ?? { tick: 0, curve: 'jump' as const }), tick: 0, bpm: value } }, { historyKey: 'transport:bpm' }) },
    setTimeSignature: (signature) => mutateProject((project) => {
      project.transport.timeSignature = { numerator: Math.max(1, Math.min(32, Math.round(signature.numerator))), denominator: [1, 2, 4, 8, 16].includes(signature.denominator) ? signature.denominator : 4 }
      project.transport.tempoMap.timeSignatures[0] = { bar: 1, ...project.transport.timeSignature }
    }),
    toggleLoop: () => mutateProject((project) => { project.transport.loop.enabled = !project.transport.loop.enabled }),
    setLoopRange: (startSec, endSec) => mutateProject((project) => {
      const start = Math.max(0, Math.min(startSec, endSec))
      const end = Math.max(start + 0.05, Math.max(startSec, endSec))
      project.transport.loop = { enabled: true, startSec: start, endSec: end }
    }, { historyKey: 'transport:loop' }),
    setLoopToSelection: () => {
      const state = get()
      const selected = new Set(state.selectedClipIds)
      const spans = state.project.tracks.flatMap((track) => [...track.clips, ...track.midiClips].filter((clip) => selected.has(clip.id)))
      if (!spans.length) { get().showToast('먼저 클립을 선택하세요'); return }
      get().setLoopRange(Math.min(...spans.map((clip) => clip.startSec)), Math.max(...spans.map((clip) => clip.startSec + clip.durationSec)))
    },
    setZoom: (value) => set({ pixelsPerSecond: Math.max(4, Math.min(400, value)) }),
    setPianoRollZoom: (value) => set({ pianoRollZoom: Math.max(32, Math.min(160, value)) }),
    setTrackHeight: (value) => set({ trackHeight: Math.max(46, Math.min(200, value)) }),
    setTrackViewHeight: (trackId, value) => {
      const current = get().project
      const index = current.tracks.findIndex((track) => track.id === trackId)
      if (index < 0) return
      const tracks = current.tracks.slice()
      tracks[index] = { ...tracks[index]!, height: Math.max(46, Math.min(240, value)) }
      commitProject(current, { ...current, tracks }, `track-height:${trackId}`)
    },
    resizeSelectedTracks: (delta) => {
      const selected = new Set(get().selectedTrackIds)
      if (!selected.size) return
      const state = get()
      const tracks = state.project.tracks.map((track) => selected.has(track.id) ? { ...track, height: Math.max(46, Math.min(240, (track.height ?? state.trackHeight) + delta)) } : track)
      commitProject(state.project, { ...state.project, tracks }, `track-heights:${[...selected].sort().join(',')}`)
    },
    resizeAllTracks: (delta) => {
      const state = get()
      const nextDefault = Math.max(46, Math.min(200, state.trackHeight + delta))
      const tracks = state.project.tracks.map((track) => ({ ...track, height: Math.max(46, Math.min(240, (track.height ?? state.trackHeight) + delta)) }))
      commitProject(state.project, { ...state.project, tracks }, 'track-heights:all')
      set({ trackHeight: nextDefault })
    },
    toggleSnap: () => set((state) => ({ snapEnabled: !state.snapEnabled })),
    togglePianoSnap: () => set((state) => ({ pianoSnapEnabled: !state.pianoSnapEnabled })),
    setGridTicks: (ticks) => set({ gridTicks: Math.max(1, Math.round(ticks)) }),
    setPianoGridTicks: (ticks) => set({ pianoGridTicks: Math.max(1, Math.round(ticks)) }),
    setArrangementSwing: (value) => set({ arrangementSwing: Math.max(0, Math.min(100, value)) }),
    setPianoSwing: (value) => set({ pianoSwing: Math.max(0, Math.min(100, value)) }),
    toggleFollowPlayhead: () => set((state) => ({ followPlayhead: !state.followPlayhead })),
    setLowerPanelHeight: (value) => set((state) => {
      const height = state.lowerTab === 'effects'
        ? Math.max(30, Math.min(350, value))
        : Math.max(120, value)
      return state.lowerTab === 'effects'
        ? { lowerPanelHeight: height, effectsPanelHeight: height, lowerPanelCollapsed: false }
        : { lowerPanelHeight: height, mixerPanelHeight: height, lowerPanelCollapsed: false }
    }),
    toggleLowerPanel: () => set((state) => ({ lowerPanelCollapsed: !state.lowerPanelCollapsed })),
    setLowerTab: (tab) => set((state) => ({
      lowerTab: tab,
      lowerPanelHeight: (tab === 'effects' ? state.effectsPanelHeight : state.mixerPanelHeight) ?? state.lowerPanelHeight,
      lowerPanelCollapsed: false,
      rackTarget: tab === 'effects' && state.selectedTrackId && state.project.tracks.some((track) => track.id === state.selectedTrackId)
        ? { kind: 'track' as const, id: state.selectedTrackId }
        : state.rackTarget,
    })),
    setShortcutsOpen: (open) => set({ shortcutsOpen: open }),
    // Toasts self-expire. Without this every showToast caller had to remember to
    // clear it, and the ones that forgot pinned a message on screen forever.
    showToast: (message) => {
      clearTimeout(toastTimer)
      toastTimer = setTimeout(() => set({ toast: null }), 3600) as unknown as number
      set({ toast: message })
    },
    clearToast: () => { clearTimeout(toastTimer); set({ toast: null }) },
    undo: () => {
      const state = get(); const previous = state.past.at(-1); if (!previous) return
      lastHistoryKey = ''
      const entry = state.history.at(-1)
      set({ project: previous, playheadSec: previous.transport.playheadSec, past: state.past.slice(0, -1), future: [state.project, ...state.future].slice(0, 50), history: state.history.slice(0, -1), futureHistory: entry ? [entry, ...state.futureHistory].slice(0, 50) : state.futureHistory })
    },
    redo: () => {
      const state = get(); const next = state.future[0]; if (!next) return
      lastHistoryKey = ''
      const entry = state.futureHistory[0] ?? { id: crypto.randomUUID(), label: '다시 실행', timestamp: Date.now() }
      set({ project: next, playheadSec: next.transport.playheadSec, past: [...state.past, state.project].slice(-50), future: state.future.slice(1), history: [...state.history, entry].slice(-50), futureHistory: state.futureHistory.slice(1) })
    },
  }
})

function createInstrumentInstance(plugin?: ExternalPluginRef): Track['instrument'] {
  if (plugin) return {
    id: crypto.randomUUID(),
    type: `${plugin.format}:${plugin.uid}`,
    bypassed: false,
    params: Object.fromEntries((plugin.parameters ?? []).map((parameter) => [parameter.id, parameter.defaultValue])),
    plugin,
  }
  return {
    id: crypto.randomUUID(),
    type: 'builtin:testtone',
    bypassed: false,
    params: { waveform: 0, attack: 0.01, decay: 0.15, sustain: 0.7, release: 0.3, gainDb: -12, polyphony: 16, velocityCurve: 1 },
  }
}

export function getProjectSnapshot(): ProjectState {
  const { project, playheadSec } = useProjectStore.getState()
  return {
    ...project,
    transport: { ...project.transport, playheadSec },
  }
}

function targetEffects(project: ProjectState, target: RackTarget): EffectInstance[] | undefined {
  if (target.kind === 'master') return project.master.effects
  if (target.kind === 'bus') return project.buses.find((bus) => bus.id === target.id)?.effects
  return project.tracks.find((track) => track.id === target.id)?.effects
}

export function effectDefaults(type: EffectInstance['type']): Record<string, number> {
  if (type === 'builtin:eq') return eqDefaults([80, 400, 2500, 10000], [3, 0, 0, 2], [1, 0, 0, 0])
  if (type === 'builtin:eq8') return { ...eqDefaults([30, 100, 300, 800, 2500, 6000, 12000, 16000], [3, 0, 0, 0, 0, 0, 0, 2], [1, 0, 0, 0, 0, 0, 0, 0]), adaptiveQ: 1, scale: 1, outputDb: 0 }
  if (type === 'builtin:compressor') return { threshold: -18, ratio: 3, attack: 0.01, release: 0.2, knee: 12, makeupDb: 0 }
  if (type === 'builtin:multiband-compressor') return { splitLow: 150, splitHigh: 2500, lowThreshold: -24, lowRatio: 3, lowAttack: 0.03, lowRelease: 0.25, lowMakeupDb: 0, midThreshold: -20, midRatio: 2.5, midAttack: 0.015, midRelease: 0.18, midMakeupDb: 0, highThreshold: -18, highRatio: 2, highAttack: 0.006, highRelease: 0.12, highMakeupDb: 0, knee: 8, outputDb: 0, mix: 1 }
  if (type === 'builtin:distortion') return { splitLow: 180, splitHigh: 4500, oversample: 4, lowEnabled: 1, lowMode: 1, lowGainDb: 0, lowDriveDb: 6, lowMix: .75, midEnabled: 1, midMode: 2, midGainDb: 0, midDriveDb: 6, midMix: .75, highEnabled: 1, highMode: 4, highGainDb: 0, highDriveDb: 6, highMix: .6 }
  if (type === 'builtin:disperser') return { frequency: 3050, amount: .25, pinch: .45 }
  if (type === 'builtin:mastering-limiter') return { algorithm: 0, inputDb: 0, outputDb: -1, releaseMs: 120, stereoLink: 1, truePeak: 1 }
  if (type === 'builtin:vocoder') return { source: 3, bands: 16, pitchHz: 110, attackMs: 5, releaseMs: 90, formantShift: 0, bandwidth: 1, mix: 1, outputDb: 0 }
  if (type === 'builtin:lfo-tremolo') return { rateHz: 4, waveform: 0, volumeDepth: .5, panDepth: 0, stereoPhase: .25 }
  if (type === 'builtin:clipper') return { inputDb: 6, knee: .25, outputDb: -1, oversample: 4 }
  if (type === 'builtin:upward-compressor') return { threshold: -32, ratio: 3, attackMs: 35, releaseMs: 240, rangeDb: 12, stereoLink: 1, mix: 1, outputDb: 0 }
  if (type === 'builtin:transient-shaper') return { attack: 0, sustain: 0, thresholdDb: -36, speed: .5, clip: 0 }
  if (type === 'builtin:roboter') return { amount: .72, number: 0 }
  if (type === 'builtin:formant-shifter') return { mode: 0, pitchSemitones: 0, formantSemitones: 0, formantLink: 1, mix: 1, outputDb: 0 }
  if (type === 'builtin:resonator') return { midi: 0, key: 0, scale: 0, pitch0: 1, pitch1: 0, pitch2: 1, pitch3: 0, pitch4: 1, pitch5: 1, pitch6: 0, pitch7: 1, pitch8: 0, pitch9: 1, pitch10: 0, pitch11: 1, resonance: .62, decay: .45, depth: .82, mix: .72 }
  if (type === 'builtin:utility') return { inputMode: 0, invertLeft: 0, invertRight: 0, width: 1, gainDb: 0, balance: 0, mono: 0, bassMono: 0, bassFreq: 120, mute: 0, dcBlock: 0 }
  if (type === 'builtin:delay') return { time: 0.25, feedback: 0.3, mix: 0.25, damping: 0.35, pingPong: 0 }
  if (type === 'builtin:reverb') return { decaySec: 2.4, damping: 0.4, width: 0.8, diffusion: 0.7, mix: 0.25 }
  if (type === 'builtin:waveshaper') return { driveDb: 6, curve: 0, outputDb: 0, mix: 1, oversample: 4, dcBlock: 1, autoLevel: 1 }
  return {}
}

const titleParameter = (id: string) => id
  .replace(/^band(\d+)\./, (_, band: string) => `Band ${Number(band) + 1} · `)
  .replace(/([a-z])([A-Z])/g, '$1 $2')
  .replace(/[._:-]+/g, ' ')
  .replace(/\b\w/g, (letter) => letter.toUpperCase())

function automationRange(id: string, value: number): Pick<AutomationOption, 'min' | 'max' | 'defaultValue'> {
  const key = id.toLowerCase()
  if (key.includes('freq')) return { min: 20, max: 20_000, defaultValue: value }
  if (key.includes('semitone')) return { min: -24, max: 24, defaultValue: value }
  if (key.includes('gain') || key.includes('threshold') || key.includes('makeup') || key.includes('output')) return { min: -60, max: 24, defaultValue: value }
  if (key.includes('attack') || key.includes('release') || key.includes('time') || key.includes('decay')) return { min: 0, max: Math.max(2, value * 4), defaultValue: value }
  if (key.includes('ratio')) return { min: 1, max: 20, defaultValue: value }
  if (key.includes('q')) return { min: 0.1, max: 18, defaultValue: value }
  if (key.includes('pan') || key.includes('balance')) return { min: -1, max: 1, defaultValue: value }
  if (key.includes('mode') || key.includes('type') || key.includes('waveform') || key.includes('oversample')) return { min: 0, max: Math.max(4, value), defaultValue: value }
  return { min: 0, max: 1, defaultValue: value }
}

/** Studio One-style Duplicate: exact extent with Snap off, otherwise the
 * smallest binary musical grid block that can contain the selection. */
export function studioOneDuplicateStepTicks(extentTicks: number, gridTicks: number, snapEnabled: boolean): number {
  if (extentTicks <= 0) return 0
  if (!snapEnabled) return Math.max(1, Math.round(extentTicks))
  let step = Math.max(1, Math.round(gridTicks))
  while (step < extentTicks) step *= 2
  return step
}

function recordArmedAutomation(track: Track, playheadSec: number, targetKind: AutomationLane['targetKind'], targetId: string, parameterId: string, value: number, playing: boolean): Track {
  if (!playing) return track
  const normalized = parameterId.replace(/^param:/, '')
  const laneIndex = (track.automationLanes ?? []).findIndex((lane) => {
    const mode = lane.mode ?? 'read'
    return (mode === 'write' || mode === 'latch') && lane.targetKind === targetKind && lane.targetId === targetId && lane.parameterId.replace(/^param:/, '') === normalized
  })
  if (laneIndex < 0) return track
  const lanes = (track.automationLanes ?? []).slice()
  const lane = lanes[laneIndex]!
  const points = lane.points.slice()
  const clipped = Math.max(lane.min, Math.min(lane.max, value))
  const tolerance = 1 / 30
  const nearby = points.findIndex((point) => Math.abs(point.timeSec - playheadSec) <= tolerance)
  if (nearby >= 0) points[nearby] = { ...points[nearby]!, timeSec: playheadSec, value: clipped }
  else points.push({ id: crypto.randomUUID(), timeSec: Math.max(0, playheadSec), value: clipped })
  points.sort((left, right) => left.timeSec - right.timeSec)
  lanes[laneIndex] = { ...lane, points }
  return { ...track, automationLanes: lanes }
}

/** Builds the categorized, stable automation menu for a track and its device chain. */
export function automationOptionsForTrack(track: Track): AutomationOption[] {
  const options: AutomationOption[] = [
    { targetKind: 'track', targetId: track.id, parameterId: 'volumeDb', category: 'INSERT', label: 'Volume', min: -60, max: 12, defaultValue: track.volumeDb },
    { targetKind: 'track', targetId: track.id, parameterId: 'pan', category: 'INSERT', label: 'Pan', min: -1, max: 1, defaultValue: track.pan },
  ]
  const addParams = (targetKind: AutomationOption['targetKind'], targetId: string, category: string, params: Record<string, number>) => {
    for (const [parameterId, value] of Object.entries(params).sort(([left], [right]) => left.localeCompare(right))) {
      options.push({ targetKind, targetId, parameterId, category, label: titleParameter(parameterId), ...automationRange(parameterId, value) })
    }
  }
  for (const [index, send] of track.sends.entries()) {
    options.push({ targetKind: 'send', targetId: send.id, parameterId: 'gainDb', category: 'SEND', label: `Send ${index + 1} · Level`, min: -60, max: 12, defaultValue: send.gainDb })
  }
  if (track.instrument) {
    const name = track.instrument.plugin?.name ?? track.instrument.type.replace('builtin:', '')
    const params = { ...track.instrument.params }
    const pluginParams = track.instrument.plugin?.parameters ?? []
    addParams('instrument', track.id, name, params)
    for (const parameter of pluginParams) options.push({ targetKind: 'instrument', targetId: track.id, parameterId: `param:${parameter.id}`, category: name, label: parameter.module ? `${parameter.module} · ${parameter.name}` : parameter.name, min: parameter.min, max: parameter.max, defaultValue: parameter.defaultValue })
  }
  for (const effect of track.effects) {
    const name = effect.plugin?.name ?? effect.type.replace('builtin:', '')
    const params = { ...effectDefaults(effect.type), ...effect.params }
    if (effect.type === 'builtin:transient-shaper') {
      const ranges: Record<string, { min: number; max: number }> = { attack: { min: -1, max: 1 }, sustain: { min: -1, max: 1 }, thresholdDb: { min: -72, max: 0 }, speed: { min: 0, max: 1 }, clip: { min: 0, max: 1 } }
      for (const [parameterId, value] of Object.entries(params)) options.push({ targetKind: 'effect', targetId: effect.id, parameterId, category: name, label: titleParameter(parameterId), min: ranges[parameterId]?.min ?? 0, max: ranges[parameterId]?.max ?? 1, defaultValue: value })
    } else addParams('effect', effect.id, name, params)
    options.push({ targetKind: 'effect', targetId: effect.id, parameterId: '__bypass', category: name, label: 'Bypass', min: 0, max: 1, defaultValue: effect.bypassed ? 1 : 0 })
    for (const parameter of effect.plugin?.parameters ?? []) options.push({ targetKind: 'effect', targetId: effect.id, parameterId: `param:${parameter.id}`, category: name, label: parameter.module ? `${parameter.module} · ${parameter.name}` : parameter.name, min: parameter.min, max: parameter.max, defaultValue: parameter.defaultValue })
  }
  return options
}

function eqDefaults(frequencies: number[], shapes: number[], enabled = frequencies.map(() => 1)): Record<string, number> {
  return Object.fromEntries(frequencies.flatMap((frequency, index) => [[`band${index}.enabled`, enabled[index] ?? 1], [`band${index}.freq`, frequency], [`band${index}.gain`, 0], [`band${index}.q`, .71], [`band${index}.type`, shapes[index] ?? 0]]))
}

function rackNodeKey(target: RackTarget): string {
  return target.kind === 'track' ? `track:${target.id}` : target.kind === 'bus' ? `bus:${target.id}` : 'master'
}

function sidechainSourceKey(sourceId: string): string {
  return sourceId.startsWith('bus:') ? sourceId : `track:${sourceId.replace(/^track:/, '')}`
}

function nodeEffects(project: ProjectState, node: string): EffectInstance[] {
  if (node === 'master') return project.master.effects
  if (node.startsWith('bus:')) return project.buses.find((bus) => bus.id === node.slice(4))?.effects ?? []
  return project.tracks.find((track) => track.id === node.replace(/^track:/, ''))?.effects ?? []
}

function createsSidechainCycle(project: ProjectState, targetNode: string, sourceNode: string): boolean {
  if (targetNode === sourceNode) return true
  const visited = new Set<string>()
  const visit = (node: string): boolean => {
    if (node === targetNode) return true
    if (visited.has(node)) return false
    visited.add(node)
    return nodeEffects(project, node).some((effect) => effect.sidechain?.enabled && effect.sidechain.sourceTrackId && visit(sidechainSourceKey(effect.sidechain.sourceTrackId)))
  }
  return visit(sourceNode)
}
