// Canvas piano roll sharing the arrangement tool system and realtime test-tone preview.
import { Activity, ChevronDown, Drum, Eraser, Gauge, Magnet, Maximize2, Minimize2, MousePointer2, Pencil, Piano, Plus, Scissors, Volume2, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { GRID_OPTIONS, MIDI_PITCH_BEND_LANE, MIDI_PPQ, secondsPerBeat, type CcLane, type MidiNote } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { snapTicksWithSwing, useProjectStore } from '../store/projectStore'
import { useToolStore, type ToolId } from '../store/toolStore'
import { FloatingPanel, MenuPanel, type MenuItem } from './Menu'
import { EditableNumber } from './controls'
import { findVisibleNoteStart, pianoPitchAtClientY } from './pianoRollMath'
import { buildRulerTicks } from './rulerMath'

const GUTTER = 52
const EVENT_LANE_HEIGHT = 74
const PIANO_RULER_HEIGHT = 24
const SCALES: Record<string, number[]> = { Major: [0, 2, 4, 5, 7, 9, 11], 'Natural Minor': [0, 2, 3, 5, 7, 8, 10], 'Harmonic Minor': [0, 2, 3, 5, 7, 8, 11], Dorian: [0, 2, 3, 5, 7, 9, 10], Phrygian: [0, 1, 3, 5, 7, 8, 10], Lydian: [0, 2, 4, 6, 7, 9, 11], Mixolydian: [0, 2, 4, 5, 7, 9, 10], Locrian: [0, 1, 3, 5, 6, 8, 10], 'Pentatonic Major': [0, 2, 4, 7, 9], 'Pentatonic Minor': [0, 3, 5, 7, 10], Blues: [0, 3, 5, 6, 7, 10] }

type Drag = { mode: 'move' | 'resize' | 'create'; note: MidiNote; startX: number; startY: number; latestTick: number; latestPitch: number; latestLength: number; copy: boolean }
type Marquee = { x0: number; y0: number; x1: number; y1: number }
type NoteMenu = { x: number; y: number; tick: number; pitch: number; note: MidiNote | null }
type EventLane = 'velocity' | 'releaseVelocity' | 'sustain' | 'vibrato' | 'pitchBend' | 'cc'
type PianoPaintShape = 'freehand' | 'line' | 'parabola' | 'square' | 'triangle' | 'saw' | 'sine' | 'transform'
type NeighborClipRange = { id: string; name: string; startTicks: number; endTicks: number }
const EVENT_LANE_OPTIONS: Array<[EventLane, string]> = [
  ['velocity', 'Velocity'], ['releaseVelocity', 'Release'], ['sustain', 'Pedal'],
  ['vibrato', 'Vibrato'], ['pitchBend', 'Pitch Bend'], ['cc', 'CC'],
]
const DEFAULT_EVENT_LANES: EventLane[] = ['velocity', 'sustain', 'pitchBend']
const GM_DRUM_NAMES: Record<number, string> = {
  35: 'Ac. Kick', 36: 'Kick', 37: 'Side Stick', 38: 'Snare', 39: 'Clap', 40: 'E. Snare',
  41: 'Low Tom', 42: 'Closed HH', 43: 'Low Tom 2', 44: 'Pedal HH', 45: 'Mid Tom', 46: 'Open HH',
  47: 'Mid Tom 2', 48: 'High Tom', 49: 'Crash 1', 50: 'High Tom 2', 51: 'Ride 1', 52: 'China',
  53: 'Ride Bell', 54: 'Tambourine', 55: 'Splash', 56: 'Cowbell', 57: 'Crash 2', 58: 'Vibraslap',
  59: 'Ride 2', 60: 'High Bongo', 61: 'Low Bongo', 62: 'Mute Conga', 63: 'High Conga', 64: 'Low Conga',
  65: 'High Timbale', 66: 'Low Timbale', 67: 'High Agogo', 68: 'Low Agogo', 69: 'Cabasa', 70: 'Maracas',
  71: 'Short Whistle', 72: 'Long Whistle', 73: 'Short Guiro', 74: 'Long Guiro', 75: 'Claves', 76: 'High Wood',
  77: 'Low Wood', 78: 'Mute Cuica', 79: 'Open Cuica', 80: 'Mute Triangle', 81: 'Open Triangle',
}

export function PianoRoll() {
  const editor = useProjectStore((state) => state.editorClip)
  const tracks = useProjectStore((state) => state.project.tracks)
  const selected = useProjectStore((state) => state.selectedNoteIds)
  const maximized = useProjectStore((state) => state.editorMaximized)
  const focused = useProjectStore((state) => state.editFocus === 'pianoRoll')
  const setEditFocus = useProjectStore((state) => state.setEditFocus)
  const engine = useEngine()
  const track = editor ? tracks.find((item) => item.id === editor.trackId) : undefined
  const clip = editor ? track?.midiClips.find((item) => item.id === editor.clipId) : undefined
  const gridTicks = useProjectStore((state) => state.pianoGridTicks)
  const setGridTicks = useProjectStore((state) => state.setPianoGridTicks)
  const snapEnabled = useProjectStore((state) => state.pianoSnapEnabled)
  const quantizeSwing = useProjectStore((state) => state.pianoSwing)
  const signature = useProjectStore((state) => state.project.transport.timeSignature)
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const setQuantizeSwing = useProjectStore((state) => state.setPianoSwing)
  const pixelsPerQuarter = useProjectStore((state) => state.pianoRollZoom)
  const setPixelsPerQuarter = useProjectStore((state) => state.setPianoRollZoom)
  const [noteHeight, setNoteHeight] = useState(12)
  const pianoTool = useToolStore((state) => state.pianoTool)
  const setPianoTool = useToolStore((state) => state.choosePianoTool)
  const [rollMode, setRollMode] = useState<'piano' | 'drum'>('piano')
  const [paintShape, setPaintShape] = useState<PianoPaintShape>('freehand')
  const [paintMenuOpen, setPaintMenuOpen] = useState(false)
  const paintButton = useRef<HTMLButtonElement>(null)
  const [canvasWidth, setCanvasWidth] = useState(1000)
  const [preview, setPreview] = useState(true)
  const [keyboardVelocity, setKeyboardVelocity] = useState(100)
  const [scaleRoot, setScaleRoot] = useState(0)
  const [scaleName, setScaleName] = useState('Major')
  const [scaleSnap, setScaleSnap] = useState(false)
  const [quantizeStrength, setQuantizeStrength] = useState(100)
  const [quantizeEnds, setQuantizeEnds] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const horizontalScrollRef = useRef<HTMLDivElement>(null)
  const backgroundRef = useRef<HTMLCanvasElement>(null)
  const notesRef = useRef<HTMLCanvasElement>(null)
  const overlayRef = useRef<HTMLCanvasElement>(null)
  const eventCanvasRef = useRef<HTMLCanvasElement>(null)
  const dragRef = useRef<Drag | null>(null)
  const marqueeRef = useRef<Marquee | null>(null)
  const marqueeBaseRef = useRef<string[]>([])
  const [marquee, setMarquee] = useState<Marquee | null>(null)
  const [noteMenu, setNoteMenu] = useState<NoteMenu | null>(null)
  const [noteCursor, setNoteCursor] = useState('default')
  const [eventLane, setEventLane] = useState<EventLane>('velocity')
  const [visibleEventLanes, setVisibleEventLanes] = useState<EventLane[]>(DEFAULT_EVENT_LANES)
  const [eventPickerOpen, setEventPickerOpen] = useState(false)
  const eventAddButton = useRef<HTMLButtonElement>(null)
  const [genericCc, setGenericCc] = useState(11)
  const centredClipId = useRef<string | null>(null)
  const auditionId = useRef(2_000_000)
  const activeAuditions = useRef(new Map<number, { trackId: string; pitch: number; timer: number | null }>())
  const lastNoteLengthRef = useRef(gridTicks)
  const pixelsPerTick = pixelsPerQuarter / MIDI_PPQ
  const maxTicks = useMemo(() => {
    const barTicks = Math.max(1, Math.round(MIDI_PPQ * 4 * signature.numerator / signature.denominator))
    const contentEnd = Math.max(0, clip?.loopLengthTicks ?? 0, ...(clip?.notes.map((note) => note.startTicks + note.lengthTicks) ?? []), ...(clip?.ccLanes.flatMap((lane) => lane.points.map((point) => point.ticks)) ?? []))
    const expanded = Math.ceil((contentEnd + barTicks * 4) / (barTicks * 4)) * barTicks * 4
    return Math.max(barTicks * 172, expanded)
  }, [clip?.ccLanes, clip?.loopLengthTicks, clip?.notes, signature.denominator, signature.numerator])
  const contentWidth = Math.max(1000, Math.ceil(maxTicks * pixelsPerTick))
  const contentHeight = 128 * noteHeight
  const activeTicks = useMemo(() => Math.max(1, Math.round((clip?.durationSec ?? 0) / secondsPerBeat(bpm) * MIDI_PPQ)), [bpm, clip?.durationSec])
  const neighboringClips = useMemo<NeighborClipRange[]>(() => {
    if (!clip || !track) return []
    const ticksPerSecond = MIDI_PPQ / secondsPerBeat(bpm)
    return track.midiClips.filter((item) => item.id !== clip.id).map((item) => ({
      id: item.id,
      name: item.name,
      startTicks: Math.round((item.startSec - clip.startSec) * ticksPerSecond),
      endTicks: Math.round((item.startSec + item.durationSec - clip.startSec) * ticksPerSecond),
    })).filter((item) => item.endTicks > activeTicks && item.endTicks > 0)
  }, [activeTicks, bpm, clip, track])
  const activeCc = eventLane === 'pitchBend' ? MIDI_PITCH_BEND_LANE : eventLane === 'sustain' ? 64 : eventLane === 'vibrato' ? 1 : genericCc
  const activeControllerLane = clip?.ccLanes.find((lane) => lane.cc === activeCc)

  const stopAudition = useCallback((id: number | null) => {
    if (id === null) return
    const active = activeAuditions.current.get(id)
    if (!active) return
    if (active.timer !== null) window.clearTimeout(active.timer)
    activeAuditions.current.delete(id)
    engine.midiNote(active.trackId, id, active.pitch, 0, false)
  }, [engine])
  const startAudition = useCallback((pitch: number, durationMs: number | null) => {
    if (!track || track.kind !== 'instrument' || !track.instrument || !preview) return null
    const id = auditionId.current++
    engine.midiNote(track.id, id, pitch, keyboardVelocity / 127, true)
    activeAuditions.current.set(id, { trackId: track.id, pitch, timer: null })
    if (durationMs !== null) {
      const timer = window.setTimeout(() => stopAudition(id), durationMs)
      activeAuditions.current.set(id, { trackId: track.id, pitch, timer })
    }
    return id
  }, [engine, keyboardVelocity, preview, stopAudition, track])
  const audition = useCallback((pitch: number) => { startAudition(pitch, 180) }, [startAudition])
  const beginHeldAudition = useCallback((pitch: number) => startAudition(pitch, null), [startAudition])
  const endHeldAudition = useCallback((id: number | null) => stopAudition(id), [stopAudition])

  // A clip/track switch or closing the piano roll must never leave an audition
  // voice held in the native engine or an isolated plug-in process.
  useEffect(() => () => {
    for (const id of [...activeAuditions.current.keys()]) stopAudition(id)
  }, [stopAudition, track?.id])

  // Opening a clip used to land at pitch 127, so every clip needed a manual scroll.
  // Centre on the clip's own material instead, falling back to C4 when it is empty.
  useEffect(() => {
    const scroll = scrollRef.current
    if (!clip || !scroll || centredClipId.current === clip.id) return
    centredClipId.current = clip.id
    const pitches = clip.notes.map((note) => note.pitch)
    const focusPitch = pitches.length ? (Math.min(...pitches) + Math.max(...pitches)) / 2 : 60
    scroll.scrollTop = Math.max(0, (127 - focusPitch) * noteHeight - scroll.clientHeight / 2)
    scroll.scrollLeft = Math.max(0, (clip.notes[0]?.startTicks ?? 0) * pixelsPerTick - 80)
  }, [clip, noteHeight, pixelsPerTick])
  useEffect(() => {
    const main = scrollRef.current
    const horizontal = horizontalScrollRef.current
    if (!main || !horizontal) return
    let syncing = false
    const fromMain = () => { if (syncing) return; syncing = true; horizontal.scrollLeft = main.scrollLeft; syncing = false }
    const fromHorizontal = () => { if (syncing) return; syncing = true; main.scrollLeft = horizontal.scrollLeft; syncing = false }
    main.addEventListener('scroll', fromMain, { passive: true })
    horizontal.addEventListener('scroll', fromHorizontal, { passive: true })
    return () => { main.removeEventListener('scroll', fromMain); horizontal.removeEventListener('scroll', fromHorizontal) }
  }, [contentWidth])
  useEffect(() => {
    const scroll = scrollRef.current
    if (!scroll) return
    const resize = () => setCanvasWidth(Math.max(320, scroll.clientWidth - GUTTER))
    const observer = new ResizeObserver(resize)
    observer.observe(scroll); resize()
    return () => observer.disconnect()
  }, [])
  useEffect(() => {
    const scroll = scrollRef.current
    let frame = 0
    const draw = () => drawGrid(backgroundRef.current, canvasWidth, contentHeight, noteHeight, pixelsPerTick, gridTicks, Math.round(MIDI_PPQ * 4 * signature.numerator / signature.denominator), scaleRoot, scaleName, scroll?.scrollLeft ?? 0, activeTicks, neighboringClips)
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    scroll?.addEventListener('scroll', schedule, { passive: true }); schedule()
    return () => { cancelAnimationFrame(frame); scroll?.removeEventListener('scroll', schedule) }
  }, [activeTicks, canvasWidth, contentHeight, gridTicks, neighboringClips, noteHeight, pixelsPerTick, scaleName, scaleRoot, signature.denominator, signature.numerator])

  useEffect(() => {
    if (!clip) return
    const scroll = scrollRef.current
    let frame = requestAnimationFrame(() => drawNotes(notesRef.current, clip.notes, selected, canvasWidth, contentHeight, noteHeight, pixelsPerTick, scroll, rollMode === 'drum'))
    const onScroll = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => drawNotes(notesRef.current, clip.notes, selected, canvasWidth, contentHeight, noteHeight, pixelsPerTick, scroll, rollMode === 'drum'))
    }
    scroll?.addEventListener('scroll', onScroll, { passive: true })
    return () => { cancelAnimationFrame(frame); scroll?.removeEventListener('scroll', onScroll) }
  }, [canvasWidth, clip, contentHeight, noteHeight, pixelsPerTick, rollMode, selected])

  useEffect(() => {
    if (!clip) return
    let frame = 0
    let previousX = Number.NaN
    let previousScroll = Number.NaN
    const animate = () => {
      const state = useProjectStore.getState()
      const x = (state.playheadSec - clip.startSec) * state.project.transport.bpm / 60 * MIDI_PPQ * pixelsPerTick
      const scrollLeft = scrollRef.current?.scrollLeft ?? 0
      if (x !== previousX || scrollLeft !== previousScroll || dragRef.current) {
        previousX = x
        previousScroll = scrollLeft
        drawOverlay(overlayRef.current, canvasWidth, contentHeight, x, dragRef.current, noteHeight, pixelsPerTick, scrollLeft, activeTicks * pixelsPerTick)
      }
      frame = requestAnimationFrame(animate)
    }
    frame = requestAnimationFrame(animate)
    return () => cancelAnimationFrame(frame)
  }, [activeTicks, canvasWidth, clip, contentHeight, noteHeight, pixelsPerTick])

  useEffect(() => {
    if (!clip) return
    const scroll = scrollRef.current
    let frame = 0
    const draw = () => drawEventLane(eventCanvasRef.current, eventLane, activeControllerLane, clip.notes, selected, pixelsPerTick, gridTicks, scroll?.scrollLeft ?? 0)
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const resize = new ResizeObserver(schedule)
    if (eventCanvasRef.current) resize.observe(eventCanvasRef.current)
    scroll?.addEventListener('scroll', schedule, { passive: true })
    schedule()
    return () => { cancelAnimationFrame(frame); resize.disconnect(); scroll?.removeEventListener('scroll', schedule) }
  }, [activeControllerLane, clip, eventLane, gridTicks, pixelsPerTick, selected])

  const noteAt = (x: number, y: number): MidiNote | undefined => {
    if (!clip) return undefined
    const tick = x / pixelsPerTick
    const pitch = 127 - Math.floor(y / noteHeight)
    const start = lowerBound(clip.notes, Math.max(0, tick - 4000))
    for (let index = start; index < clip.notes.length; index += 1) {
      const note = clip.notes[index]!
      if (note.startTicks > tick) break
      if (note.pitch === pitch && tick <= note.startTicks + note.lengthTicks) return note
    }
  }
  const snapTick = (tick: number, bypass = false) => Math.max(0, bypass || !snapEnabled ? Math.round(tick) : snapTicksWithSwing(tick, gridTicks, quantizeSwing))
  const snapPitch = (pitch: number) => scaleSnap ? nearestScalePitch(pitch, scaleRoot, SCALES[scaleName] ?? SCALES.Major!) : Math.max(0, Math.min(127, pitch))
  const point = (event: React.PointerEvent<HTMLCanvasElement>) => { const rect = event.currentTarget.getBoundingClientRect(); return { x: event.clientX - rect.left + (scrollRef.current?.scrollLeft ?? 0), y: event.clientY - rect.top } }
  const activateNeighborAtTick = (ticks: number) => {
    if (ticks < activeTicks || !track) return false
    const neighbor = neighboringClips.find((item) => ticks >= item.startTicks && ticks <= item.endTicks)
    if (!neighbor) return false
    const store = useProjectStore.getState()
    store.selectClip(neighbor.id)
    store.openMidiEditor(track.id, neighbor.id)
    return true
  }

  const onPointerDown = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!editor || !clip) return
    if (event.button !== 0) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    const { x, y } = point(event)
    const hit = noteAt(x, y)
    const tool = pianoTool
    if (!hit && activateNeighborAtTick(x / pixelsPerTick)) return
    if (!hit && (tool === 'paint' || ((event.ctrlKey || event.metaKey) && (tool === 'arrow' || tool === 'range')))) {
      const pitch = snapPitch(127 - Math.floor(y / noteHeight))
      const anchor = snapTick(x / pixelsPerTick, event.shiftKey)
      const id = useProjectStore.getState().addMidiNote(editor.trackId, editor.clipId, { pitch, velocity: keyboardVelocity, startTicks: anchor, lengthTicks: gridTicks, releaseVelocity: 64, muted: false })
      if (!id) return
      const heldAuditionId = beginHeldAudition(pitch)
      const note: MidiNote = { id, pitch, velocity: keyboardVelocity, startTicks: anchor, lengthTicks: gridTicks, releaseVelocity: 64, muted: false }
      dragRef.current = { mode: 'create', note, startX: x, startY: y, latestTick: anchor, latestPitch: pitch, latestLength: gridTicks, copy: false }
      setNoteCursor('ew-resize')
      const move = (pointer: PointerEvent) => {
        const drag = dragRef.current; if (!drag) return
        const current = snapTick(anchor + (pointer.clientX - event.clientX) / pixelsPerTick, pointer.shiftKey)
        drag.latestTick = Math.min(anchor, current)
        drag.latestLength = Math.max(pointer.shiftKey ? 1 : gridTicks, Math.abs(current - anchor) || gridTicks)
        drawOverlay(overlayRef.current, canvasWidth, contentHeight, -1, drag, noteHeight, pixelsPerTick, scrollRef.current?.scrollLeft ?? 0, activeTicks * pixelsPerTick)
      }
      const up = () => {
        const drag = dragRef.current; dragRef.current = null; setNoteCursor('default')
        window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); window.removeEventListener('pointercancel', up)
        endHeldAudition(heldAuditionId)
        if (drag) {
          lastNoteLengthRef.current = drag.latestLength
          useProjectStore.getState().updateMidiNotes(editor.trackId, editor.clipId, [id], { startTicks: drag.latestTick, lengthTicks: drag.latestLength })
        }
      }
      window.addEventListener('pointermove', move); window.addEventListener('pointerup', up, { once: true }); window.addEventListener('pointercancel', up, { once: true })
      return
    }
    if (!hit) {
      // Rubber-band select. Without this, multi-note edits needed Shift+click on
      // every single note, which is unusable on a dense pattern.
      if (!event.shiftKey && !event.ctrlKey && !event.metaKey) useProjectStore.getState().clearNoteSelection()
      const additive = event.shiftKey || event.ctrlKey || event.metaKey
      const base = additive ? useProjectStore.getState().selectedNoteIds : []
      marqueeBaseRef.current = base
      marqueeRef.current = { x0: x, y0: y, x1: x, y1: y }
      setMarquee(marqueeRef.current)
      return
    }
    if (tool === 'erase') { useProjectStore.getState().deleteMidiNotes(editor.trackId, editor.clipId, [hit.id]); return }
    if (tool === 'mute') { useProjectStore.getState().updateMidiNotes(editor.trackId, editor.clipId, [hit.id], { muted: !hit.muted }); return }
    if (tool === 'split') { const at = snapTick(x / pixelsPerTick, event.shiftKey); if (at > hit.startTicks && at < hit.startTicks + hit.lengthTicks) { useProjectStore.getState().updateMidiNotes(editor.trackId, editor.clipId, [hit.id], { lengthTicks: at - hit.startTicks }); useProjectStore.getState().addMidiNote(editor.trackId, editor.clipId, { ...hit, startTicks: at, lengthTicks: hit.startTicks + hit.lengthTicks - at, releaseVelocity: hit.releaseVelocity, muted: hit.muted }) }; return }
    if (tool === 'listen') { audition(hit.pitch); return }
    const store = useProjectStore.getState()
    const additive = event.shiftKey || event.ctrlKey || event.metaKey
    if (!store.selectedNoteIds.includes(hit.id) || additive) store.selectMidiNote(hit.id, additive)
    audition(hit.pitch)
    const resize = Math.abs(x - (hit.startTicks + hit.lengthTicks) * pixelsPerTick) <= 7
    setNoteCursor(resize ? 'ew-resize' : 'grabbing')
    dragRef.current = { mode: resize ? 'resize' : 'move', note: { ...hit }, startX: x, startY: y, latestTick: hit.startTicks, latestPitch: hit.pitch, latestLength: hit.lengthTicks, copy: event.altKey && !resize }
    const move = (pointer: PointerEvent) => {
      const drag = dragRef.current; if (!drag) return
      if (drag.mode === 'resize') drag.latestLength = Math.max(pointer.shiftKey ? 1 : gridTicks, snapTick(drag.note.lengthTicks + (pointer.clientX - event.clientX) / pixelsPerTick, pointer.shiftKey))
      else { const nextTick = snapTick(drag.note.startTicks + (pointer.clientX - event.clientX) / pixelsPerTick, pointer.shiftKey); const nextPitch = snapPitch(drag.note.pitch - Math.round((pointer.clientY - event.clientY) / noteHeight)); if (nextPitch !== drag.latestPitch || nextTick !== drag.latestTick) audition(nextPitch); drag.latestTick = nextTick; drag.latestPitch = nextPitch }
      drawOverlay(overlayRef.current, canvasWidth, contentHeight, -1, drag, noteHeight, pixelsPerTick, scrollRef.current?.scrollLeft ?? 0, activeTicks * pixelsPerTick)
    }
    const up = () => {
      const drag = dragRef.current; dragRef.current = null
      setNoteCursor('default')
      window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up)
      if (!drag) return
      const ids = useProjectStore.getState().selectedNoteIds.includes(drag.note.id) ? useProjectStore.getState().selectedNoteIds : [drag.note.id]
      const currentClip = useProjectStore.getState().project.tracks.find((item) => item.id === editor.trackId)?.midiClips.find((item) => item.id === editor.clipId)
      if (drag.mode === 'resize') { const delta = drag.latestLength - drag.note.lengthTicks; if (delta !== 0) { lastNoteLengthRef.current = Math.max(1, drag.latestLength); useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, (currentClip?.notes.filter((item) => ids.includes(item.id)) ?? []).map((note) => ({ id: note.id, patch: { lengthTicks: Math.max(1, note.lengthTicks + delta) } }))) } }
      else {
        const deltaTick = drag.latestTick - drag.note.startTicks
        const deltaPitch = drag.latestPitch - drag.note.pitch
        if (deltaTick === 0 && deltaPitch === 0) return
        if (drag.copy) useProjectStore.getState().duplicateMidiNotes(editor.trackId, editor.clipId, ids, deltaTick, deltaPitch)
        else useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, (currentClip?.notes.filter((item) => ids.includes(item.id)) ?? []).map((note) => ({ id: note.id, patch: { startTicks: Math.max(0, note.startTicks + deltaTick), pitch: snapPitch(note.pitch + deltaPitch) } })))
      }
    }
    window.addEventListener('pointermove', move); window.addEventListener('pointerup', up, { once: true })
  }

  const onNoteCanvasDoubleClick = (event: React.MouseEvent<HTMLCanvasElement>) => {
    if (!editor || !clip || pianoTool === 'paint') return
    const rect = event.currentTarget.getBoundingClientRect()
    const x = event.clientX - rect.left + (scrollRef.current?.scrollLeft ?? 0)
    const y = event.clientY - rect.top
    if (noteAt(x, y) || activateNeighborAtTick(x / pixelsPerTick)) return
    const pitch = snapPitch(127 - Math.floor(y / noteHeight))
    const startTicks = snapTick(x / pixelsPerTick, event.shiftKey)
    const lengthTicks = Math.max(1, lastNoteLengthRef.current)
    const id = useProjectStore.getState().addMidiNote(editor.trackId, editor.clipId, { pitch, velocity: keyboardVelocity, startTicks, lengthTicks, releaseVelocity: 64, muted: false })
    if (id) audition(pitch)
  }

  const onOverlayPointerMove = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const box = marqueeRef.current
    if (!box) {
      const { x, y } = point(event)
      const hit = noteAt(x, y)
      const tool = pianoTool
      const atResizeEdge = hit && Math.abs(x - (hit.startTicks + hit.lengthTicks) * pixelsPerTick) <= 7
      setNoteCursor(atResizeEdge && (tool === 'arrow' || tool === 'range') ? 'ew-resize' : pianoCursorFor(tool, Boolean(hit)))
      return
    }
    const { x, y } = point(event)
    box.x1 = x
    box.y1 = y
    setMarquee({ ...box })
  }

  const finishMarquee = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const box = marqueeRef.current
    if (!box || !clip) return
    marqueeRef.current = null
    setMarquee(null)
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
    const startTick = Math.min(box.x0, box.x1) / pixelsPerTick
    const endTick = Math.max(box.x0, box.x1) / pixelsPerTick
    const highPitch = Math.min(127, 127 - Math.floor(Math.min(box.y0, box.y1) / noteHeight))
    const lowPitch = Math.max(0, 127 - Math.floor(Math.max(box.y0, box.y1) / noteHeight))
    const inside = clip.notes.filter((note) => note.pitch >= lowPitch && note.pitch <= highPitch && note.startTicks + note.lengthTicks >= startTick && note.startTicks <= endTick)
    useProjectStore.setState({ selectedNoteIds: [...new Set([...marqueeBaseRef.current, ...inside.map((note) => note.id)])] })
  }

  /** Note attributes paint bars; controller lanes paint quantized MIDI events. */
  const onEventPointerDown = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!editor || !clip) return
    if (event.button !== 0) return
    event.preventDefault()
    const canvas = event.currentTarget
    canvas.setPointerCapture(event.pointerId)
    const rect = canvas.getBoundingClientRect()
    const touchedNotes = new Map<string, number>()
    const touchedPoints = new Map<number, number>()
    const origin = { x: event.clientX, y: event.clientY }
    let latest = origin
    const continuous = eventLane !== 'velocity' && eventLane !== 'releaseVelocity' && eventLane !== 'sustain'
    const shaped = pianoTool === 'paint' && paintShape !== 'freehand' && paintShape !== 'transform' && continuous
    const apply = (clientX: number, clientY: number) => {
      const x = clientX - rect.left + (scrollRef.current?.scrollLeft ?? 0)
      const y = Math.max(0, Math.min(EVENT_LANE_HEIGHT, clientY - rect.top))
      if (eventLane === 'velocity' || eventLane === 'releaseVelocity') {
        const value = Math.max(eventLane === 'velocity' ? 1 : 0, Math.min(127, Math.round((1 - y / EVENT_LANE_HEIGHT) * 127)))
        for (const note of clip.notes) {
          const noteX = note.startTicks * pixelsPerTick
          if (x >= noteX - 4 && x <= noteX + Math.max(8, note.lengthTicks * pixelsPerTick)) touchedNotes.set(note.id, value)
        }
        if (touchedNotes.size) {
          const key = eventLane
          useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, [...touchedNotes].map(([id, value]) => ({ id, patch: { [key]: value } })))
        }
        return
      }
      const ticks = snapTick(x / pixelsPerTick, event.shiftKey)
      const value = eventLane === 'pitchBend'
        ? Math.max(-8192, Math.min(8191, Math.round((.5 - y / EVENT_LANE_HEIGHT) * 16_384)))
        : eventLane === 'sustain'
          ? y < EVENT_LANE_HEIGHT / 2 ? 127 : 0
          : Math.max(0, Math.min(127, Math.round((1 - y / EVENT_LANE_HEIGHT) * 127)))
      touchedPoints.set(ticks, value)
      useProjectStore.getState().upsertMidiControlPoints(editor.trackId, editor.clipId, activeCc, [...touchedPoints].map(([ticks, value]) => ({ ticks, value })))
    }
    if (!shaped) apply(event.clientX, event.clientY)
    const move = (pointer: PointerEvent) => { latest = { x: pointer.clientX, y: pointer.clientY }; if (!shaped) apply(pointer.clientX, pointer.clientY) }
    const up = () => {
      if (shaped) {
        const scrollLeft = scrollRef.current?.scrollLeft ?? 0
        const x0 = origin.x - rect.left + scrollLeft; const x1 = latest.x - rect.left + scrollLeft
        const y0 = Math.max(0, Math.min(EVENT_LANE_HEIGHT, origin.y - rect.top)); const y1 = Math.max(0, Math.min(EVENT_LANE_HEIGHT, latest.y - rect.top))
        const from = Math.min(x0, x1); const to = Math.max(x0, x1); const reversed = x1 < x0
        const rawValue = (y: number) => eventLane === 'pitchBend' ? Math.max(-8192, Math.min(8191, Math.round((.5 - y / EVENT_LANE_HEIGHT) * 16_384))) : Math.max(0, Math.min(127, Math.round((1 - y / EVENT_LANE_HEIGHT) * 127)))
        const startValue = rawValue(reversed ? y1 : y0); const endValue = rawValue(reversed ? y0 : y1)
        const count = Math.max(2, Math.min(128, Math.ceil((to - from) / Math.max(3, gridTicks * pixelsPerTick))))
        const points = new Map<number, number>()
        for (let index = 0; index <= count; index += 1) {
          const t = index / count
          const value = shapedMidiValue(paintShape, t, startValue, endValue, eventLane === 'pitchBend' ? -8192 : 0, eventLane === 'pitchBend' ? 8191 : 127)
          points.set(snapTick((from + (to - from) * t) / pixelsPerTick, event.shiftKey), Math.round(value))
        }
        useProjectStore.getState().upsertMidiControlPoints(editor.trackId, editor.clipId, activeCc, [...points].map(([ticks, value]) => ({ ticks, value })))
      }
      canvas.releasePointerCapture(event.pointerId)
      canvas.removeEventListener('pointermove', move)
      canvas.removeEventListener('pointerup', up)
      canvas.removeEventListener('pointercancel', up)
    }
    canvas.addEventListener('pointermove', move)
    canvas.addEventListener('pointerup', up)
    canvas.addEventListener('pointercancel', up)
  }

  const onEventContextMenu = (event: React.MouseEvent<HTMLCanvasElement>) => {
    if (!editor || !activeControllerLane?.points.length || eventLane === 'velocity' || eventLane === 'releaseVelocity') return
    event.preventDefault()
    const rect = event.currentTarget.getBoundingClientRect()
    const ticks = (event.clientX - rect.left + (scrollRef.current?.scrollLeft ?? 0)) / pixelsPerTick
    const closest = activeControllerLane.points.reduce((best, point) => Math.abs(point.ticks - ticks) < Math.abs(best.ticks - ticks) ? point : best)
    if (Math.abs(closest.ticks - ticks) * pixelsPerTick <= 10) useProjectStore.getState().removeMidiControlPoint(editor.trackId, editor.clipId, activeCc, closest.ticks)
  }

  const quantize = () => {
    if (!editor || !clip) return
    const ids = selected.length ? selected : clip.notes.map((note) => note.id)
    const updates = clip.notes.filter((item) => ids.includes(item.id)).map((note) => {
      const target = snapTicksWithSwing(note.startTicks, gridTicks, quantizeSwing)
      const startTicks = Math.round(note.startTicks + (target - note.startTicks) * quantizeStrength / 100)
      if (!quantizeEnds) return { id: note.id, patch: { startTicks } }
      const oldEnd = note.startTicks + note.lengthTicks
      const targetEnd = snapTicksWithSwing(oldEnd, gridTicks, quantizeSwing)
      const end = Math.round(oldEnd + (targetEnd - oldEnd) * quantizeStrength / 100)
      return { id: note.id, patch: { startTicks, lengthTicks: Math.max(1, end - startTicks) } }
    })
    useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, updates)
  }

  const transposeMenuNotes = (note: MidiNote, semitones: number) => {
    if (!editor || !clip) return
    const ids = selected.includes(note.id) ? new Set(selected) : new Set([note.id])
    useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, clip.notes.filter((item) => ids.has(item.id)).map((item) => ({ id: item.id, patch: { pitch: Math.max(0, Math.min(127, item.pitch + semitones)) } })))
  }

  const noteMenuItems: MenuItem[] = noteMenu?.note && editor && clip ? [
    { kind: 'item', label: '노트 미리듣기', run: () => audition(noteMenu.note!.pitch) },
    { kind: 'item', label: '한 옥타브 위', run: () => transposeMenuNotes(noteMenu.note!, 12) },
    { kind: 'item', label: '한 옥타브 아래', run: () => transposeMenuNotes(noteMenu.note!, -12) },
    { kind: 'item', label: noteMenu.note.muted ? '뮤트 해제' : '뮤트', checked: noteMenu.note.muted, run: () => useProjectStore.getState().updateMidiNotes(editor.trackId, editor.clipId, selected.includes(noteMenu.note!.id) ? selected : [noteMenu.note!.id], { muted: !noteMenu.note!.muted }) },
    { kind: 'separator' },
    { kind: 'item', label: '노트 삭제', keys: 'Delete', danger: true, run: () => useProjectStore.getState().deleteMidiNotes(editor.trackId, editor.clipId, selected.includes(noteMenu.note!.id) ? selected : [noteMenu.note!.id]) },
  ] : noteMenu && editor && clip ? [
    { kind: 'item', label: '여기에 노트 추가', run: () => { const id = useProjectStore.getState().addMidiNote(editor.trackId, editor.clipId, { pitch: noteMenu.pitch, velocity: keyboardVelocity, startTicks: noteMenu.tick, lengthTicks: gridTicks, releaseVelocity: 64, muted: false }); if (id) audition(noteMenu.pitch) } },
    { kind: 'item', label: '전체 노트 선택', keys: 'Ctrl+A', run: () => useProjectStore.setState({ selectedNoteIds: clip.notes.map((note) => note.id) }) },
    { kind: 'item', label: '선택 노트 퀀타이즈', disabled: !selected.length, run: quantize },
  ] : []

  if (!editor || !clip || !track) return <div className="piano-roll-empty" onPointerDownCapture={() => setEditFocus('pianoRoll')}><Piano size={28} /><strong>MIDI 클립을 더블클릭하세요</strong><span>인스트루먼트 트랙에서 MIDI 클립을 열면 피아노롤이 표시됩니다.</span></div>
  return <div className={`piano-roll ${focused ? 'edit-focused' : ''}`} onPointerDownCapture={() => setEditFocus('pianoRoll')}>
    <div className="piano-toolbar">
      <div className="piano-tool-palette" role="toolbar" aria-label="피아노 롤 도구"><button className={pianoTool === 'arrow' ? 'active' : ''} title="1 · 포인터" aria-keyshortcuts="1" onClick={() => setPianoTool('arrow')}><MousePointer2 size={13} /></button><button className={pianoTool === 'range' ? 'active' : ''} title="2 · 범위 선택" aria-keyshortcuts="2" onClick={() => setPianoTool('range')}><Activity size={13} /></button><button className={pianoTool === 'split' ? 'active' : ''} title="3 · 분할" aria-keyshortcuts="3" onClick={() => setPianoTool('split')}><Scissors size={13} /></button><button className={pianoTool === 'erase' ? 'active' : ''} title="4 · 지우개" aria-keyshortcuts="4" onClick={() => setPianoTool('erase')}><Eraser size={13} /></button><span className="piano-paint-tool"><button ref={paintButton} className={pianoTool === 'paint' ? 'active' : ''} title={`5 · 그리기 · ${paintShape}`} aria-keyshortcuts="5" onClick={() => setPianoTool('paint')}><Pencil size={13} /></button><button className={paintMenuOpen ? 'active' : ''} title="그리기 모양" onClick={() => setPaintMenuOpen((open) => !open)}><ChevronDown size={9} /></button></span><button className={pianoTool === 'mute' ? 'active' : ''} title="6 · 뮤트" aria-keyshortcuts="6" onClick={() => setPianoTool('mute')}>M</button><button className={pianoTool === 'listen' ? 'active' : ''} title="7 · 미리듣기" aria-keyshortcuts="7" onClick={() => setPianoTool('listen')}><Volume2 size={12} /></button>{paintMenuOpen && <FloatingPanel getAnchorElement={() => paintButton.current} onClose={() => setPaintMenuOpen(false)} className="piano-paint-menu"><strong>MIDI DRAW</strong>{(['freehand', 'line', 'parabola', 'square', 'triangle', 'saw', 'sine', 'transform'] as PianoPaintShape[]).map((shape) => <button key={shape} className={paintShape === shape ? 'active' : ''} onClick={() => { setPaintShape(shape); setPianoTool(shape === 'transform' ? 'arrow' : 'paint'); setPaintMenuOpen(false) }}><i className={`shape-${shape}`} /><span>{{ freehand: '프리핸드', line: '라인', parabola: '파라볼라', square: '스퀘어', triangle: '트라이앵글', saw: '톱니', sine: '사인', transform: '변형' }[shape]}</span></button>)}</FloatingPanel>}</div>
      <strong><Piano size={14} /> {clip.name}</strong>
      <button className={rollMode === 'drum' ? 'active' : ''} title={rollMode === 'drum' ? '피아노 롤로 전환' : '드럼 롤로 전환'} onClick={() => setRollMode((mode) => mode === 'piano' ? 'drum' : 'piano')}>{rollMode === 'drum' ? <Piano size={12} /> : <Drum size={12} />} {rollMode === 'drum' ? 'PIANO' : 'DRUM'}</button>
      <button className={snapEnabled ? 'active' : ''} title={`스냅 ${snapEnabled ? '켜짐' : '꺼짐'}`} aria-pressed={snapEnabled} onClick={() => useProjectStore.getState().togglePianoSnap()}><Magnet size={12} /> SNAP</button>
      <label>GRID <select value={gridTicks} onChange={(event) => setGridTicks(Number(event.target.value))}>{GRID_OPTIONS.map((grid) => <option key={grid.label} value={grid.ticks}>{grid.label}</option>)}</select></label>
      <button onClick={quantize}>퀀타이즈</button><label>강도 <input type="range" min="0" max="100" value={quantizeStrength} onChange={(event) => setQuantizeStrength(Number(event.target.value))} />{quantizeStrength}%</label><label>스윙 <input type="range" min="0" max="100" value={quantizeSwing} onChange={(event) => setQuantizeSwing(Number(event.target.value))} /><EditableNumber value={quantizeSwing} min={0} max={100} step={1} onChange={setQuantizeSwing} format={(value) => `${Math.round(value)}%`} /></label><button className={quantizeEnds ? 'active' : ''} onClick={() => setQuantizeEnds(!quantizeEnds)}>노트 끝</button>
      <label>KEY <select value={scaleRoot} onChange={(event) => setScaleRoot(Number(event.target.value))}>{['C','C#','D','D#','E','F','F#','G','G#','A','A#','B'].map((name, index) => <option key={name} value={index}>{name}</option>)}</select><select value={scaleName} onChange={(event) => setScaleName(event.target.value)}>{Object.keys(SCALES).map((name) => <option key={name}>{name}</option>)}</select></label>
      <button className={scaleSnap ? 'active' : ''} onClick={() => setScaleSnap(!scaleSnap)}>스케일 맞춤</button>
      <label>H <input type="range" min="7" max="22" value={noteHeight} onChange={(event) => setNoteHeight(Number(event.target.value))} /></label><label>W <input type="range" min="32" max="160" value={pixelsPerQuarter} onChange={(event) => setPixelsPerQuarter(Number(event.target.value))} /></label>
      <button className={preview ? 'active' : ''} onClick={() => setPreview(!preview)}><Volume2 size={13} /> 미리듣기</button>
      <button onClick={() => useProjectStore.getState().toggleEditorMaximized()}>{maximized ? <Minimize2 size={13} /> : <Maximize2 size={13} />}</button>
      <button className="piano-close" title="피아노 롤 닫기 (F2)" onClick={() => useProjectStore.getState().togglePianoRoll()}><X size={13} /></button>
    </div>
    <div className="keyboard-toolbar"><span>↑↓ 반음 · Ctrl+↑↓ 옥타브 · Alt+드래그 복사 · D 다음 구간 복제 · W/E 가로 줌 · CapsLock 가상 피아노</span><label>미리듣기 벨로시티 <input type="range" min="1" max="127" value={keyboardVelocity} onChange={(event) => setKeyboardVelocity(Number(event.target.value))} /> {keyboardVelocity}</label></div>
    <div className="piano-scroll" ref={scrollRef}>
      <div className="piano-stage" style={{ width: GUTTER + contentWidth, height: contentHeight + PIANO_RULER_HEIGHT }}>
        <PianoRuler clipStartSec={clip.startSec} width={contentWidth} pixelsPerQuarter={pixelsPerQuarter} />
        <canvas className="piano-layer piano-background" ref={backgroundRef} width={canvasWidth} height={contentHeight} style={{ left: GUTTER, top: PIANO_RULER_HEIGHT }} />
        <canvas className="piano-layer piano-notes" ref={notesRef} width={canvasWidth} height={contentHeight} style={{ left: GUTTER, top: PIANO_RULER_HEIGHT }} />
        <canvas className="piano-layer piano-overlay" ref={overlayRef} width={canvasWidth} height={contentHeight} style={{ left: GUTTER, top: PIANO_RULER_HEIGHT, cursor: noteCursor }} onContextMenu={(event) => { event.preventDefault(); const rect = event.currentTarget.getBoundingClientRect(); const x = event.clientX - rect.left + (scrollRef.current?.scrollLeft ?? 0); const y = event.clientY - rect.top; const note = noteAt(x, y) ?? null; if (note && !selected.includes(note.id)) useProjectStore.getState().selectMidiNote(note.id); setNoteMenu({ x: event.clientX, y: event.clientY, tick: snapTick(x / pixelsPerTick), pitch: snapPitch(127 - Math.floor(y / noteHeight)), note }) }} onDoubleClick={onNoteCanvasDoubleClick} onPointerDown={onPointerDown} onPointerMove={onOverlayPointerMove} onPointerLeave={() => { if (!marqueeRef.current && !dragRef.current) setNoteCursor('default') }} onPointerUp={finishMarquee} onPointerCancel={finishMarquee} />
        <div className="piano-gutter-offset" style={{ top: PIANO_RULER_HEIGHT }}><PianoGutter height={contentHeight} noteHeight={noteHeight} mode={rollMode} onNoteOn={beginHeldAudition} onNoteOff={endHeldAudition} onSelectPitch={(pitch) => useProjectStore.setState({ selectedNoteIds: clip.notes.filter((note) => note.pitch === pitch).map((note) => note.id), editFocus: 'pianoRoll' })} /></div>
        {marquee && <div className="note-marquee" style={{ left: GUTTER + Math.min(marquee.x0, marquee.x1), top: PIANO_RULER_HEIGHT + Math.min(marquee.y0, marquee.y1), width: Math.abs(marquee.x1 - marquee.x0), height: Math.abs(marquee.y1 - marquee.y0) }} />}
        {noteMenu && <MenuPanel items={noteMenuItems} anchor={{ x: noteMenu.x, y: noteMenu.y }} onClose={() => setNoteMenu(null)} />}
      </div>
    </div>
    <div className="piano-horizontal-scroll" ref={horizontalScrollRef} aria-label="피아노 롤 가로 스크롤"><div style={{ width: GUTTER + contentWidth }} /></div>
    <div className="midi-event-editor">
      <div className="midi-event-toolbar">
        <strong><Activity size={12} /> MIDI EVENT</strong>
        {visibleEventLanes.map((lane) => { const label = EVENT_LANE_OPTIONS.find(([id]) => id === lane)?.[1] ?? lane; const removable = !DEFAULT_EVENT_LANES.includes(lane); return <button key={lane} className={`midi-event-tab ${eventLane === lane ? 'active' : ''}`} onClick={() => setEventLane(lane)}><span>{label}</span>{removable && <i title={`${label} 탭 제거`} onClick={(event) => { event.stopPropagation(); const next = visibleEventLanes.filter((item) => item !== lane); setVisibleEventLanes(next); if (eventLane === lane) setEventLane(next[0] ?? 'velocity') }}><X size={9} /></i>}</button> })}
        <button ref={eventAddButton} className="midi-event-add" title="MIDI 이벤트 탭 추가" onClick={() => setEventPickerOpen((open) => !open)}><Plus size={11} /></button>
        {eventPickerOpen && <FloatingPanel getAnchorElement={() => eventAddButton.current} onClose={() => setEventPickerOpen(false)} className="midi-event-picker"><header><strong>MIDI EVENT 추가</strong></header><div>{EVENT_LANE_OPTIONS.filter(([lane]) => !visibleEventLanes.includes(lane)).map(([lane, label]) => <button key={lane} onClick={() => { setVisibleEventLanes((items) => [...items, lane]); setEventLane(lane); setEventPickerOpen(false) }}>{label}</button>)}{EVENT_LANE_OPTIONS.every(([lane]) => visibleEventLanes.includes(lane)) && <small>모든 이벤트가 표시되어 있습니다.</small>}</div></FloatingPanel>}
        {eventLane === 'cc' && <label>CC <select value={genericCc} onChange={(event) => setGenericCc(Number(event.target.value))}>{Array.from({ length: 128 }, (_, cc) => <option key={cc} value={cc}>{cc} · {midiCcName(cc)}</option>)}</select></label>}
        <span>{eventLane === 'velocity' || eventLane === 'releaseVelocity' ? '노트별 막대 · 드래그하여 값 편집' : eventLane === 'sustain' ? '계단형 스위치 · 위=ON / 아래=OFF' : '연속형 포인트 · 우클릭으로 포인트 삭제'}</span>
      </div>
      <div className="midi-event-lane">
        <div className="midi-event-gutter"><Gauge size={12} /><strong>{eventLaneLabel(eventLane, genericCc)}</strong><small>{eventLane === 'pitchBend' ? '±2 st' : '0–127'}</small></div>
        <canvas ref={eventCanvasRef} className={`midi-event-canvas lane-${eventLane}`} height={EVENT_LANE_HEIGHT} onPointerDown={onEventPointerDown} onContextMenu={onEventContextMenu} />
      </div>
    </div>
  </div>
}

function PianoGutter({ height, noteHeight, mode, onNoteOn, onNoteOff, onSelectPitch }: { height: number; noteHeight: number; mode: 'piano' | 'drum'; onNoteOn(pitch: number): number | null; onNoteOff(id: number | null): void; onSelectPitch(pitch: number): void }) {
  const rows = useMemo(() => Array.from({ length: 128 }, (_, row) => 127 - row), [])
  const dragging = useRef(false)
  const active = useRef<{ pitch: number; id: number | null; button: HTMLButtonElement | null } | null>(null)
  const stop = useCallback(() => {
    if (active.current) {
      active.current.button?.classList.remove('playing')
      onNoteOff(active.current.id)
    }
    active.current = null
  }, [onNoteOff])
  const playAt = useCallback((gutter: HTMLDivElement, clientY: number) => {
    // Resolve by row geometry instead of elementFromPoint. Pointer capture and
    // the short visual width of black keys must not create dead zones while scrubbing.
    const pitch = pianoPitchAtClientY(clientY, gutter.getBoundingClientRect().top, noteHeight)
    if (pitch === null) { stop(); return }
    const row = 127 - pitch
    if (active.current?.pitch === pitch) return
    stop()
    const button = gutter.children.item(row) as HTMLButtonElement | null
    button?.classList.add('playing')
    active.current = { pitch, id: onNoteOn(pitch), button }
  }, [noteHeight, onNoteOn, stop])
  useEffect(() => () => stop(), [stop])
  return <div className={`piano-gutter ${mode === 'drum' ? 'drum' : ''}`} style={{ width: GUTTER, height }} onPointerDown={(event) => { if (event.button !== 0) return; event.preventDefault(); dragging.current = true; stop(); event.currentTarget.setPointerCapture(event.pointerId); playAt(event.currentTarget, event.clientY) }} onPointerMove={(event) => { if (dragging.current) playAt(event.currentTarget, event.clientY) }} onPointerUp={(event) => { dragging.current = false; stop(); if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId) }} onPointerCancel={() => { dragging.current = false; stop() }} onLostPointerCapture={() => { dragging.current = false; stop() }}>{rows.map((pitch) => { const black = [1, 3, 6, 8, 10].includes(pitch % 12); const drumName = GM_DRUM_NAMES[pitch]; return <button key={pitch} type="button" data-pitch={pitch} className={mode === 'drum' ? drumName ? 'mapped' : 'unmapped' : black ? 'black' : 'white'} style={{ height: noteHeight }} title={mode === 'drum' ? `${pitch} · ${drumName ?? 'Unmapped'}` : undefined} onDoubleClick={() => onSelectPitch(pitch)}>{mode === 'drum' ? drumName ? `${pitch} ${drumName}` : pitch : pitch % 12 === 0 ? `C${Math.floor(pitch / 12) - 1}` : ''}</button> })}</div>
}

function PianoRuler({ clipStartSec, width, pixelsPerQuarter }: { clipStartSec: number; width: number; pixelsPerQuarter: number }) {
  const engine = useEngine()
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const signature = useProjectStore((state) => state.project.transport.timeSignature)
  const pixelsPerSecond = pixelsPerQuarter * bpm / 60
  const ticks = useMemo(() => buildRulerTicks(width, pixelsPerSecond, bpm, signature), [bpm, pixelsPerSecond, signature, width])
  return <div className="piano-ruler" style={{ width: GUTTER + width }} onPointerDown={(event) => {
    const localSec = Math.max(0, (event.clientX - event.currentTarget.getBoundingClientRect().left - GUTTER) / pixelsPerSecond)
    const sec = clipStartSec + localSec
    useProjectStore.getState().setPlayhead(sec)
    void engine.seek(sec)
  }}><span className="piano-ruler-corner" style={{ width: GUTTER }} />{ticks.map((tick) => <i key={tick.sec} className={tick.strong ? 'strong' : ''} style={{ left: GUTTER + tick.sec * pixelsPerSecond }}>{tick.label && <b>{tick.label}</b>}</i>)}</div>
}

function setupCanvas(canvas: HTMLCanvasElement | null, width: number, height: number) { if (!canvas) return null; const ratio = Math.min(window.devicePixelRatio || 1, 1.5); if (canvas.width !== Math.ceil(width * ratio) || canvas.height !== Math.ceil(height * ratio)) { canvas.width = Math.ceil(width * ratio); canvas.height = Math.ceil(height * ratio); canvas.style.width = `${width}px`; canvas.style.height = `${height}px` }; const context = canvas.getContext('2d'); context?.setTransform(ratio, 0, 0, ratio, 0, 0); return context }
function positionPianoCanvas(canvas: HTMLCanvasElement | null, scrollLeft: number) { if (canvas) canvas.style.transform = `translate3d(${scrollLeft}px,0,0)` }
function drawGrid(canvas: HTMLCanvasElement | null, width: number, height: number, rowHeight: number, ppt: number, grid: number, barTicks: number, root: number, scaleName: string, scrollLeft: number, activeTicks: number, neighbors: readonly NeighborClipRange[]) {
  const context = setupCanvas(canvas, width, height)
  if (!context) return
  positionPianoCanvas(canvas, scrollLeft)
  context.clearRect(0, 0, width, height)
  const scale = SCALES[scaleName] ?? SCALES.Major!
  for (let row = 0; row < 128; row += 1) {
    const pitch = 127 - row
    const black = [1, 3, 6, 8, 10].includes(pitch % 12)
    const out = !scale.includes((pitch - root + 120) % 12)
    context.fillStyle = out ? '#10161c' : black ? '#151d24' : '#192129'
    context.fillRect(0, row * rowHeight, width, rowHeight - 1)
  }
  const startTick = Math.max(0, Math.floor(scrollLeft / ppt / grid) * grid)
  const endTick = (scrollLeft + width) / ppt + grid
  for (let tick = startTick; tick <= endTick; tick += grid) {
    const x = tick * ppt - scrollLeft
    const onBar = tick % Math.max(1, barTicks) === 0
    context.strokeStyle = onBar ? '#52606b' : tick % MIDI_PPQ === 0 ? '#34414c' : '#26323c'
    context.lineWidth = onBar ? 1.5 : 1
    context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.stroke()
  }
  const activeEndX = activeTicks * ppt - scrollLeft
  if (activeEndX < width) {
    context.fillStyle = 'rgba(5,9,13,.58)'
    context.fillRect(Math.max(0, activeEndX), 0, width - Math.max(0, activeEndX), height)
    context.strokeStyle = '#70818d'; context.lineWidth = 1.5; context.beginPath(); context.moveTo(Math.round(activeEndX) + .5, 0); context.lineTo(Math.round(activeEndX) + .5, height); context.stroke()
  }
  for (const neighbor of neighbors) {
    const left = neighbor.startTicks * ppt - scrollLeft
    const right = neighbor.endTicks * ppt - scrollLeft
    if (right < 0 || left > width) continue
    context.fillStyle = 'rgba(75,112,130,.2)'
    context.fillRect(Math.max(0, left), 0, Math.min(width, right) - Math.max(0, left), height)
    context.strokeStyle = '#668fa2'; context.lineWidth = 1; context.setLineDash([4, 4]); context.strokeRect(Math.round(left) + .5, .5, Math.max(1, right - left), height - 1); context.setLineDash([])
  }
}

function drawNotes(canvas: HTMLCanvasElement | null, notes: MidiNote[], selected: string[], width: number, height: number, rowHeight: number, ppt: number, scroll: HTMLDivElement | null, drumMode: boolean) {
  const context = setupCanvas(canvas, width, height)
  if (!context) return
  const selectedSet = new Set(selected)
  const scrollLeft = scroll?.scrollLeft ?? 0
  positionPianoCanvas(canvas, scrollLeft)
  const startTick = Math.max(0, scrollLeft / ppt)
  const endTick = startTick + width / ppt + 960
  const start = lowerBound(notes, startTick)
  context.clearRect(0, 0, width, height)
  for (let index = start; index < notes.length; index += 1) {
    const note = notes[index]!
    if (note.startTicks > endTick) break
    const x = note.startTicks * ppt - scrollLeft
    const y = (127 - note.pitch) * rowHeight + 1
    const w = Math.max(3, note.lengthTicks * ppt - 1)
    const velocity = note.velocity / 127
    const isSelected = selectedSet.has(note.id)
    context.fillStyle = note.muted ? '#53606a' : isSelected ? '#f4d35e' : `hsl(193 78% ${34 + velocity * 28}%)`
    context.strokeStyle = isSelected ? '#fff2a6' : '#92e5ff88'
    if (drumMode) {
      const radius = Math.max(3, Math.min(7, (rowHeight - 3) / 2))
      context.beginPath(); context.moveTo(x, y + rowHeight / 2 - 1); context.lineTo(x + radius, y + 1); context.lineTo(x + radius * 2, y + rowHeight / 2 - 1); context.lineTo(x + radius, y + rowHeight - 3); context.closePath(); context.fill(); context.stroke()
    } else {
      context.fillRect(x, y, w, rowHeight - 2)
      context.strokeRect(x + .5, y + .5, w - 1, rowHeight - 3)
    }
  }
}
function drawOverlay(canvas: HTMLCanvasElement | null, width: number, height: number, playheadX: number, drag: Drag | null, rowHeight: number, ppt: number, scrollLeft: number, activeEndX: number) { const context = setupCanvas(canvas, width, height); if (!context) return; positionPianoCanvas(canvas, scrollLeft); context.clearRect(0, 0, width, height); const localActiveEnd = activeEndX - scrollLeft; if (localActiveEnd < width) { context.fillStyle = 'rgba(4,7,10,.34)'; context.fillRect(Math.max(0, localActiveEnd), 0, width - Math.max(0, localActiveEnd), height) } const localPlayhead = playheadX - scrollLeft; if (localPlayhead >= 0 && localPlayhead <= width) { context.strokeStyle = '#fff'; context.beginPath(); context.moveTo(localPlayhead, 0); context.lineTo(localPlayhead, height); context.stroke() } if (drag) { const x = drag.latestTick * ppt - scrollLeft; context.fillStyle = '#fff4a955'; context.strokeStyle = '#fff4a9'; context.fillRect(x, (127 - drag.latestPitch) * rowHeight + 1, drag.latestLength * ppt, rowHeight - 2); context.strokeRect(x, (127 - drag.latestPitch) * rowHeight + 1, drag.latestLength * ppt, rowHeight - 2) } }
function drawEventLane(canvas: HTMLCanvasElement | null, lane: EventLane, controller: CcLane | undefined, notes: MidiNote[], selected: string[], ppt: number, gridTicks: number, scrollLeft: number) {
  if (!canvas) return
  const width = Math.max(1, canvas.clientWidth)
  const height = EVENT_LANE_HEIGHT
  const context = setupCanvas(canvas, width, height)
  if (!context) return
  const selectedSet = new Set(selected)
  context.clearRect(0, 0, width, height)
  context.fillStyle = '#10171d'
  context.fillRect(0, 0, width, height)
  const startTick = Math.max(0, scrollLeft / ppt)
  const endTick = startTick + width / ppt
  for (let tick = Math.floor(startTick / gridTicks) * gridTicks; tick <= endTick; tick += gridTicks) {
    const x = tick * ppt - scrollLeft
    context.strokeStyle = tick % 3840 === 0 ? '#40515e' : tick % 960 === 0 ? '#2b3943' : '#202b33'
    context.lineWidth = tick % 3840 === 0 ? 1.5 : 1
    context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.stroke()
  }
  context.strokeStyle = '#34434d'
  context.beginPath(); context.moveTo(0, height / 2); context.lineTo(width, height / 2); context.stroke()
  if (lane === 'velocity' || lane === 'releaseVelocity') {
    const key = lane
    for (const note of notes.slice(lowerBound(notes, startTick))) {
      if (note.startTicks > endTick) break
      const x = note.startTicks * ppt - scrollLeft
      const value = note[key]
      const bar = value / 127 * (height - 7)
      context.fillStyle = selectedSet.has(note.id) ? '#f4d35e' : lane === 'velocity' ? '#49c8ed' : '#8b9cf4'
      context.fillRect(x - 1, height - bar, Math.max(3, Math.min(7, note.lengthTicks * ppt)), bar)
      context.fillRect(x - 2, height - bar - 2, Math.max(5, Math.min(9, note.lengthTicks * ppt + 2)), 2)
    }
    return
  }
  const points = controller?.points ?? []
  const y = (value: number) => lane === 'pitchBend' ? (1 - (value + 8192) / 16_383) * height : (1 - value / 127) * height
  const defaultY = lane === 'pitchBend' ? height / 2 : height
  context.strokeStyle = lane === 'sustain' ? '#f1bb55' : lane === 'pitchBend' ? '#e884cf' : '#58d0f3'
  context.fillStyle = lane === 'sustain' ? '#d9a64626' : '#58cbed22'
  context.lineWidth = 1.5
  context.beginPath()
  let previousY = defaultY
  let started = false
  for (const point of points) {
    if (point.ticks < startTick) { previousY = y(point.value); continue }
    if (point.ticks > endTick) break
    const x = point.ticks * ppt - scrollLeft
    if (!started) { context.moveTo(0, previousY); started = true }
    if (lane === 'sustain') { context.lineTo(x, previousY); context.lineTo(x, y(point.value)) }
    else context.lineTo(x, y(point.value))
    previousY = y(point.value)
  }
  if (!started) context.moveTo(0, previousY)
  context.lineTo(width, previousY)
  context.stroke()
  if (lane === 'sustain') { context.lineTo(width, height); context.lineTo(0, height); context.closePath(); context.fill() }
  for (const point of points) {
    if (point.ticks < startTick || point.ticks > endTick) continue
    const x = point.ticks * ppt - scrollLeft
    context.beginPath(); context.arc(x, y(point.value), 3, 0, Math.PI * 2); context.fillStyle = context.strokeStyle; context.fill()
  }
}

function eventLaneLabel(lane: EventLane, cc: number): string {
  if (lane === 'velocity') return 'VELOCITY'
  if (lane === 'releaseVelocity') return 'RELEASE'
  if (lane === 'sustain') return 'SUSTAIN 64'
  if (lane === 'vibrato') return 'MOD 1'
  if (lane === 'pitchBend') return 'PITCH'
  return `CC ${cc}`
}

function midiCcName(cc: number): string {
  const names: Record<number, string> = { 1: 'Modulation', 2: 'Breath', 4: 'Foot', 7: 'Volume', 10: 'Pan', 11: 'Expression', 64: 'Sustain', 65: 'Portamento', 71: 'Resonance', 74: 'Brightness', 91: 'Reverb', 93: 'Chorus' }
  return names[cc] ?? 'Controller'
}
const lowerBound = findVisibleNoteStart
function pianoCursorFor(tool: ToolId, hit: boolean): string {
  return ({ arrow: hit ? 'grab' : 'default', range: hit ? 'grab' : 'crosshair', split: 'col-resize', erase: 'not-allowed', paint: 'crosshair', mute: 'pointer', listen: 'pointer' } as Record<ToolId, string>)[tool]
}
function shapedMidiValue(shape: PianoPaintShape, t: number, start: number, end: number, min: number, max: number): number {
  if (shape === 'line') return start + (end - start) * t
  if (shape === 'parabola') return start + (end - start) * t * t
  if (shape === 'square') return t < .5 ? start : end
  if (shape === 'triangle') { const peak = start + end < min + max ? max : min; return t < .5 ? start + (peak - start) * t * 2 : peak + (end - peak) * (t - .5) * 2 }
  if (shape === 'saw') return min + (max - min) * ((t * 4) % 1)
  if (shape === 'sine') { const center = (start + end) / 2; const amplitude = Math.max(Math.abs(start - center), Math.abs(end - center), (max - min) * .25); return Math.max(min, Math.min(max, center + Math.sin(t * Math.PI * 4 - Math.PI / 2) * amplitude)) }
  return start + (end - start) * t
}
function nearestScalePitch(pitch: number, root: number, scale: number[]) { for (let distance = 0; distance < 12; distance += 1) { for (const candidate of [pitch - distance, pitch + distance]) if (candidate >= 0 && candidate <= 127 && scale.includes((candidate - root + 120) % 12)) return candidate } return Math.max(0, Math.min(127, pitch)) }
