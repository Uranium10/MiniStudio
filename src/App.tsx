// Top-level MiniStudio layout and engine/store synchronization.
import { useEffect } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { Inspector } from './components/Inspector'
import { AudioSettingsDialog } from './components/AudioSettingsDialog'
import { BrowserPanel } from './components/BrowserPanel'
import { ExportDialog } from './components/ExportDialog'
import { LowerPanel } from './components/LowerPanel'
import { MissingAssetsDialog } from './components/MissingAssetsDialog'
import { PianoRollPanel } from './components/PianoRollPanel'
import { MenuBar, ToolBar } from './components/TopChrome'
import { Timeline } from './components/Timeline'
import { TransportBar } from './components/TransportBar'
import { ShortcutsDialog } from './components/ShortcutsDialog'
import { StartupDialog } from './components/StartupDialog'
import { VirtualPiano } from './components/VirtualPiano'
import { useFineRangeControls } from './components/controls'
import { describeEngineError, effectiveBusGainDb, effectiveMasterGainDb, type AutomationLane, type ExternalPluginRef, type IAudioEngine, type PluginDescriptor, type ProjectState } from './engine'
import { useEngine } from './hooks/useEngine'
import { markSessionClean, writeRecoverySnapshot } from './io/autosave'
import { installMidiInputCoordinator } from './midi/MidiInputCoordinator'
import { hydratePluginRef, scanPluginsOnce } from './plugins/scan'
import { installPluginShellBridge } from './plugins/shellBridge'
import { useShortcuts } from './shortcuts/useShortcuts'
import { useProjectStore } from './store/projectStore'
import './App.css'

export default function App() {
  const engine = useEngine()
  const lowerHeight = useProjectStore((state) => (state.lowerTab === 'effects' ? state.effectsPanelHeight : state.mixerPanelHeight) ?? state.lowerPanelHeight)
  const collapsed = useProjectStore((state) => state.lowerPanelCollapsed)
  const editorMaximized = useProjectStore((state) => state.editorMaximized)
  const pianoRollOpen = useProjectStore((state) => state.pianoRollOpen)
  const pianoRollHeight = useProjectStore((state) => state.pianoRollHeight)
  const inspectorVisible = useProjectStore((state) => state.inspectorVisible)
  const browserVisible = useProjectStore((state) => state.browserVisible)
  const browserDock = useProjectStore((state) => state.browserDock)
  const toast = useProjectStore((state) => state.toast)
  useShortcuts()
  useFineRangeControls()

  useEffect(() => installPluginShellBridge(engine), [engine])

  useEffect(() => { void scanPluginsOnce(engine).catch(() => undefined) }, [engine])

  useEffect(() => {
    const attempted = new Set<string>()
    const hydrate = (project: ProjectState) => {
      const refs: Array<{ reference: ExternalPluginRef; instrument: boolean }> = []
      for (const track of project.tracks) {
        if (track.instrument?.plugin) refs.push({ reference: track.instrument.plugin, instrument: true })
        for (const effect of track.effects) if (effect.plugin) refs.push({ reference: effect.plugin, instrument: false })
      }
      for (const bus of project.buses) for (const effect of bus.effects) if (effect.plugin) refs.push({ reference: effect.plugin, instrument: false })
      for (const effect of project.master.effects) if (effect.plugin) refs.push({ reference: effect.plugin, instrument: false })
      for (const { reference, instrument } of refs) {
        if (reference.parameters?.length || (reference.parameters !== undefined && reference.paramCount === 0)) continue
        const key = `${reference.format}:${reference.uid}:${reference.path}`
        if (attempted.has(key)) continue
        attempted.add(key)
        void hydratePluginRef(engine, reference, instrument).then((hydrated) => mergeHydratedPluginReference(hydrated)).catch(() => undefined)
      }
    }
    hydrate(useProjectStore.getState().project)
    return useProjectStore.subscribe((state, previous) => { if (state.project !== previous.project) hydrate(state.project) })
  }, [engine])

  useEffect(() => {
    let saveTimer = 0
    let active = false
    const schedule = () => {
      if (!active) return
      window.clearTimeout(saveTimer)
      saveTimer = window.setTimeout(() => writeRecoverySnapshot(useProjectStore.getState().project), 1_500)
    }
    const start = () => { active = true; writeRecoverySnapshot(useProjectStore.getState().project) }
    const clean = () => markSessionClean()
    window.addEventListener('ministudio:session-started', start)
    window.addEventListener('beforeunload', clean)
    const unsubscribe = useProjectStore.subscribe((state, previous) => { if (state.project !== previous.project) schedule() })
    return () => { unsubscribe(); window.clearTimeout(saveTimer); window.removeEventListener('ministudio:session-started', start); window.removeEventListener('beforeunload', clean) }
  }, [])

  useEffect(() => {
    let timer = 0
    let signature = ''
    let hasRequestedInitialGraph = false
    let graphSyncRunning = false
    let graphSyncQueued = false
    let stopped = false
    let realtimeValues = new Map<string, number | boolean>()
    const runGraphSync = async () => {
      if (graphSyncRunning || stopped) return
      graphSyncRunning = true
      try {
        do {
          graphSyncQueued = false
          const current = useProjectStore.getState().project
          // Mute/dim are UI-side gain offsets, so the graph receives effective gains.
          await engine.syncGraph({
            tracks: current.tracks,
            buses: current.buses.map((bus) => ({ ...bus, volumeDb: effectiveBusGainDb(bus) })),
            master: { ...current.master, volumeDb: effectiveMasterGainDb(current.master) },
            transport: { ...current.transport, playheadSec: 0, isPlaying: false },
            audioSourceRefs: current.audioSourceRefs,
          })
          // Store/plugin hydration can request another structural graph while
          // the current native rebuild is still running. Consume the latest
          // snapshot first and announce only the final stable graph; otherwise
          // editor restoration can attach to an instance retired immediately
          // by the next rebuild.
        } while (!stopped && graphSyncQueued)
        if (!stopped) window.dispatchEvent(new Event('ministudio:graph-synced'))
      } catch (error) {
        if (!stopped) useProjectStore.getState().showToast(`오디오 그래프를 준비하지 못했습니다: ${describeEngineError(error)}`)
      } finally {
        graphSyncRunning = false
        // Cover a request that arrived after the loop condition but before the
        // async continuation reached finally.
        if (!stopped && graphSyncQueued) void runGraphSync()
      }
    }
    const syncGraph = () => {
      graphSyncQueued = true
      if (!graphSyncRunning) void runGraphSync()
    }
    const syncEngineState = () => {
      const project = useProjectStore.getState().project
      const nextSignature = graphStructureSignature(project)
      if (nextSignature !== signature) {
        signature = nextSignature
        realtimeValues = collectRealtimeValues(project)
        window.clearTimeout(timer)
        if (!hasRequestedInitialGraph) { hasRequestedInitialGraph = true; syncGraph() }
        // One macrotask coalesces synchronous store mutations without adding a
        // human-visible 100 ms penalty before a newly inserted plug-in exists.
        else timer = window.setTimeout(syncGraph, 0)
        return
      }
      realtimeValues = applyRealtimeChanges(engine, project, realtimeValues)
    }
    syncEngineState()
    const unsubscribe = useProjectStore.subscribe(syncEngineState)
    const pluginsRefreshed = (event: Event) => {
      // Inspecting a browser candidate also refreshes the global descriptor
      // cache. Rebuild only when that refresh actually changed a plug-in
      // already referenced by this project; otherwise dropping BBC after
      // Serum needlessly rebuilds Serum once before the real BBC graph commit.
      if (refreshProjectPluginMetadata((event as CustomEvent<PluginDescriptor[]>).detail ?? [])) syncGraph()
    }
    window.addEventListener('ministudio:plugins-refreshed', pluginsRefreshed)
    return () => { stopped = true; unsubscribe(); window.clearTimeout(timer); window.removeEventListener('ministudio:plugins-refreshed', pluginsRefreshed) }
  }, [engine])

  useEffect(() => engine.onPlayhead((sec) => {
    const store = useProjectStore.getState()
    store.setPlayhead(sec)
    store.setCountInActive(engine.getStreamStatus().countInBeatsRemaining > 0)
    applyReadAutomation(engine, store.project, sec)
  }), [engine])

  useEffect(() => engine.onPluginParameterChanges((changes) => useProjectStore.getState().applyPluginParameterChanges(changes)), [engine])

  useEffect(() => installMidiInputCoordinator(engine), [engine])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    let dragPaths: string[] = []
    const logical = (value: number) => value / Math.max(1, window.devicePixelRatio || 1)
    void getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload
      if (payload.type === 'enter') dragPaths = payload.paths.filter((candidate) => /\.(wav|mp3|flac|ogg|m4a|aac)$/i.test(candidate))
      if (payload.type === 'leave') { dragPaths = []; window.dispatchEvent(new CustomEvent('ministudio-native-audio-drag', { detail: { type: 'leave' } })); return }
      if (payload.type !== 'over' && payload.type !== 'drop' && payload.type !== 'enter') return
      const paths = payload.type === 'drop' ? payload.paths.filter((candidate) => /\.(wav|mp3|flac|ogg|m4a|aac)$/i.test(candidate)) : dragPaths
      window.dispatchEvent(new CustomEvent('ministudio-native-audio-drag', { detail: { type: payload.type, paths, x: logical(payload.position.x), y: logical(payload.position.y) } }))
      if (payload.type === 'drop') dragPaths = []
    }).then((dispose) => { unlisten = dispose }).catch(() => { /* Browser preview has no Tauri event bridge. */ })
    return () => unlisten?.()
  }, [])

  return (
    <div
      className={`daw-shell ${editorMaximized ? 'editor-maximized' : ''} ${browserVisible ? `browser-visible browser-${browserDock}` : ''}`}
      style={{ '--lower-height': collapsed || editorMaximized ? '0px' : `${lowerHeight}px`, '--piano-height': pianoRollOpen ? `${pianoRollHeight}px` : '0px' } as React.CSSProperties}
      onContextMenu={(event) => event.preventDefault()}
    >
      <MenuBar />
      <ToolBar />
      <main className={`workspace ${inspectorVisible ? '' : 'inspector-hidden'}`}>
        {inspectorVisible && <Inspector />}
        <Timeline />
      </main>
      {browserVisible && <BrowserPanel />}
      <PianoRollPanel />
      <LowerPanel />
      <TransportBar />
      <MissingAssetsDialog />
      <AudioSettingsDialog />
      <ExportDialog />
      <ShortcutsDialog />
      <StartupDialog />
      <VirtualPiano />
      {toast && <button className="toast" type="button" title="클릭하여 닫기" onClick={() => useProjectStore.getState().clearToast()}><span role="status">{toast}</span></button>}
    </div>
  )
}

function refreshProjectPluginMetadata(plugins: PluginDescriptor[]): boolean {
  if (!plugins.length) return false
  const byId = new Map(plugins.map((plugin) => [`${plugin.format}:${plugin.uid}`, plugin]))
  const project = structuredClone(useProjectStore.getState().project)
  let changed = false
  const refresh = (reference: NonNullable<ProjectState['tracks'][number]['instrument']>['plugin']) => {
    if (!reference) return
    const descriptor = byId.get(`${reference.format}:${reference.uid}`)
    if (!descriptor) return
    const isCurrent = reference.path === descriptor.path
      && reference.hasEditor === descriptor.hasEditor
      && (reference.parameters?.length ?? 0) === descriptor.parameters.length
    if (isCurrent) return
    Object.assign(reference, { path: descriptor.path, audioInputBuses: descriptor.audioInputBuses, audioOutputBuses: descriptor.audioOutputBuses, supportsSidechain: descriptor.supportsSidechain, hasEditor: descriptor.hasEditor, paramCount: descriptor.paramCount, parameters: descriptor.parameters })
    changed = true
  }
  for (const track of project.tracks) { refresh(track.instrument?.plugin); for (const effect of track.effects) refresh(effect.plugin) }
  for (const bus of project.buses) for (const effect of bus.effects) refresh(effect.plugin)
  for (const effect of project.master.effects) refresh(effect.plugin)
  if (changed) useProjectStore.setState({ project })
  return changed
}

function mergeHydratedPluginReference(hydrated: ExternalPluginRef): void {
  if (!hydrated.parameters?.length) return
  const project = structuredClone(useProjectStore.getState().project)
  let changed = false
  const merge = (reference: typeof hydrated, params: Record<string, number>) => {
    if (!reference || reference.format !== hydrated.format || reference.uid !== hydrated.uid) return
    Object.assign(reference, hydrated)
    for (const parameter of hydrated.parameters ?? []) if (!(parameter.id in params)) params[parameter.id] = parameter.defaultValue
    changed = true
  }
  for (const track of project.tracks) {
    if (track.instrument?.plugin) merge(track.instrument.plugin, track.instrument.params)
    for (const effect of track.effects) if (effect.plugin) merge(effect.plugin, effect.params)
  }
  for (const bus of project.buses) for (const effect of bus.effects) if (effect.plugin) merge(effect.plugin, effect.params)
  for (const effect of project.master.effects) if (effect.plugin) merge(effect.plugin, effect.params)
  if (changed) useProjectStore.setState({ project })
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
  const linearMid = (from.value + to.value) / 2
  const bend = Math.max(-1, Math.min(1, from.curve ?? 0)) * (lane.max - lane.min) * .35
  const control = 2 * (linearMid + bend) - .5 * (from.value + to.value)
  const inverse = 1 - mix
  return Math.max(lane.min, Math.min(lane.max, inverse * inverse * from.value + 2 * inverse * mix * control + mix * mix * to.value))
}

/** Read-mode automation. Native smoothers make the 30 fps control stream click-free. */
function applyReadAutomation(engine: IAudioEngine, project: ProjectState, sec: number): void {
  for (const track of project.tracks) for (const lane of track.automationLanes ?? []) {
    if ((lane.mode ?? 'read') === 'off' || lane.mode === 'write') continue
    const value = automationValueAt(lane, sec)
    if (value === null) continue
    if (lane.targetKind === 'track') {
      if (lane.parameterId === 'volumeDb') engine.setTrackVolume(track.id, value)
      else if (lane.parameterId === 'pan') engine.setTrackPan(track.id, value)
    } else if (lane.targetKind === 'send') engine.setSendLevel(lane.targetId, value)
    else if (lane.targetKind === 'instrument') engine.setInstrumentParam(track.id, lane.parameterId, value)
    else if (lane.parameterId === '__bypass') engine.setEffectBypass(lane.targetId, value >= .5)
    else engine.setEffectParam(lane.targetId, lane.parameterId, value)
  }
}

function graphStructureSignature(project: ProjectState): string {
  const fields: Array<string | number | boolean> = [project.transport.bpm, project.transport.loop.enabled, project.transport.loop.startSec, project.transport.loop.endSec]
  for (const point of project.transport.tempoMap.tempoPoints) fields.push(point.tick, point.bpm, point.curve)
  for (const signature of project.transport.tempoMap.timeSignatures) fields.push(signature.bar, signature.numerator, signature.denominator)
  for (const track of project.tracks) {
    fields.push(track.id, track.kind, track.outputBusId ?? '')
    for (const clip of track.clips) { const source = project.audioSourceRefs[clip.audioSourceRefId]; fields.push(clip.id, clip.audioSourceRefId, source?.assetId ?? '', clip.startSec, clip.offsetSec, clip.durationSec, clip.gainDb, clip.fadeInSec, clip.fadeOutSec, clip.fadeInCurve ?? 0, clip.fadeOutCurve ?? 0, clip.muted ?? false, clip.playbackRate ?? 1, clip.pitchSemitones ?? 0, clip.fineCents ?? 0, clip.reversed ?? false, clip.warpMode ?? 'none', clip.warpSourceBpm ?? project.transport.bpm); for (const point of clip.gainPoints ?? []) fields.push(point.id, point.timeSec, point.valueDb, point.curve ?? 0) }
    for (const clip of track.midiClips) {
      fields.push(clip.id, clip.startSec, clip.durationSec, clip.loopEnabled, clip.loopStartTicks, clip.loopLengthTicks, clip.transposeSemitones, clip.velocityScale, clip.muted)
      for (const note of clip.notes) fields.push(note.id, note.pitch, note.velocity, note.startTicks, note.lengthTicks, note.releaseVelocity, note.muted)
      for (const lane of clip.ccLanes) { fields.push(lane.cc); for (const point of lane.points) fields.push(point.ticks, point.value) }
    }
    if (track.instrument) fields.push(track.instrument.id, track.instrument.type, track.instrument.bypassed)
    for (const effect of track.effects) fields.push(effect.id, effect.type, effect.sidechain?.enabled ?? false, effect.sidechain?.sourceTrackId ?? '', effect.type === 'builtin:resonator' ? Math.round(effect.params.quality ?? 0) : '')
    for (const send of track.sends) fields.push(send.id, send.targetBusId, send.preFader)
  }
  for (const bus of project.buses) {
    fields.push(bus.id)
    for (const effect of bus.effects) fields.push(effect.id, effect.type, effect.sidechain?.enabled ?? false, effect.sidechain?.sourceTrackId ?? '', effect.type === 'builtin:resonator' ? Math.round(effect.params.quality ?? 0) : '')
  }
  for (const effect of project.master.effects) fields.push(effect.id, effect.type, effect.sidechain?.enabled ?? false, effect.sidechain?.sourceTrackId ?? '', effect.type === 'builtin:resonator' ? Math.round(effect.params.quality ?? 0) : '')
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
    for (const effect of track.effects) {
      values.set(`effect-bypass:${effect.id}`, effect.bypassed)
      for (const [param, value] of Object.entries(effect.params)) values.set(`effect:${effect.id}:${param}`, value)
    }
  }
  for (const bus of project.buses) {
    values.set(`bus-volume:${bus.id}`, effectiveBusGainDb(bus))
    for (const effect of bus.effects) {
      values.set(`effect-bypass:${effect.id}`, effect.bypassed)
      for (const [param, value] of Object.entries(effect.params)) values.set(`effect:${effect.id}:${param}`, value)
    }
  }
  values.set('master-volume', effectiveMasterGainDb(project.master))
  for (const effect of project.master.effects) {
    values.set(`effect-bypass:${effect.id}`, effect.bypassed)
    for (const [param, value] of Object.entries(effect.params)) values.set(`effect:${effect.id}:${param}`, value)
  }
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
    for (const effect of track.effects) {
      if (changed(`effect-bypass:${effect.id}`)) engine.setEffectBypass(effect.id, effect.bypassed)
      for (const [param, value] of Object.entries(effect.params)) if (changed(`effect:${effect.id}:${param}`)) engine.setEffectParam(effect.id, param, value)
    }
  }
  for (const bus of project.buses) {
    if (changed(`bus-volume:${bus.id}`)) engine.setBusVolume(bus.id, effectiveBusGainDb(bus))
    for (const effect of bus.effects) {
      if (changed(`effect-bypass:${effect.id}`)) engine.setEffectBypass(effect.id, effect.bypassed)
      for (const [param, value] of Object.entries(effect.params)) if (changed(`effect:${effect.id}:${param}`)) engine.setEffectParam(effect.id, param, value)
    }
  }
  if (changed('master-volume')) engine.setMasterVolume(effectiveMasterGainDb(project.master))
  for (const effect of project.master.effects) {
    if (changed(`effect-bypass:${effect.id}`)) engine.setEffectBypass(effect.id, effect.bypassed)
    for (const [param, value] of Object.entries(effect.params)) if (changed(`effect:${effect.id}:${param}`)) engine.setEffectParam(effect.id, param, value)
  }
  return next
}
