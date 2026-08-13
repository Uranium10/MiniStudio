// Canvas piano roll sharing the arrangement tool system and realtime test-tone preview.
import { Maximize2, Minimize2, Piano, Volume2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { GRID_OPTIONS, MIDI_PPQ, type MidiNote } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { useProjectStore } from '../store/projectStore'
import { getEffectiveTool, useToolStore } from '../store/toolStore'
import { MenuPanel, type MenuItem } from './Menu'
import { findVisibleNoteStart } from './pianoRollMath'

const GUTTER = 52
const VELOCITY_HEIGHT = 72
const SCALES: Record<string, number[]> = { Major: [0, 2, 4, 5, 7, 9, 11], 'Natural Minor': [0, 2, 3, 5, 7, 8, 10], 'Harmonic Minor': [0, 2, 3, 5, 7, 8, 11], Dorian: [0, 2, 3, 5, 7, 9, 10], Phrygian: [0, 1, 3, 5, 7, 8, 10], Lydian: [0, 2, 4, 6, 7, 9, 11], Mixolydian: [0, 2, 4, 5, 7, 9, 10], Locrian: [0, 1, 3, 5, 6, 8, 10], 'Pentatonic Major': [0, 2, 4, 7, 9], 'Pentatonic Minor': [0, 3, 5, 7, 10], Blues: [0, 3, 5, 6, 7, 10] }

type Drag = { mode: 'move' | 'resize'; note: MidiNote; startX: number; startY: number; latestTick: number; latestPitch: number; latestLength: number; copy: boolean }
type Marquee = { x0: number; y0: number; x1: number; y1: number }
type NoteMenu = { x: number; y: number; tick: number; pitch: number; note: MidiNote | null }

export function PianoRoll() {
  const editor = useProjectStore((state) => state.editorClip)
  const tracks = useProjectStore((state) => state.project.tracks)
  const selected = useProjectStore((state) => state.selectedNoteIds)
  const maximized = useProjectStore((state) => state.editorMaximized)
  const playhead = useProjectStore((state) => state.playheadSec)
  const focused = useProjectStore((state) => state.editFocus === 'pianoRoll')
  const setEditFocus = useProjectStore((state) => state.setEditFocus)
  const engine = useEngine()
  const track = editor ? tracks.find((item) => item.id === editor.trackId) : undefined
  const clip = editor ? track?.midiClips.find((item) => item.id === editor.clipId) : undefined
  // The grid is shared with the toolbar and the arrangement so a project has one
  // musical resolution instead of three editors disagreeing with each other.
  const gridTicks = useProjectStore((state) => state.gridTicks)
  const setGridTicks = useProjectStore((state) => state.setGridTicks)
  const pixelsPerQuarter = useProjectStore((state) => state.pianoRollZoom)
  const setPixelsPerQuarter = useProjectStore((state) => state.setPianoRollZoom)
  const [noteHeight, setNoteHeight] = useState(12)
  const [preview, setPreview] = useState(true)
  const [keyboardVelocity, setKeyboardVelocity] = useState(100)
  const [scaleRoot, setScaleRoot] = useState(0)
  const [scaleName, setScaleName] = useState('Major')
  const [scaleSnap, setScaleSnap] = useState(false)
  const [quantizeStrength, setQuantizeStrength] = useState(100)
  const [quantizeSwing, setQuantizeSwing] = useState(0)
  const [quantizeEnds, setQuantizeEnds] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const backgroundRef = useRef<HTMLCanvasElement>(null)
  const notesRef = useRef<HTMLCanvasElement>(null)
  const overlayRef = useRef<HTMLCanvasElement>(null)
  const velocityRef = useRef<HTMLCanvasElement>(null)
  const dragRef = useRef<Drag | null>(null)
  const marqueeRef = useRef<Marquee | null>(null)
  const marqueeBaseRef = useRef<string[]>([])
  const [marquee, setMarquee] = useState<Marquee | null>(null)
  const [noteMenu, setNoteMenu] = useState<NoteMenu | null>(null)
  const centredClipId = useRef<string | null>(null)
  const auditionId = useRef(2_000_000)
  const pixelsPerTick = pixelsPerQuarter / MIDI_PPQ
  const maxTicks = Math.max(15_360, clip?.loopLengthTicks ?? 0, ...(clip?.notes.map((note) => note.startTicks + note.lengthTicks + 960) ?? []))
  const contentWidth = Math.max(1000, Math.ceil(maxTicks * pixelsPerTick))
  const contentHeight = 128 * noteHeight

  const audition = (pitch: number) => {
    if (!track || track.kind !== 'instrument') return
    if (!preview) return
    const id = auditionId.current++
    engine.midiNote(track.id, id, pitch, keyboardVelocity / 127, true)
    window.setTimeout(() => engine.midiNote(track.id, id, pitch, 0, false), 180)
  }

  // Opening a clip used to land at pitch 127, so every clip needed a manual scroll.
  // Centre on the clip's own material instead, falling back to C4 when it is empty.
  useEffect(() => {
    const scroll = scrollRef.current
    if (!clip || !scroll || centredClipId.current === clip.id) return
    centredClipId.current = clip.id
    const pitches = clip.notes.map((note) => note.pitch)
    const focusPitch = pitches.length ? (Math.min(...pitches) + Math.max(...pitches)) / 2 : 60
    scroll.scrollTop = Math.max(0, (127 - focusPitch) * noteHeight - scroll.clientHeight / 2 + VELOCITY_HEIGHT / 2)
    scroll.scrollLeft = Math.max(0, (clip.notes[0]?.startTicks ?? 0) * pixelsPerTick - 80)
  }, [clip, noteHeight, pixelsPerTick])
  useEffect(() => {
    if (!clip) return
    const scroll = scrollRef.current
    const draw = () => {
      drawGrid(backgroundRef.current, contentWidth, contentHeight, noteHeight, pixelsPerTick, gridTicks, scaleRoot, scaleName)
      drawNotes(notesRef.current, clip.notes, selected, contentWidth, contentHeight, noteHeight, pixelsPerTick, scroll)
      drawOverlay(overlayRef.current, contentWidth, contentHeight, (playhead - clip.startSec) * useProjectStore.getState().project.transport.bpm / 60 * MIDI_PPQ * pixelsPerTick, dragRef.current, noteHeight, pixelsPerTick)
      drawVelocity(velocityRef.current, clip.notes, selected, contentWidth, VELOCITY_HEIGHT, pixelsPerTick, scroll)
    }
    let frame = requestAnimationFrame(draw)
    const onScroll = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    scroll?.addEventListener('scroll', onScroll, { passive: true })
    return () => { cancelAnimationFrame(frame); scroll?.removeEventListener('scroll', onScroll) }
  }, [clip, selected, contentWidth, contentHeight, noteHeight, pixelsPerTick, gridTicks, scaleRoot, scaleName, playhead])

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
  const snapTick = (tick: number) => Math.max(0, Math.round(tick / gridTicks) * gridTicks)
  const snapPitch = (pitch: number) => scaleSnap ? nearestScalePitch(pitch, scaleRoot, SCALES[scaleName] ?? SCALES.Major!) : Math.max(0, Math.min(127, pitch))
  const point = (event: React.PointerEvent<HTMLCanvasElement>) => { const rect = event.currentTarget.getBoundingClientRect(); return { x: event.clientX - rect.left, y: event.clientY - rect.top } }

  const onPointerDown = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!editor || !clip) return
    if (event.button !== 0) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    const { x, y } = point(event)
    const hit = noteAt(x, y)
    const tool = getEffectiveTool(useToolStore.getState())
    if (tool === 'paint' || (!hit && event.detail >= 2)) {
      const pitch = snapPitch(127 - Math.floor(y / noteHeight)); const tick = snapTick(x / pixelsPerTick)
      const id = useProjectStore.getState().addMidiNote(editor.trackId, editor.clipId, { pitch, velocity: keyboardVelocity, startTicks: tick, lengthTicks: gridTicks, releaseVelocity: 64, muted: false })
      if (id) audition(pitch)
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
    if (tool === 'split') { const at = snapTick(x / pixelsPerTick); if (at > hit.startTicks && at < hit.startTicks + hit.lengthTicks) { useProjectStore.getState().updateMidiNotes(editor.trackId, editor.clipId, [hit.id], { lengthTicks: at - hit.startTicks }); useProjectStore.getState().addMidiNote(editor.trackId, editor.clipId, { ...hit, startTicks: at, lengthTicks: hit.startTicks + hit.lengthTicks - at, releaseVelocity: hit.releaseVelocity, muted: hit.muted }) }; return }
    if (tool === 'listen') { audition(hit.pitch); return }
    const store = useProjectStore.getState()
    const additive = event.shiftKey || event.ctrlKey || event.metaKey
    if (!store.selectedNoteIds.includes(hit.id) || additive) store.selectMidiNote(hit.id, additive)
    audition(hit.pitch)
    const resize = Math.abs(x - (hit.startTicks + hit.lengthTicks) * pixelsPerTick) <= 7
    dragRef.current = { mode: resize ? 'resize' : 'move', note: { ...hit }, startX: x, startY: y, latestTick: hit.startTicks, latestPitch: hit.pitch, latestLength: hit.lengthTicks, copy: event.altKey && !resize }
    const move = (pointer: PointerEvent) => {
      const drag = dragRef.current; if (!drag) return
      if (drag.mode === 'resize') drag.latestLength = Math.max(gridTicks, snapTick(drag.note.lengthTicks + (pointer.clientX - event.clientX) / pixelsPerTick))
      else { const nextTick = snapTick(drag.note.startTicks + (pointer.clientX - event.clientX) / pixelsPerTick); const nextPitch = snapPitch(drag.note.pitch - Math.round((pointer.clientY - event.clientY) / noteHeight)); if (nextPitch !== drag.latestPitch || nextTick !== drag.latestTick) audition(nextPitch); drag.latestTick = nextTick; drag.latestPitch = nextPitch }
      drawOverlay(overlayRef.current, contentWidth, contentHeight, -1, drag, noteHeight, pixelsPerTick)
    }
    const up = () => {
      const drag = dragRef.current; dragRef.current = null
      window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up)
      if (!drag) return
      const ids = useProjectStore.getState().selectedNoteIds.includes(drag.note.id) ? useProjectStore.getState().selectedNoteIds : [drag.note.id]
      const currentClip = useProjectStore.getState().project.tracks.find((item) => item.id === editor.trackId)?.midiClips.find((item) => item.id === editor.clipId)
      if (drag.mode === 'resize') { const delta = drag.latestLength - drag.note.lengthTicks; if (delta !== 0) useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, (currentClip?.notes.filter((item) => ids.includes(item.id)) ?? []).map((note) => ({ id: note.id, patch: { lengthTicks: Math.max(gridTicks, note.lengthTicks + delta) } }))) }
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

  const onOverlayPointerMove = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const box = marqueeRef.current
    if (!box) return
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

  /** Velocity lane paints across notes while dragging instead of setting one per click. */
  const onVelocityPointerDown = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!editor || !clip) return
    event.preventDefault()
    const canvas = event.currentTarget
    canvas.setPointerCapture(event.pointerId)
    const rect = canvas.getBoundingClientRect()
    const touched = new Map<string, number>()
    const apply = (clientX: number, clientY: number) => {
      const x = clientX - rect.left
      const velocity = Math.max(1, Math.min(127, Math.round((1 - (clientY - rect.top) / VELOCITY_HEIGHT) * 126 + 1)))
      for (const note of clip.notes) {
        const noteX = note.startTicks * pixelsPerTick
        if (x >= noteX - 3 && x <= noteX + Math.max(8, note.lengthTicks * pixelsPerTick)) touched.set(note.id, velocity)
      }
      if (touched.size) useProjectStore.getState().updateMidiNoteBatch(editor.trackId, editor.clipId, [...touched].map(([id, value]) => ({ id, patch: { velocity: value } })))
    }
    apply(event.clientX, event.clientY)
    const move = (pointer: PointerEvent) => apply(pointer.clientX, pointer.clientY)
    const up = () => {
      canvas.releasePointerCapture(event.pointerId)
      canvas.removeEventListener('pointermove', move)
      canvas.removeEventListener('pointerup', up)
      canvas.removeEventListener('pointercancel', up)
    }
    canvas.addEventListener('pointermove', move)
    canvas.addEventListener('pointerup', up)
    canvas.addEventListener('pointercancel', up)
  }

  const quantize = () => {
    if (!editor || !clip) return
    const ids = selected.length ? selected : clip.notes.map((note) => note.id)
    const swingOffset = (step: number) => step % 2 ? Math.round(gridTicks * quantizeSwing / 200) : 0
    const updates = clip.notes.filter((item) => ids.includes(item.id)).map((note) => {
      const step = Math.round(note.startTicks / gridTicks)
      const target = step * gridTicks + swingOffset(step)
      const startTicks = Math.round(note.startTicks + (target - note.startTicks) * quantizeStrength / 100)
      if (!quantizeEnds) return { id: note.id, patch: { startTicks } }
      const oldEnd = note.startTicks + note.lengthTicks
      const endStep = Math.round(oldEnd / gridTicks)
      const targetEnd = endStep * gridTicks + swingOffset(endStep)
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
      <strong><Piano size={14} /> {clip.name}</strong>
      <label>GRID <select value={gridTicks} onChange={(event) => setGridTicks(Number(event.target.value))}>{GRID_OPTIONS.map((grid) => <option key={grid.label} value={grid.ticks}>{grid.label}</option>)}</select></label>
      <button onClick={quantize}>퀀타이즈</button><label>강도 <input type="range" min="0" max="100" value={quantizeStrength} onChange={(event) => setQuantizeStrength(Number(event.target.value))} />{quantizeStrength}%</label><label>스윙 <input type="range" min="0" max="100" value={quantizeSwing} onChange={(event) => setQuantizeSwing(Number(event.target.value))} />{quantizeSwing}%</label><button className={quantizeEnds ? 'active' : ''} onClick={() => setQuantizeEnds(!quantizeEnds)}>노트 끝</button>
      <label>KEY <select value={scaleRoot} onChange={(event) => setScaleRoot(Number(event.target.value))}>{['C','C#','D','D#','E','F','F#','G','G#','A','A#','B'].map((name, index) => <option key={name} value={index}>{name}</option>)}</select><select value={scaleName} onChange={(event) => setScaleName(event.target.value)}>{Object.keys(SCALES).map((name) => <option key={name}>{name}</option>)}</select></label>
      <button className={scaleSnap ? 'active' : ''} onClick={() => setScaleSnap(!scaleSnap)}>스케일 맞춤</button>
      <label>H <input type="range" min="7" max="22" value={noteHeight} onChange={(event) => setNoteHeight(Number(event.target.value))} /></label><label>W <input type="range" min="32" max="160" value={pixelsPerQuarter} onChange={(event) => setPixelsPerQuarter(Number(event.target.value))} /></label>
      <button className={preview ? 'active' : ''} onClick={() => setPreview(!preview)}><Volume2 size={13} /> 미리듣기</button>
      <button onClick={() => useProjectStore.getState().toggleEditorMaximized()}>{maximized ? <Minimize2 size={13} /> : <Maximize2 size={13} />}</button>
    </div>
    <div className="keyboard-toolbar"><span>↑↓ 반음 · Ctrl+↑↓ 옥타브 · Alt+드래그 복사 · D 다음 구간 복제 · W/E 가로 줌 · CapsLock 가상 피아노</span><label>미리듣기 벨로시티 <input type="range" min="1" max="127" value={keyboardVelocity} onChange={(event) => setKeyboardVelocity(Number(event.target.value))} /> {keyboardVelocity}</label></div>
    <div className="piano-scroll" ref={scrollRef}>
      <div className="piano-stage" style={{ width: GUTTER + contentWidth, height: contentHeight + VELOCITY_HEIGHT }}>
        <canvas className="piano-layer piano-background" ref={backgroundRef} width={contentWidth} height={contentHeight} style={{ left: GUTTER }} />
        <canvas className="piano-layer piano-notes" ref={notesRef} width={contentWidth} height={contentHeight} style={{ left: GUTTER }} />
        <canvas className="piano-layer piano-overlay" ref={overlayRef} width={contentWidth} height={contentHeight} style={{ left: GUTTER }} onContextMenu={(event) => { event.preventDefault(); const rect = event.currentTarget.getBoundingClientRect(); const x = event.clientX - rect.left; const y = event.clientY - rect.top; const note = noteAt(x, y) ?? null; if (note && !selected.includes(note.id)) useProjectStore.getState().selectMidiNote(note.id); setNoteMenu({ x: event.clientX, y: event.clientY, tick: snapTick(x / pixelsPerTick), pitch: snapPitch(127 - Math.floor(y / noteHeight)), note }) }} onPointerDown={onPointerDown} onPointerMove={onOverlayPointerMove} onPointerUp={finishMarquee} onPointerCancel={finishMarquee} />
        <PianoGutter height={contentHeight} noteHeight={noteHeight} onNote={(pitch) => audition(pitch)} onSelectPitch={(pitch) => useProjectStore.setState({ selectedNoteIds: clip.notes.filter((note) => note.pitch === pitch).map((note) => note.id) })} />
        <canvas className="velocity-canvas" ref={velocityRef} width={contentWidth} height={VELOCITY_HEIGHT} style={{ left: GUTTER, top: contentHeight }} onPointerDown={onVelocityPointerDown} />
        {marquee && <div className="note-marquee" style={{ left: GUTTER + Math.min(marquee.x0, marquee.x1), top: Math.min(marquee.y0, marquee.y1), width: Math.abs(marquee.x1 - marquee.x0), height: Math.abs(marquee.y1 - marquee.y0) }} />}
        {noteMenu && <MenuPanel items={noteMenuItems} anchor={{ x: noteMenu.x, y: noteMenu.y }} onClose={() => setNoteMenu(null)} />}
      </div>
    </div>
  </div>
}

function PianoGutter({ height, noteHeight, onNote, onSelectPitch }: { height: number; noteHeight: number; onNote(pitch: number): void; onSelectPitch(pitch: number): void }) {
  const rows = useMemo(() => Array.from({ length: 128 }, (_, row) => 127 - row), [])
  return <div className="piano-gutter" style={{ width: GUTTER, height }}>{rows.map((pitch) => { const black = [1, 3, 6, 8, 10].includes(pitch % 12); return <button key={pitch} className={black ? 'black' : 'white'} style={{ height: noteHeight }} onPointerDown={() => onNote(pitch)} onDoubleClick={() => onSelectPitch(pitch)}>{pitch % 12 === 0 ? `C${Math.floor(pitch / 12) - 1}` : ''}</button> })}</div>
}

function setupCanvas(canvas: HTMLCanvasElement | null, width: number, height: number) { if (!canvas) return null; const ratio = Math.min(window.devicePixelRatio || 1, 1.5); if (canvas.width !== Math.ceil(width * ratio) || canvas.height !== Math.ceil(height * ratio)) { canvas.width = Math.ceil(width * ratio); canvas.height = Math.ceil(height * ratio); canvas.style.width = `${width}px`; canvas.style.height = `${height}px` }; const context = canvas.getContext('2d'); context?.setTransform(ratio, 0, 0, ratio, 0, 0); return context }
function drawGrid(canvas: HTMLCanvasElement | null, width: number, height: number, rowHeight: number, ppt: number, grid: number, root: number, scaleName: string) { const context = setupCanvas(canvas, width, height); if (!context) return; context.clearRect(0, 0, width, height); const scale = SCALES[scaleName] ?? SCALES.Major!; for (let row = 0; row < 128; row += 1) { const pitch = 127 - row; const black = [1,3,6,8,10].includes(pitch % 12); const out = !scale.includes((pitch - root + 120) % 12); context.fillStyle = out ? '#10161c' : black ? '#151d24' : '#192129'; context.fillRect(0, row * rowHeight, width, rowHeight - 1) } for (let tick = 0; tick * ppt < width; tick += grid) { const x = tick * ppt; context.strokeStyle = tick % 3840 === 0 ? '#52606b' : tick % 960 === 0 ? '#34414c' : '#26323c'; context.lineWidth = tick % 3840 === 0 ? 1.5 : 1; context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.stroke() } }
function drawNotes(canvas: HTMLCanvasElement | null, notes: MidiNote[], selected: string[], width: number, height: number, rowHeight: number, ppt: number, scroll: HTMLDivElement | null) { const context = setupCanvas(canvas, width, height); if (!context) return; const startTick = Math.max(0, ((scroll?.scrollLeft ?? 0) - GUTTER) / ppt); const endTick = startTick + (scroll?.clientWidth ?? width) / ppt + 960; const start = lowerBound(notes, startTick); context.clearRect(Math.max(0, startTick * ppt - 10), 0, (endTick - startTick) * ppt + 20, height); for (let index = start; index < notes.length; index += 1) { const note = notes[index]!; if (note.startTicks > endTick) break; const x = note.startTicks * ppt; const y = (127 - note.pitch) * rowHeight + 1; const w = Math.max(3, note.lengthTicks * ppt - 1); const velocity = note.velocity / 127; context.fillStyle = note.muted ? '#53606a' : selected.includes(note.id) ? '#f4d35e' : `hsl(193 78% ${34 + velocity * 28}%)`; context.fillRect(x, y, w, rowHeight - 2); context.strokeStyle = selected.includes(note.id) ? '#fff2a6' : '#92e5ff88'; context.strokeRect(x + .5, y + .5, w - 1, rowHeight - 3) } }
function drawOverlay(canvas: HTMLCanvasElement | null, width: number, height: number, playheadX: number, drag: Drag | null, rowHeight: number, ppt: number) { const context = setupCanvas(canvas, width, height); if (!context) return; context.clearRect(0, 0, width, height); if (playheadX >= 0) { context.strokeStyle = '#fff'; context.beginPath(); context.moveTo(playheadX, 0); context.lineTo(playheadX, height); context.stroke() } if (drag) { context.fillStyle = '#fff4a955'; context.strokeStyle = '#fff4a9'; context.fillRect(drag.latestTick * ppt, (127 - drag.latestPitch) * rowHeight + 1, drag.latestLength * ppt, rowHeight - 2); context.strokeRect(drag.latestTick * ppt, (127 - drag.latestPitch) * rowHeight + 1, drag.latestLength * ppt, rowHeight - 2) } }
function drawVelocity(canvas: HTMLCanvasElement | null, notes: MidiNote[], selected: string[], width: number, height: number, ppt: number, scroll: HTMLDivElement | null) { const context = setupCanvas(canvas, width, height); if (!context) return; const startTick = Math.max(0, ((scroll?.scrollLeft ?? 0) - GUTTER) / ppt); const endTick = startTick + (scroll?.clientWidth ?? width) / ppt + 960; context.clearRect(Math.max(0, startTick * ppt - 10), 0, (endTick - startTick) * ppt + 20, height); context.fillStyle = '#12191f'; context.fillRect(0, 0, width, height); for (const note of notes.slice(lowerBound(notes, startTick))) { if (note.startTicks > endTick) break; const x = note.startTicks * ppt; const bar = note.velocity / 127 * (height - 8); context.fillStyle = selected.includes(note.id) ? '#f4d35e' : '#49c8ed'; context.fillRect(x, height - bar, 4, bar) } }
const lowerBound = findVisibleNoteStart
function nearestScalePitch(pitch: number, root: number, scale: number[]) { for (let distance = 0; distance < 12; distance += 1) { for (const candidate of [pitch - distance, pitch + distance]) if (candidate >= 0 && candidate <= 127 && scale.includes((candidate - root + 120) % 12)) return candidate } return Math.max(0, Math.min(127, pitch)) }
