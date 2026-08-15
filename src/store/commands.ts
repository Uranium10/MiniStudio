// Engine-aware project commands shared by the menu bar, toolbar, and shortcuts.
import { describeEngineError, type IAudioEngine } from '../engine'
import { chooseAudioFile, chooseExportPath, hydrateProjectAudio, openProject, saveProject } from '../io/projectFiles'
import { getProjectSnapshot, useProjectStore } from './projectStore'

const store = () => useProjectStore.getState()

export async function togglePlayback(engine: IAudioEngine): Promise<void> {
  const state = store()
  try {
    if (state.project.transport.isPlaying) { await engine.pause(); state.setPlaying(false) }
    else { await engine.play(state.playheadSec); state.setPlaying(true) }
  } catch (error) {
    state.showToast(`재생 장치를 시작할 수 없습니다: ${describeEngineError(error)}`)
  }
}

export function stopPlayback(engine: IAudioEngine): void {
  void engine.stop()
  store().setPlaying(false)
  store().setPlayhead(0)
}

export function seekTo(engine: IAudioEngine, sec: number): void {
  const target = Math.max(0, sec)
  store().setPlayhead(target)
  void engine.seek(target)
}

export async function importAudio(engine: IAudioEngine): Promise<void> {
  try {
    const path = await chooseAudioFile()
    if (!path) return
    store().showToast('오디오를 디코딩하고 있습니다…')
    store().addAssetAsTrack(await engine.loadAudioFile(path))
    store().showToast('오디오를 가져왔습니다')
  } catch (error) {
    store().showToast(`오디오를 가져오지 못했습니다: ${describeEngineError(error)}`)
  }
}

export async function openProjectFile(engine: IAudioEngine): Promise<boolean> {
  try {
    const result = await openProject()
    if (!result) return false
    const hydrated = await hydrateProjectAudio(engine, result.project)
    store().setProject(hydrated.project)
    store().setMissingAssets(hydrated.missing)
    store().showToast('프로젝트를 열었습니다')
    return true
  } catch {
    store().showToast('프로젝트 파일을 열 수 없습니다')
    return false
  }
}

export async function saveProjectFile(engine: IAudioEngine): Promise<void> {
  try {
    await saveProject(await snapshotWithPluginStates(engine, getProjectSnapshot()))
    store().showToast('프로젝트를 저장했습니다')
  } catch {
    store().showToast('데스크톱 앱에서 저장할 수 있습니다')
  }
}

async function snapshotWithPluginStates(engine: IAudioEngine, source: ReturnType<typeof getProjectSnapshot>): Promise<ReturnType<typeof getProjectSnapshot>> {
  const project = structuredClone(source)
  for (const track of project.tracks) {
    if (track.instrument?.plugin) {
      try { track.instrument.plugin.state = await engine.savePluginState('instrument', track.id) } catch { /* Preserve the previous state when the plug-in declines snapshots. */ }
    }
    for (const effect of track.effects) if (effect.plugin) {
      try { effect.plugin.state = await engine.savePluginState('effect', effect.id) } catch { /* Optional state extension. */ }
    }
  }
  for (const bus of project.buses) for (const effect of bus.effects) if (effect.plugin) {
    try { effect.plugin.state = await engine.savePluginState('effect', effect.id) } catch { /* Optional state extension. */ }
  }
  for (const effect of project.master.effects) if (effect.plugin) {
    try { effect.plugin.state = await engine.savePluginState('effect', effect.id) } catch { /* Optional state extension. */ }
  }
  return project
}

export async function exportMasterWav(engine: IAudioEngine): Promise<void> {
  try {
    const project = getProjectSnapshot()
    const path = await chooseExportPath(project.meta.name)
    if (!path) return
    store().showToast('마스터 WAV를 렌더링하고 있습니다…')
    await engine.exportProject(project, path)
    store().showToast('WAV 내보내기를 완료했습니다')
  } catch (error) {
    store().showToast(`WAV 내보내기에 실패했습니다: ${describeEngineError(error)}`)
  }
}

/** Splits every selected clip at the playhead. */
export function splitSelectionAtPlayhead(): void {
  const state = store()
  const selected = new Set(state.selectedClipIds)
  if (!selected.size) { state.showToast('먼저 클립을 선택하세요'); return }
  for (const track of state.project.tracks) {
    for (const clip of [...track.clips, ...track.midiClips]) {
      if (selected.has(clip.id)) state.splitClip(track.id, clip.id, state.playheadSec)
    }
  }
}

export function selectAllInContext(): void {
  const state = store()
  if (state.editFocus === 'pianoRoll' && state.pianoRollOpen && state.editorClip) {
    const clip = state.project.tracks.find((track) => track.id === state.editorClip!.trackId)?.midiClips.find((item) => item.id === state.editorClip!.clipId)
    useProjectStore.setState({ selectedNoteIds: clip?.notes.map((note) => note.id) ?? [] })
    return
  }
  useProjectStore.setState({ selectedClipIds: state.project.tracks.flatMap((track) => [...track.clips, ...track.midiClips].map((clip) => clip.id)) })
}

export function deleteInContext(): void {
  const state = store()
  if (state.selectedClipGainPoint) { const point = state.selectedClipGainPoint; state.removeClipGainPoint(point.trackId, point.clipId, point.pointId); return }
  if (state.selectedAutomationPoints.length) { state.deleteSelectedAutomationPoints(); return }
  if (state.editFocus === 'pianoRoll' && state.pianoRollOpen && state.editorClip && state.selectedNoteIds.length) {
    state.deleteMidiNotes(state.editorClip.trackId, state.editorClip.clipId, state.selectedNoteIds)
    return
  }
  if (state.selectedClipIds.length) {
    state.deleteSelectedClips()
    return
  }
  if (state.editFocus === 'arrangement' && state.selectedTrackId) state.removeSelectedTrack()
}

export function copyInContext(): void {
  const state = store()
  if (state.selectedAutomationPoints.length) state.copySelectedAutomationPoints()
  else state.copySelectedClips()
}

export function pasteInContext(): void {
  const state = store()
  if (state.selectedAutomationPoints.length || (state.automationClipboard && !state.clipboard)) state.pasteAutomationPoints(state.playheadSec)
  else state.pasteClipboard()
}

/** Quantizes notes in the piano roll, otherwise arrangement clips, to the active grid. */
export function quantizeInContext(): void {
  const state = store()
  if (state.editFocus === 'pianoRoll' && state.editorClip && state.selectedNoteIds.length) {
    const clip = state.project.tracks.find((track) => track.id === state.editorClip!.trackId)?.midiClips.find((item) => item.id === state.editorClip!.clipId)
    if (!clip) return
    const selected = new Set(state.selectedNoteIds)
    state.updateMidiNoteBatch(state.editorClip.trackId, state.editorClip.clipId, clip.notes.filter((note) => selected.has(note.id)).map((note) => ({ id: note.id, patch: { startTicks: Math.max(0, Math.round(note.startTicks / state.gridTicks) * state.gridTicks) } })))
    return
  }
  const selected = new Set(state.selectedClipIds)
  const step = (state.gridTicks / 960) * (60 / state.project.transport.bpm)
  for (const track of state.project.tracks) for (const clip of [...track.clips, ...track.midiClips]) if (selected.has(clip.id)) state.updateClip(track.id, clip.id, { startSec: Math.max(0, Math.round(clip.startSec / step) * step) })
}
