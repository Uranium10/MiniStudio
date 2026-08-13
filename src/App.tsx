// Top-level MiniDAW layout and engine/store synchronization.
import { useEffect } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { Inspector } from './components/Inspector'
import { AudioSettingsDialog } from './components/AudioSettingsDialog'
import { LowerPanel } from './components/LowerPanel'
import { MissingAssetsDialog } from './components/MissingAssetsDialog'
import { PianoRollPanel } from './components/PianoRollPanel'
import { MenuBar, ToolBar } from './components/TopChrome'
import { Timeline } from './components/Timeline'
import { TransportBar } from './components/TransportBar'
import { ShortcutsDialog } from './components/ShortcutsDialog'
import { VirtualPiano } from './components/VirtualPiano'
import { describeEngineError, effectiveBusGainDb, effectiveMasterGainDb, type AutomationLane, type IAudioEngine, type ProjectState } from './engine'
import { useEngine } from './hooks/useEngine'
import { useShortcuts } from './shortcuts/useShortcuts'
import { useProjectStore } from './store/projectStore'
import './App.css'

export default function App() {
  const engine = useEngine()
  const lowerHeight = useProjectStore((state) => state.lowerPanelHeight)
  const collapsed = useProjectStore((state) => state.lowerPanelCollapsed)
  const editorMaximized = useProjectStore((state) => state.editorMaximized)
  const pianoRollOpen = useProjectStore((state) => state.pianoRollOpen)
  const pianoRollHeight = useProjectStore((state) => state.pianoRollHeight)
  const inspectorVisible = useProjectStore((state) => state.inspectorVisible)
  const toast = useProjectStore((state) => state.toast)
  useShortcuts()

  useEffect(() => {
    let timer = 0
    let signature = ''
    let hasRequestedInitialGraph = false
    let realtimeValues = new Map<string, number | boolean>()
    const syncGraph = () => {
      const current = useProjectStore.getState().project
      // Mute/dim are UI-side gain offsets, so the graph receives effective gains.
      void engine.syncGraph({
        tracks: current.tracks,
        buses: current.buses.map((bus) => ({ ...bus, volumeDb: effectiveBusGainDb(bus) })),
        master: { ...current.master, volumeDb: effectiveMasterGainDb(current.master) },
        transport: { ...current.transport, playheadSec: 0, isPlaying: false },
      }).catch((error) => useProjectStore.getState().showToast(`오디오 그래프를 준비하지 못했습니다: ${describeEngineError(error)}`))
    }
    const syncEngineState = () => {
      const project = useProjectStore.getState().project
      const nextSignature = graphStructureSignature(project)
      if (nextSignature !== signature) {
        signature = nextSignature
        realtimeValues = collectRealtimeValues(project)
        window.clearTimeout(timer)
        if (!hasRequestedInitialGraph) { hasRequestedInitialGraph = true; syncGraph() }
        else timer = window.setTimeout(syncGraph, 100)
        return
      }
      realtimeValues = applyRealtimeChanges(engine, project, realtimeValues)
    }
    syncEngineState()
    const unsubscribe = useProjectStore.subscribe(syncEngineState)
    return () => { unsubscribe(); window.clearTimeout(timer) }
  }, [engine])

  useEffect(() => engine.onPlayhead((sec) => {
    const store = useProjectStore.getState()
    store.setPlayhead(sec)
    applyReadAutomation(engine, store.project, sec)
  }), [engine])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== 'drop') return
      for (const path of event.payload.paths.filter((candidate) => /\.(wav|mp3|flac|ogg|m4a)$/i.test(candidate))) {
        void engine.loadAudioFile(path).then((asset) => useProjectStore.getState().addAssetAsTrack(asset)).catch(() => useProjectStore.getState().showToast('드롭한 오디오를 읽을 수 없습니다'))
      }
    }).then((dispose) => { unlisten = dispose }).catch(() => { /* Browser preview has no Tauri event bridge. */ })
    return () => unlisten?.()
  }, [engine])

  return (
    <div
      className={`daw-shell ${editorMaximized ? 'editor-maximized' : ''}`}
      style={{ '--lower-height': collapsed || editorMaximized ? '0px' : `${lowerHeight}px`, '--piano-height': pianoRollOpen ? `${pianoRollHeight}px` : '0px' } as React.CSSProperties}
      onContextMenu={(event) => event.preventDefault()}
    >
      <MenuBar />
      <ToolBar />
      <main className={`workspace ${inspectorVisible ? '' : 'inspector-hidden'}`}>
        {inspectorVisible && <Inspector />}
        <Timeline />
      </main>
      <PianoRollPanel />
      <LowerPanel />
      <TransportBar />
      <MissingAssetsDialog />
      <AudioSettingsDialog />
      <ShortcutsDialog />
      <VirtualPiano />
      {toast && <button className="toast" type="button" title="클릭하여 닫기" onClick={() => useProjectStore.getState().clearToast()}><span role="status">{toast}</span></button>}
    </div>
  )
}

function automationValueAt(lane: AutomationLane, sec: number): number | null {
  if (!lane.points.length) return null
  const points = lane.points
  if (sec <= points[0]!.timeSec) return points[0]!.value
  if (sec >= points.at(-1)!.timeSec) return points.at(-1)!.value
  let right = 1
  while (right < points.length && points[right]!.timeSec < sec) right += 1
  const from = points[right - 1]!
  const to = points[right]!
  const mix = (sec - from.timeSec) / Math.max(0.000_001, to.timeSec - from.timeSec)
  return from.value + (to.value - from.value) * mix
}

/** Read-mode automation. Native smoothers make the 30 fps control stream click-free. */
function applyReadAutomation(engine: IAudioEngine, project: ProjectState, sec: number): void {
  for (const track of project.tracks) for (const lane of track.automationLanes ?? []) {
    const value = automationValueAt(lane, sec)
    if (value === null) continue
    if (lane.targetKind === 'track') {
      if (lane.parameterId === 'volumeDb') engine.setTrackVolume(track.id, value)
      else if (lane.parameterId === 'pan') engine.setTrackPan(track.id, value)
    } else if (lane.targetKind === 'instrument') engine.setInstrumentParam(track.id, lane.parameterId, value)
    else engine.setEffectParam(lane.targetId, lane.parameterId, value)
  }
}

function graphStructureSignature(project: ProjectState): string {
  const fields: Array<string | number | boolean> = [project.transport.bpm, project.transport.loop.enabled, project.transport.loop.startSec, project.transport.loop.endSec]
  for (const track of project.tracks) {
    fields.push(track.id, track.kind, track.outputBusId ?? '')
    for (const clip of track.clips) fields.push(clip.id, clip.assetId, clip.startSec, clip.offsetSec, clip.durationSec, clip.gainDb, clip.fadeInSec, clip.fadeOutSec, clip.muted ?? false)
    for (const clip of track.midiClips) {
      fields.push(clip.id, clip.startSec, clip.durationSec, clip.loopEnabled, clip.loopStartTicks, clip.loopLengthTicks, clip.transposeSemitones, clip.velocityScale, clip.muted)
      for (const note of clip.notes) fields.push(note.id, note.pitch, note.velocity, note.startTicks, note.lengthTicks, note.releaseVelocity, note.muted)
    }
    if (track.instrument) fields.push(track.instrument.id, track.instrument.type, track.instrument.bypassed)
    for (const effect of track.effects) fields.push(effect.id, effect.type, effect.bypassed, effect.sidechain?.enabled ?? false, effect.sidechain?.sourceTrackId ?? '')
    for (const send of track.sends) fields.push(send.id, send.targetBusId, send.preFader)
  }
  for (const bus of project.buses) {
    fields.push(bus.id)
    for (const effect of bus.effects) fields.push(effect.id, effect.type, effect.bypassed, effect.sidechain?.enabled ?? false, effect.sidechain?.sourceTrackId ?? '')
  }
  for (const effect of project.master.effects) fields.push(effect.id, effect.type, effect.bypassed, effect.sidechain?.enabled ?? false, effect.sidechain?.sourceTrackId ?? '')
  return fields.join('|')
}

function collectRealtimeValues(project: ProjectState): Map<string, number | boolean> {
  const values = new Map<string, number | boolean>()
  for (const track of project.tracks) {
    values.set(`track-volume:${track.id}`, track.volumeDb)
    values.set(`track-pan:${track.id}`, track.pan)
    values.set(`track-mute:${track.id}`, track.muted)
    values.set(`track-solo:${track.id}`, track.solo)
    for (const send of track.sends) values.set(`send:${send.id}`, send.gainDb)
    if (track.instrument) for (const [param, value] of Object.entries(track.instrument.params)) values.set(`instrument:${track.id}:${param}`, value)
    for (const effect of track.effects) for (const [param, value] of Object.entries(effect.params)) values.set(`effect:${effect.id}:${param}`, value)
  }
  for (const bus of project.buses) {
    values.set(`bus-volume:${bus.id}`, effectiveBusGainDb(bus))
    for (const effect of bus.effects) for (const [param, value] of Object.entries(effect.params)) values.set(`effect:${effect.id}:${param}`, value)
  }
  values.set('master-volume', effectiveMasterGainDb(project.master))
  for (const effect of project.master.effects) for (const [param, value] of Object.entries(effect.params)) values.set(`effect:${effect.id}:${param}`, value)
  return values
}

function applyRealtimeChanges(engine: IAudioEngine, project: ProjectState, previous: Map<string, number | boolean>): Map<string, number | boolean> {
  const next = collectRealtimeValues(project)
  const changed = (key: string) => previous.get(key) !== next.get(key)
  for (const track of project.tracks) {
    if (changed(`track-volume:${track.id}`)) engine.setTrackVolume(track.id, track.volumeDb)
    if (changed(`track-pan:${track.id}`)) engine.setTrackPan(track.id, track.pan)
    if (changed(`track-mute:${track.id}`)) engine.setTrackMute(track.id, track.muted)
    if (changed(`track-solo:${track.id}`)) engine.setTrackSolo(track.id, track.solo)
    for (const send of track.sends) if (changed(`send:${send.id}`)) engine.setSendLevel(send.id, send.gainDb)
    if (track.instrument) for (const [param, value] of Object.entries(track.instrument.params)) if (changed(`instrument:${track.id}:${param}`)) engine.setInstrumentParam(track.id, param, value)
    for (const effect of track.effects) for (const [param, value] of Object.entries(effect.params)) if (changed(`effect:${effect.id}:${param}`)) engine.setEffectParam(effect.id, param, value)
  }
  for (const bus of project.buses) {
    if (changed(`bus-volume:${bus.id}`)) engine.setBusVolume(bus.id, effectiveBusGainDb(bus))
    for (const effect of bus.effects) for (const [param, value] of Object.entries(effect.params)) if (changed(`effect:${effect.id}:${param}`)) engine.setEffectParam(effect.id, param, value)
  }
  if (changed('master-volume')) engine.setMasterVolume(effectiveMasterGainDb(project.master))
  for (const effect of project.master.effects) for (const [param, value] of Object.entries(effect.params)) if (changed(`effect:${effect.id}:${param}`)) engine.setEffectParam(effect.id, param, value)
  return next
}
