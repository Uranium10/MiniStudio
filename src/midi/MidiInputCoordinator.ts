import type { IAudioEngine, MidiInputPortInfo } from '../engine'
import { useProjectStore } from '../store/projectStore'

/**
 * Owns physical MIDI discovery, native routing and the Web MIDI fallback.
 *
 * Native MidiSrv calls are authoritative when healthy. The backend probes them in a disposable
 * helper with a deadline; if Windows MIDI is wedged, WebView MIDI keeps note input usable without
 * blocking graph/editor control. No timer polls either backend.
 */
export function installMidiInputCoordinator(engine: IAudioEngine): () => void {
  let cancelled = false
  let checking = false
  let requestedRevision = 0
  let scanRequested = false
  let knownPorts = new Map<string, MidiInputPortInfo>()
  const announced = new Set<string>()
  const nativeConnected = new Set<string>()
  const webNotes = new Map<string, Array<{ noteId: number; trackId: string; pitch: number }>>()
  let webNoteCounter = 1_000_000
  let midiErrorShown = false

  const monitoredTrack = () => {
    const store = useProjectStore.getState()
    return store.project.tracks.find((track) => track.id === store.selectedTrackId && track.kind === 'instrument')
      ?? store.project.tracks.find((track) => track.kind === 'instrument' && track.armed)
      ?? store.project.tracks.find((track) => track.kind === 'instrument')
  }

  const releaseWebNotes = () => {
    for (const notes of webNotes.values()) {
      for (const note of notes) engine.midiNote(note.trackId, note.noteId, note.pitch, 0, false)
    }
    webNotes.clear()
  }

  const reconcile = async (scanDevices = false) => {
    requestedRevision += 1
    scanRequested ||= scanDevices
    if (checking || cancelled) return
    checking = true
    try {
      while (!cancelled) {
        const revision = requestedRevision
        const shouldScan = scanRequested
        scanRequested = false
        const store = useProjectStore.getState()
        const target = monitoredTrack()
        if (shouldScan) {
          const ports = await engine.listMidiInputs()
          const available = new Map(ports.map((port) => [port.id, port]))
          for (const portId of knownPorts.keys()) {
            if (!available.has(portId)) {
              nativeConnected.delete(portId)
              await engine.disconnectMidiInput(portId).catch(() => undefined)
            }
          }
          knownPorts = available
        }
        if (!target) {
          for (const [portId, port] of knownPorts) {
            if (port.connected) await engine.disconnectMidiInput(portId).catch(() => undefined)
            nativeConnected.delete(portId)
            knownPorts.set(portId, { ...port, connected: false, targetTrackId: null })
          }
        } else {
          for (const [portId, port] of knownPorts) {
            if (cancelled || (port.connected && port.targetTrackId === target.id)) continue
            try {
              if (port.targetTrackId && port.targetTrackId !== target.id) engine.midiAllNotesOff(port.targetTrackId)
              await engine.connectMidiInput(portId, target.id)
              if (nativeConnected.size === 0) releaseWebNotes()
              nativeConnected.add(portId)
              knownPorts.set(portId, { ...port, connected: true, targetTrackId: target.id })
              if (!announced.has(portId)) {
                announced.add(portId)
                store.showToast(`${port.name} MIDI 입력을 ${target.name}에 연결했습니다.`)
              }
            } catch (error) {
              console.warn(`Native MIDI input '${port.name}' could not connect.`, error)
            }
          }
        }
        if (revision === requestedRevision) break
      }
    } catch (error) {
      console.warn('MiniStudio native MIDI unavailable; Web MIDI fallback remains active.', error)
      if (!midiErrorShown) {
        midiErrorShown = true
        useProjectStore.getState().showToast('Windows MIDI 서비스가 응답하지 않아 Web MIDI 입력으로 전환했습니다.')
      }
    } finally {
      checking = false
    }
  }

  void reconcile(true)
  let selectedTrackId = useProjectStore.getState().selectedTrackId
  const unsubscribe = useProjectStore.subscribe((state) => {
    if (state.selectedTrackId === selectedTrackId) return
    selectedTrackId = state.selectedTrackId
    void reconcile()
  })
  const graphChanged = () => void reconcile()
  const devicesChanged = () => void reconcile(true)
  window.addEventListener('ministudio:midi-refresh', devicesChanged)
  window.addEventListener('ministudio:graph-synced', graphChanged)

  let access: MIDIAccess | null = null
  const webMessage = (event: MIDIMessageEvent) => {
    if (cancelled || nativeConnected.size > 0 || !event.data?.length) return
    const target = monitoredTrack()
    if (!target) return
    const status = event.data[0] ?? 0
    const command = status & 0xf0
    if (command !== 0x80 && command !== 0x90) return
    const pitch = Math.min(127, event.data[1] ?? 0)
    const velocity = Math.min(127, event.data[2] ?? 0)
    const source = (event.currentTarget as MIDIInput | null)?.id ?? 'web-midi'
    const key = `${source}:${status & 0x0f}:${pitch}`
    if (command === 0x90 && velocity > 0) {
      const note = { noteId: webNoteCounter++, trackId: target.id, pitch }
      const stack = webNotes.get(key) ?? []
      stack.push(note)
      webNotes.set(key, stack)
      engine.midiNote(note.trackId, note.noteId, pitch, velocity / 127, true)
      return
    }
    const stack = webNotes.get(key)
    const note = stack?.pop()
    if (stack?.length === 0) webNotes.delete(key)
    if (note) engine.midiNote(note.trackId, note.noteId, note.pitch, velocity / 127, false)
  }
  const attachWebInputs = (value: MIDIAccess) => {
    for (const input of value.inputs.values()) input.onmidimessage = webMessage
  }
  void navigator.requestMIDIAccess?.({ sysex: false }).then((value) => {
    if (cancelled) return
    access = value
    attachWebInputs(value)
    value.onstatechange = () => {
      attachWebInputs(value)
      devicesChanged()
    }
  }).catch((error) => console.warn('Web MIDI access unavailable.', error))

  return () => {
    cancelled = true
    unsubscribe()
    window.removeEventListener('ministudio:midi-refresh', devicesChanged)
    window.removeEventListener('ministudio:graph-synced', graphChanged)
    if (access) {
      access.onstatechange = null
      for (const input of access.inputs.values()) input.onmidimessage = null
    }
    releaseWebNotes()
  }
}
