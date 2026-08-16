// CapsLock-gated virtual piano that owns musical key input only while visible.
import { ChevronDown, ChevronUp, Piano, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { TempoMap } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { useProjectStore } from '../store/projectStore'

const KEY_OFFSETS: Record<string, number> = { q: 0, '2': 1, w: 2, '3': 3, e: 4, r: 5, '5': 6, t: 7, '6': 8, y: 9, '7': 10, u: 11 }
const WHITE_KEYS = [{ key: 'q', offset: 0 }, { key: 'w', offset: 2 }, { key: 'e', offset: 4 }, { key: 'r', offset: 5 }, { key: 't', offset: 7 }, { key: 'y', offset: 9 }, { key: 'u', offset: 11 }]
const BLACK_KEYS = [{ key: '2', offset: 1, left: 1 }, { key: '3', offset: 3, left: 2 }, { key: '5', offset: 6, left: 4 }, { key: '6', offset: 8, left: 5 }, { key: '7', offset: 10, left: 6 }]

type HeldNote = { id: number; pitch: number; trackId: string; startSec: number; velocity: number }
type PianoPosition = { x: number; y: number }

export function VirtualPiano() {
  const open = useProjectStore((state) => state.virtualPianoOpen)
  const octave = useProjectStore((state) => state.virtualPianoOctave)
  const velocity = useProjectStore((state) => state.virtualPianoVelocity)
  const selectedTrackId = useProjectStore((state) => state.selectedTrackId)
  const tracks = useProjectStore((state) => state.project.tracks)
  const engine = useEngine()
  const held = useRef(new Map<string, HeldNote>())
  const nextNoteId = useRef(3_000_000)
  const recordingTakeClip = useRef<string | null>(null)
  const windowRef = useRef<HTMLElement>(null)
  const dragRef = useRef<{ pointerId: number; offsetX: number; offsetY: number } | null>(null)
  const [pressed, setPressed] = useState<Set<number>>(() => new Set())
  const [position, setPosition] = useState<PianoPosition | null>(null)
  const selectedTrack = tracks.find((track) => track.id === selectedTrackId && track.kind === 'instrument')
    ?? tracks.find((track) => track.kind === 'instrument')

  const stopNote = useCallback((code: string) => {
    const note = held.current.get(code)
    if (!note) return
    held.current.delete(code)
    engine.midiNote(note.trackId, note.id, note.pitch, 0, false)
    setPressed(new Set([...held.current.values()].map((item) => item.pitch % 12)))
    recordNote(note, recordingTakeClip)
  }, [engine])

  const releaseAll = useCallback(() => {
    for (const code of [...held.current.keys()]) stopNote(code)
    setPressed(new Set())
  }, [stopNote])

  const startNote = useCallback((code: string, offset: number) => {
    if (held.current.has(code)) return
    const store = useProjectStore.getState()
    const track = store.project.tracks.find((item) => item.id === store.selectedTrackId && item.kind === 'instrument')
      ?? store.project.tracks.find((item) => item.kind === 'instrument')
    if (!track) { store.showToast('가상 피아노를 연주할 인스트루먼트 트랙이 없습니다'); return }
    const pitch = Math.max(0, Math.min(127, (store.virtualPianoOctave + 1) * 12 + offset))
    const note = { id: nextNoteId.current++, pitch, trackId: track.id, startSec: store.playheadSec, velocity: store.virtualPianoVelocity }
    held.current.set(code, note)
    engine.midiNote(track.id, note.id, pitch, note.velocity / 127, true)
    setPressed(new Set([...held.current.values()].map((item) => item.pitch % 12)))
  }, [engine])

  useEffect(() => {
    const down = (event: KeyboardEvent) => {
      if (event.key === 'CapsLock') {
        event.preventDefault()
        event.stopImmediatePropagation()
        const next = !useProjectStore.getState().virtualPianoOpen
        if (!next) releaseAll()
        useProjectStore.getState().setVirtualPianoOpen(next)
        return
      }
      const store = useProjectStore.getState()
      if (!store.virtualPianoOpen || isEditable(event.target)) return
      const offset = KEY_OFFSETS[event.key.toLowerCase()]
      if (offset === undefined) return
      event.preventDefault()
      event.stopImmediatePropagation()
      if (!event.repeat) startNote(event.code, offset)
    }
    const up = (event: KeyboardEvent) => {
      if (!held.current.has(event.code)) return
      event.preventDefault()
      event.stopImmediatePropagation()
      stopNote(event.code)
    }
    const blur = () => releaseAll()
    window.addEventListener('keydown', down, true)
    window.addEventListener('keyup', up, true)
    window.addEventListener('blur', blur)
    return () => { releaseAll(); window.removeEventListener('keydown', down, true); window.removeEventListener('keyup', up, true); window.removeEventListener('blur', blur) }
  }, [releaseAll, startNote, stopNote])

  useEffect(() => { if (!open) releaseAll() }, [open, releaseAll])

  const clampPosition = useCallback((x: number, y: number): PianoPosition => {
    const bounds = windowRef.current?.getBoundingClientRect()
    const width = bounds?.width ?? 390
    const height = bounds?.height ?? 125
    return {
      x: Math.max(6, Math.min(window.innerWidth - width - 6, x)),
      y: Math.max(6, Math.min(window.innerHeight - height - 6, y)),
    }
  }, [])

  useEffect(() => {
    if (!open) return
    const frame = requestAnimationFrame(() => setPosition((current) => current
      ? clampPosition(current.x, current.y)
      : clampPosition((window.innerWidth - (windowRef.current?.offsetWidth ?? 390)) / 2, window.innerHeight - (windowRef.current?.offsetHeight ?? 125) - 51)))
    const resize = () => setPosition((current) => current && clampPosition(current.x, current.y))
    window.addEventListener('resize', resize)
    return () => { cancelAnimationFrame(frame); window.removeEventListener('resize', resize) }
  }, [clampPosition, open])

  const beginWindowDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest('button, input')) return
    const bounds = windowRef.current?.getBoundingClientRect()
    if (!bounds) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    dragRef.current = { pointerId: event.pointerId, offsetX: event.clientX - bounds.left, offsetY: event.clientY - bounds.top }
    setPosition({ x: bounds.left, y: bounds.top })
  }
  const moveWindow = (event: React.PointerEvent<HTMLElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    setPosition(clampPosition(event.clientX - drag.offsetX, event.clientY - drag.offsetY))
  }
  const endWindowDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return
    dragRef.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }

  if (!open) return null
  const pointerKey = (offset: number) => `pointer-${offset}`
  const pointerHandlers = (offset: number) => ({
    onPointerDown: (event: React.PointerEvent<HTMLButtonElement>) => { event.currentTarget.setPointerCapture(event.pointerId); startNote(pointerKey(offset), offset) },
    onPointerUp: () => stopNote(pointerKey(offset)),
    onPointerCancel: () => stopNote(pointerKey(offset)),
  })

  return (
    <section ref={windowRef} className="virtual-piano" aria-label="가상 피아노" style={position ? { left: position.x, top: position.y } : undefined}>
      <header onPointerDown={beginWindowDrag} onPointerMove={moveWindow} onPointerUp={endWindowDrag} onPointerCancel={endWindowDrag}><Piano size={14} /><strong>{selectedTrack?.name ?? 'No instrument'}</strong><span>CapsLock · Q 2 W 3 E R 5 T 6 Y 7 U</span><button title="가상 피아노 닫기" onClick={() => { releaseAll(); useProjectStore.getState().setVirtualPianoOpen(false) }}><X size={14} /></button></header>
      <div className="virtual-piano-body">
        <div className="virtual-piano-settings">
          <button title="옥타브 내리기" onClick={() => useProjectStore.getState().setVirtualPianoOctave(octave - 1)}><ChevronDown size={13} /></button>
          <b>C{octave}</b>
          <button title="옥타브 올리기" onClick={() => useProjectStore.getState().setVirtualPianoOctave(octave + 1)}><ChevronUp size={13} /></button>
          <label>VEL <input type="range" min="1" max="127" value={velocity} onChange={(event) => useProjectStore.getState().setVirtualPianoVelocity(Number(event.target.value))} />{velocity}</label>
        </div>
        <div className="virtual-piano-keys">
          {WHITE_KEYS.map((item) => <button key={item.key} className={pressed.has(item.offset) ? 'white pressed' : 'white'} {...pointerHandlers(item.offset)}><span>{item.key.toUpperCase()}</span></button>)}
          {BLACK_KEYS.map((item) => <button key={item.key} className={pressed.has(item.offset) ? 'black pressed' : 'black'} style={{ '--black-left': item.left } as React.CSSProperties} {...pointerHandlers(item.offset)}><span>{item.key}</span></button>)}
        </div>
      </div>
    </section>
  )
}

function recordNote(note: HeldNote, recordingTakeClip: React.MutableRefObject<string | null>): void {
  const store = useProjectStore.getState()
  const target = store.project.tracks.find((track) => track.id === note.trackId)
  if (!store.recordingEnabled || !store.project.transport.isPlaying || store.countInActive || !target?.armed) return
  const endSec = Math.max(note.startSec + 0.01, store.playheadSec)
  const tempoMap = new TempoMap(store.project.transport.tempoMap)
  const noteStartTick = tempoMap.secondsToTicks(note.startSec)
  const noteBar = tempoMap.tickToBarBeat(noteStartTick).bar
  const barStartTick = tempoMap.barStartTicks(noteBar)
  const nextBarTick = tempoMap.barStartTicks(noteBar + 1)
  const barStartSec = tempoMap.ticksToSeconds(barStartTick)
  const barSec = Math.max(0.01, tempoMap.ticksToSeconds(nextBarTick) - barStartSec)
  let clip = store.overdubMode === 'new'
    ? target.midiClips.find((item) => item.id === recordingTakeClip.current)
    : target.midiClips.find((item) => note.startSec >= item.startSec && note.startSec < item.startSec + item.durationSec)
  if (!clip) {
    const fourBarsEndSec = tempoMap.ticksToSeconds(tempoMap.barStartTicks(noteBar + 4))
    const clipId = store.addMidiClip(note.trackId, barStartSec, Math.max(barSec, fourBarsEndSec - barStartSec))
    recordingTakeClip.current = clipId
    clip = useProjectStore.getState().project.tracks.find((track) => track.id === note.trackId)?.midiClips.find((item) => item.id === clipId)
  }
  if (!clip) return
  const required = endSec - clip.startSec
  if (required > clip.durationSec) store.updateMidiClip(note.trackId, clip.id, { durationSec: required + barSec })
  const clipStartTick = tempoMap.secondsToTicks(clip.startSec)
  const noteEndTick = tempoMap.secondsToTicks(endSec)
  store.addMidiNote(note.trackId, clip.id, {
    pitch: note.pitch,
    velocity: note.velocity,
    startTicks: Math.max(0, noteStartTick - clipStartTick),
    lengthTicks: Math.max(1, noteEndTick - noteStartTick),
    releaseVelocity: 64,
    muted: false,
  })
}

function isEditable(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
}
