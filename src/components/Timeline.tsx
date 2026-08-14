// Canvas-based arrangement view with Studio One-style track lanes and tool gestures.
import { ChevronDown, GripVertical, Headphones, Layers3, MoreHorizontal, Piano, Plus, Radio } from 'lucide-react'
import { memo, useCallback, useEffect, useMemo, useRef, useState, type DragEvent as ReactDragEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { describeEngineError, gridLabel, secondsPerBar, secondsPerBeat, type AutomationLane, type Clip, type MidiClip, type TimeSignature, type Track } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { seekTo } from '../store/commands'
import { automationOptionsForTrack, clipSourceStep, snapSeconds, useProjectStore, type ClipGainPointRef } from '../store/projectStore'
import { getEffectiveTool, type ToolId, useToolStore } from '../store/toolStore'
import { FloatingPanel, MenuPanel, type MenuItem } from './Menu'
import { SignalBar } from './controls'
import { buildRulerTicks } from './rulerMath'
import { beginPointerReorder } from './pointerReorder'
import { BROWSER_DRAG_TYPE, readBrowserDrag } from './browserPayload'
import { CLIP_GAIN_CURVE_MID_DB, clipFadeAt, createForwardClipGainSampler, dbToLinearFast, prepareClipGainNodes, sampleClipGainDb, type ClipGainNode } from './clipEnvelopeMath'

const HEADER_WIDTH = 188
const MAX_CANVAS_WIDTH = 30_000
/** Keep the playhead this far from the viewport edge before scrolling ahead. */
const FOLLOW_MARGIN = 96
const horizontalCanvasDraws = new Set<() => void>()

type Gesture = {
  tool: ToolId
  mode: 'move' | 'trim-left' | 'trim-right' | 'stretch-left' | 'stretch-right' | 'fade-in' | 'fade-out' | 'fade-in-curve' | 'fade-out-curve' | 'gain-base' | 'gain-point' | 'gain-curve' | 'range' | 'paint' | 'listen'
  startX: number
  startY?: number
  currentX: number
  clipId?: string
  original?: Clip | MidiClip
  midi?: boolean
  pointId?: string
}

type ClipEnvelopeHit = { kind: 'gain-base' | 'gain-point' | 'gain-curve' | 'fade-in-curve' | 'fade-out-curve'; clip: Clip; pointId?: string }

type AudioDropPreview = { startSec: number; left: number; top: number; height: number; targetTrackId?: string; insertIndex?: number; lineTop?: number; name: string }

export function Timeline() {
  const engine = useEngine()
  const tracks = useProjectStore((state) => state.project.tracks)
  const pixelsPerSecond = useProjectStore((state) => state.pixelsPerSecond)
  const trackHeight = useProjectStore((state) => state.trackHeight)
  const gridTicks = useProjectStore((state) => state.gridTicks)
  const snapEnabled = useProjectStore((state) => state.snapEnabled)
  const addTrack = useProjectStore((state) => state.addTrack)
  const addInstrumentTrack = useProjectStore((state) => state.addInstrumentTrack)
  const focused = useProjectStore((state) => state.editFocus === 'arrangement')
  const setEditFocus = useProjectStore((state) => state.setEditFocus)
  const selectTrack = useProjectStore((state) => state.selectTrack)
  const [browserDragOver, setBrowserDragOver] = useState(false)
  const [audioDropPreview, setAudioDropPreview] = useState<AudioDropPreview | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const maxEnd = Math.max(180, ...tracks.flatMap((track) => [...track.clips, ...track.midiClips].map((clip) => clip.startSec + clip.durationSec + 12)))
  const timelineWidth = Math.ceil(maxEnd * pixelsPerSecond)

  useFollowPlayhead(scrollRef)
  useTimelineWheel(scrollRef)
  useHorizontalCanvasScroll(scrollRef)

  useEffect(() => {
    const handleNativeDrag = (raw: Event) => {
      const detail = (raw as CustomEvent<{ type: 'enter' | 'over' | 'drop' | 'leave'; paths?: string[]; x?: number; y?: number }>).detail
      if (detail.type === 'leave') { setBrowserDragOver(false); setAudioDropPreview(null); return }
      if (!detail.paths?.length || detail.x === undefined || detail.y === undefined) return
      const target = document.elementFromPoint(detail.x, detail.y)
      if (!target?.closest('.arrangement')) { setAudioDropPreview(null); return }
      const name = detail.paths[0]!.split(/[\\/]/).at(-1) ?? 'Audio'
      const placement = resolveAudioDrop({ clientX: detail.x, clientY: detail.y, shiftKey: false, target }, tracks, scrollRef.current, pixelsPerSecond, gridTicks, useProjectStore.getState().project.transport.bpm, name)
      setBrowserDragOver(true)
      setAudioDropPreview(placement)
      if (detail.type !== 'drop') return
      setBrowserDragOver(false)
      setAudioDropPreview(null)
      detail.paths.forEach((path, index) => { void engine.loadAudioFile(path).then((asset) => useProjectStore.getState().insertAudioAsset(asset, placement.startSec, index === 0 ? placement.targetTrackId : undefined, placement.insertIndex === undefined ? undefined : placement.insertIndex + index)).catch((error) => useProjectStore.getState().showToast(`외부 오디오를 읽을 수 없습니다: ${String(error)}`)) })
    }
    window.addEventListener('minidaw-native-audio-drag', handleNativeDrag)
    return () => window.removeEventListener('minidaw-native-audio-drag', handleNativeDrag)
  }, [engine, gridTicks, pixelsPerSecond, tracks])

  const onBrowserDrop = (event: ReactDragEvent<HTMLElement>) => {
    const payload = readBrowserDrag(event.dataTransfer)
    setBrowserDragOver(false)
    setAudioDropPreview(null)
    if (!payload) return
    event.preventDefault()
    const store = useProjectStore.getState()
    if (payload.kind === 'instrument') {
      const trackId = store.addInstrumentTrack(payload.plugin)
      store.setRackTarget({ kind: 'track', id: trackId })
      store.showToast(`${payload.plugin?.name ?? 'DefaultSynth'} 트랙을 추가했습니다.`)
      return
    }
    if (payload.kind === 'media') {
      const placement = audioDropPreview ?? resolveAudioDrop(event, tracks, scrollRef.current, pixelsPerSecond, gridTicks, store.project.transport.bpm, payload.name)
      void engine.loadAudioFile(payload.path).then((asset) => store.insertAudioAsset(asset, placement.startSec, placement.targetTrackId, placement.insertIndex)).catch((error) => store.showToast(`${payload.name}을 불러오지 못했습니다: ${String(error)}`))
      return
    }
    const element = event.target instanceof Element ? event.target : null
    const targetId = element?.closest<HTMLElement>('[data-track-id]')?.dataset.trackId ?? store.selectedTrackId
    if (!targetId) { store.showToast('이펙트를 놓을 트랙을 먼저 선택하세요.'); return }
    store.selectTrack(targetId)
    store.addEffect(targetId, payload.type, payload.plugin)
    store.setRackTarget({ kind: 'track', id: targetId })
  }

  return (
    <section
      className={`arrangement ${focused ? 'edit-focused' : ''} ${browserDragOver ? 'browser-drop-active' : ''}`}
      aria-label="Arrangement timeline"
      onPointerDownCapture={(event) => { setEditFocus('arrangement'); if (event.target instanceof Element && !event.target.closest('.track-stack')) selectTrack(null) }}
      onDragEnter={(event) => { if (event.dataTransfer.types.includes(BROWSER_DRAG_TYPE)) setBrowserDragOver(true) }}
      onDragOver={(event) => { if (event.dataTransfer.types.includes(BROWSER_DRAG_TYPE)) { event.preventDefault(); event.dataTransfer.dropEffect = 'copy'; const payload = readBrowserDrag(event.dataTransfer); if (payload?.kind === 'media') setAudioDropPreview(resolveAudioDrop(event, tracks, scrollRef.current, pixelsPerSecond, gridTicks, useProjectStore.getState().project.transport.bpm, payload.name)) } }}
      onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) { setBrowserDragOver(false); setAudioDropPreview(null) } }}
      onDrop={onBrowserDrop}
    >
      <div className="arrangement-topline"><span>ARRANGEMENT</span><div><span className="legend-grid" />그리드 {gridLabel(gridTicks)}{snapEnabled ? '' : ' (스냅 꺼짐)'} <span className="legend-loop" />Ctrl+룰러: 루프 시작 · Alt+룰러: 루프 끝 · Ctrl+휠 줌</div></div>
      <div className="timeline-scroll" ref={scrollRef}>
        <div className="timeline-content" style={{ width: HEADER_WIDTH + timelineWidth }}>
          <div className="ruler-row">
            <div className="track-list-heading"><span>트랙</span><button onClick={addTrack} title="오디오 트랙 추가"><Plus size={14} /></button><button onClick={() => addInstrumentTrack()} title="인스트루먼트 트랙 추가"><Piano size={13} /></button></div>
            <Ruler width={timelineWidth} pixelsPerSecond={pixelsPerSecond} />
          </div>
          <LoopStrip width={timelineWidth} pixelsPerSecond={pixelsPerSecond} />
          {tracks.map((track, index) => { const height = track.height ?? trackHeight; return <div className="track-stack" data-track-id={track.id} key={track.id}>
            <div className="track-row" style={{ height }}>
              <TrackHeader track={track} index={index} />
              <TrackLane track={track} width={timelineWidth} height={height} scrollRef={scrollRef} />
              <TrackResizeHandle track={track} height={height} />
            </div>
            {track.automationOpen && <AutomationSection track={track} width={timelineWidth} pixelsPerSecond={pixelsPerSecond} />}
          </div>})}
          {audioDropPreview && <><div className="audio-drop-preview" style={{ left: audioDropPreview.left, top: audioDropPreview.top, width: 128, height: audioDropPreview.height }}><span>{audioDropPreview.name}</span></div>{audioDropPreview.lineTop !== undefined && <div className="audio-drop-insert-line" style={{ top: audioDropPreview.lineTop }} />}</>}
          <Playhead pixelsPerSecond={pixelsPerSecond} />
          <button className="add-track-row" onClick={addTrack}><Plus size={14} /> 오디오 트랙 추가 · 상단 건반 버튼으로 인스트루먼트 추가</button>
        </div>
      </div>
    </section>
  )
}

function resolveAudioDrop(event: Pick<ReactDragEvent<HTMLElement>, 'clientX' | 'clientY' | 'shiftKey' | 'target'>, tracks: Track[], scroll: HTMLDivElement | null, pixelsPerSecond: number, gridTicks: number, bpm: number, name = 'Audio'): AudioDropPreview {
  const content = scroll?.querySelector<HTMLElement>('.timeline-content')
  const contentRect = content?.getBoundingClientRect()
  const rawSec = Math.max(0, ((event.clientX - (contentRect?.left ?? 0)) - HEADER_WIDTH) / pixelsPerSecond)
  const step = snapSeconds(gridTicks, bpm)
  const startSec = event.shiftKey ? rawSec : Math.round(rawSec / step) * step
  const left = HEADER_WIDTH + startSec * pixelsPerSecond
  const element = event.target instanceof Element ? event.target : null
  const stack = element?.closest<HTMLElement>('.track-stack')
  if (stack && contentRect) {
    const rect = stack.getBoundingClientRect()
    const index = Math.max(0, tracks.findIndex((track) => track.id === stack.dataset.trackId))
    const edge = Math.min(10, rect.height * .18)
    if (event.clientY <= rect.top + edge) return { startSec, left, top: rect.top - contentRect.top, height: Math.max(28, rect.height), insertIndex: index, lineTop: rect.top - contentRect.top, name }
    if (event.clientY >= rect.bottom - edge) return { startSec, left, top: rect.bottom - contentRect.top, height: Math.max(28, rect.height), insertIndex: index + 1, lineTop: rect.bottom - contentRect.top, name }
    const track = tracks[index]
    if (track?.kind === 'audio') return { startSec, left, top: rect.top - contentRect.top, height: Math.max(28, rect.height), targetTrackId: track.id, name }
    return { startSec, left, top: rect.top - contentRect.top, height: Math.max(28, rect.height), insertIndex: index + 1, lineTop: rect.bottom - contentRect.top, name }
  }
  const bottom = contentRect ? Math.max(64, event.clientY - contentRect.top - 20) : 64
  return { startSec, left, top: bottom, height: 54, insertIndex: tracks.length, lineTop: bottom, name }
}

/** One scroll listener for the arrangement; vertical-only motion does no canvas work. */
function useHorizontalCanvasScroll(scrollRef: React.RefObject<HTMLDivElement>): void {
  useEffect(() => {
    const scroll = scrollRef.current
    if (!scroll) return
    let lastLeft = scroll.scrollLeft
    let frame = 0
    const onScroll = () => {
      if (scroll.scrollLeft === lastLeft) return
      lastLeft = scroll.scrollLeft
      if (!frame) frame = requestAnimationFrame(() => { frame = 0; for (const draw of horizontalCanvasDraws) draw() })
    }
    scroll.addEventListener('scroll', onScroll, { passive: true })
    return () => { scroll.removeEventListener('scroll', onScroll); if (frame) cancelAnimationFrame(frame) }
  }, [scrollRef])
}

/** Scrolls the arrangement so a moving playhead stays visible during playback. */
function useFollowPlayhead(scrollRef: React.RefObject<HTMLDivElement>): void {
  useEffect(() => {
    let frame = 0
    const sync = () => {
      frame = 0
      const scroll = scrollRef.current
      const state = useProjectStore.getState()
      if (!scroll || !state.followPlayhead || !state.project.transport.isPlaying) return
      const x = state.playheadSec * state.pixelsPerSecond
      const viewStart = scroll.scrollLeft
      const viewWidth = scroll.clientWidth - HEADER_WIDTH
      // Page ahead once the playhead nears the right edge; jump back if it is behind.
      if (x > viewStart + viewWidth - FOLLOW_MARGIN) scroll.scrollLeft = Math.max(0, x - viewWidth * 0.35)
      else if (x < viewStart) scroll.scrollLeft = Math.max(0, x - FOLLOW_MARGIN)
    }
    sync()
    return useProjectStore.subscribe(() => { if (!frame) frame = requestAnimationFrame(sync) })
  }, [scrollRef])
}

/** Ctrl+wheel zooms around the cursor, Shift+wheel scrolls, Alt+wheel resizes tracks. */
function useTimelineWheel(scrollRef: React.RefObject<HTMLDivElement>): void {
  useEffect(() => {
    const scroll = scrollRef.current
    if (!scroll) return
    const onWheel = (event: WheelEvent) => {
      const store = useProjectStore.getState()
      if (event.ctrlKey || event.metaKey) {
        event.preventDefault()
        const anchorX = event.clientX - scroll.getBoundingClientRect().left - HEADER_WIDTH + scroll.scrollLeft
        const anchorSec = anchorX / store.pixelsPerSecond
        const next = Math.max(4, Math.min(400, store.pixelsPerSecond * (event.deltaY < 0 ? 1.15 : 1 / 1.15)))
        store.setZoom(next)
        // Re-pin the time that was under the cursor after the scale changes.
        requestAnimationFrame(() => { scroll.scrollLeft = Math.max(0, anchorSec * useProjectStore.getState().pixelsPerSecond - (event.clientX - scroll.getBoundingClientRect().left - HEADER_WIDTH)) })
        return
      }
      if (event.altKey) {
        event.preventDefault()
        store.resizeSelectedTracks(event.deltaY < 0 ? 6 : -6)
        return
      }
      if (event.shiftKey && event.deltaX === 0) {
        event.preventDefault()
        scroll.scrollLeft += event.deltaY
      }
    }
    scroll.addEventListener('wheel', onWheel, { passive: false })
    return () => scroll.removeEventListener('wheel', onWheel)
  }, [scrollRef])
}

function Playhead({ pixelsPerSecond }: { pixelsPerSecond: number }) {
  const playheadSec = useProjectStore((state) => state.playheadSec)
  return <div className="playhead" style={{ left: 0, transform: `translate3d(${HEADER_WIDTH + playheadSec * pixelsPerSecond}px, 0, 0)` }}><span /></div>
}

/** Draggable loop region: body moves it, edges resize it, empty lane draws a new one. */
function LoopStrip({ width, pixelsPerSecond }: { width: number; pixelsPerSecond: number }) {
  const loop = useProjectStore((state) => state.project.transport.loop)
  const setLoopRange = useProjectStore((state) => state.setLoopRange)
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const snapEnabled = useProjectStore((state) => state.snapEnabled)
  const gridTicks = useProjectStore((state) => state.gridTicks)
  const snap = (sec: number, bypass = false) => snapEnabled && !bypass ? Math.round(sec / snapSeconds(gridTicks, bpm)) * snapSeconds(gridTicks, bpm) : sec

  const begin = (event: ReactPointerEvent<HTMLElement>, mode: 'start' | 'end' | 'move' | 'draw') => {
    event.preventDefault()
    event.stopPropagation()
    const lane = event.currentTarget.closest('.loop-lane') as HTMLElement | null
    if (!lane) return
    lane.setPointerCapture(event.pointerId)
    const laneLeft = lane.getBoundingClientRect().left
    const originSec = Math.max(0, (event.clientX - laneLeft) / pixelsPerSecond)
    const { startSec, endSec } = loop
    const move = (pointer: PointerEvent) => {
      const sec = Math.max(0, (pointer.clientX - laneLeft) / pixelsPerSecond)
      if (mode === 'start') setLoopRange(snap(sec, pointer.shiftKey), endSec)
      else if (mode === 'end') setLoopRange(startSec, snap(sec, pointer.shiftKey))
      else if (mode === 'draw') setLoopRange(snap(originSec, pointer.shiftKey), snap(sec, pointer.shiftKey))
      else {
        const delta = sec - originSec
        const nextStart = Math.max(0, snap(startSec + delta, pointer.shiftKey))
        const appliedDelta = nextStart - startSec
        setLoopRange(nextStart, snap(endSec + appliedDelta, pointer.shiftKey))
      }
    }
    const up = () => {
      lane.releasePointerCapture(event.pointerId)
      lane.removeEventListener('pointermove', move)
      lane.removeEventListener('pointerup', up)
      lane.removeEventListener('pointercancel', up)
    }
    lane.addEventListener('pointermove', move)
    lane.addEventListener('pointerup', up)
    lane.addEventListener('pointercancel', up)
  }

  return (
    <div className="loop-lane" style={{ left: HEADER_WIDTH, width }} onPointerDown={(event) => begin(event, 'draw')} title="드래그하여 루프 구간 설정">
      <div className={`loop-strip ${loop.enabled ? '' : 'disabled'}`} style={{ left: loop.startSec * pixelsPerSecond, width: Math.max(4, (loop.endSec - loop.startSec) * pixelsPerSecond) }} onPointerDown={(event) => begin(event, 'move')}>
        <b className="loop-handle start" onPointerDown={(event) => begin(event, 'start')} />
        <b className="loop-handle end" onPointerDown={(event) => begin(event, 'end')} />
        <span>LOOP</span>
      </div>
    </div>
  )
}

/** Bar/beat ruler that thins its labels out as the horizontal zoom changes. */
function Ruler({ width, pixelsPerSecond }: { width: number; pixelsPerSecond: number }) {
  const engine = useEngine()
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const signature = useProjectStore((state) => state.project.transport.timeSignature)
  const gridTicks = useProjectStore((state) => state.gridTicks)
  const loop = useProjectStore((state) => state.project.transport.loop)
  const ticks = useMemo(() => buildRulerTicks(width, pixelsPerSecond, bpm, signature), [width, pixelsPerSecond, bpm, signature])
  const point = (event: ReactPointerEvent<HTMLDivElement>) => {
    const rawSec = Math.max(0, (event.clientX - event.currentTarget.getBoundingClientRect().left) / pixelsPerSecond)
    const step = snapSeconds(gridTicks, bpm)
    const sec = Math.round(rawSec / step) * step
    if (event.ctrlKey || event.metaKey) { event.preventDefault(); useProjectStore.getState().setLoopRange(sec, Math.max(loop.endSec, sec + step)); return }
    if (event.altKey) { event.preventDefault(); useProjectStore.getState().setLoopRange(Math.min(loop.startSec, Math.max(0, sec - step)), sec); return }
    seekTo(engine, rawSec)
  }
  return (
    <div className="ruler" style={{ width }} onPointerDown={point} title="클릭: 이동 · Ctrl+클릭: 루프 시작 · Alt+클릭: 루프 끝">
      {ticks.map((tick) => (
        <div className={`ruler-tick ${tick.strong ? 'strong' : ''}`} key={tick.sec} style={{ left: tick.sec * pixelsPerSecond }}>
          {tick.label && <span>{tick.label}</span>}
        </div>
      ))}
    </div>
  )
}


function TrackHeaderView({ track, index }: { track: Track; index: number }) {
  const selected = useProjectStore((state) => state.selectedTrackIds.includes(track.id))
  const selectedTrackIds = useProjectStore((state) => state.selectedTrackIds)
  const selectTrack = useProjectStore((state) => state.selectTrack)
  const updateTrack = useProjectStore((state) => state.updateTrack)
  const reorder = useProjectStore((state) => state.reorderTrack)
  const trackCount = useProjectStore((state) => state.project.tracks.length)
  const setAutomationOpen = useProjectStore((state) => state.setTrackAutomationOpen)
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const [renaming, setRenaming] = useState(false)
  const [nameDraft, setNameDraft] = useState(track.name)
  const nameRef = useRef<HTMLInputElement>(null)
  const engine = useEngine()

  const beginRename = () => { setNameDraft(track.name); setRenaming(true); queueMicrotask(() => { nameRef.current?.focus(); nameRef.current?.select() }) }
  const finishRename = () => { if (!renaming) return; const name = nameDraft.trim(); if (name && name !== track.name) updateTrack(track.id, { name }); setRenaming(false) }

  const menuItems = (): MenuItem[] => {
    const store = useProjectStore.getState()
    const run = (label: string, action: () => void, extra?: Partial<Extract<MenuItem, { kind: 'item' }>>): MenuItem => ({ kind: 'item', label, run: action, ...extra })
    return [
      run('이펙트 체인 열기', () => store.setRackTarget({ kind: 'track', id: track.id })),
      run('이름 바꾸기', beginRename),
      run('트랙 복제', () => store.duplicateTrack(track.id)),
      run('선택 트랙으로 버스 채널 생성', () => { store.createBusFromSelectedTracks() }, { disabled: selectedTrackIds.length < 2, icon: <Layers3 size={13} /> }),
      { kind: 'separator' },
      run('위로 이동', () => reorder(index, index - 1), { disabled: index === 0 }),
      run('아래로 이동', () => reorder(index, index + 1), { disabled: index >= trackCount - 1 }),
      { kind: 'separator' },
      run(track.muted ? '뮤트 해제' : '뮤트', () => { updateTrack(track.id, { muted: !track.muted }); engine.setTrackMute(track.id, !track.muted) }, { checked: track.muted }),
      run(track.solo ? '솔로 해제' : '솔로', () => { updateTrack(track.id, { solo: !track.solo }); engine.setTrackSolo(track.id, !track.solo) }, { checked: track.solo }),
      { kind: 'separator' },
      run('트랙 삭제', () => store.removeTrack(track.id), { danger: true }),
    ]
  }

  return (
    // Only the grip starts a native drag; a draggable header would hijack pointer
    // gestures on the name field and the M/S/arm buttons inside it.
    <div
      className={`track-header ${selected ? 'selected' : ''}`}
      data-track-index={index}
      style={{ '--track-color': track.color } as React.CSSProperties}
      onClick={(event) => selectTrack(track.id, event.shiftKey)}
      onContextMenu={(event) => { event.preventDefault(); if (!selected) selectTrack(track.id); setMenu({ x: event.clientX, y: event.clientY }) }}
    >
      <div className="track-color" />
      <span className="drag-handle" onPointerDown={(event) => beginPointerReorder(event, { itemSelector: '.track-header[data-track-index]', indexAttribute: 'data-track-index', axis: 'vertical', scrollSelector: '.timeline-scroll', onCommit: reorder })} title="드래그하여 트랙 순서 변경"><GripVertical size={13} /></span>
      <div className="track-header-main">
        <div className="track-number">{String(index + 1).padStart(2, '0')}</div>
        <input ref={nameRef} value={renaming ? nameDraft : track.name} readOnly={!renaming} aria-label={`${track.name} 트랙 이름`} onChange={(event) => setNameDraft(event.target.value)} onDoubleClick={(event) => { event.stopPropagation(); beginRename() }} onBlur={finishRename} onKeyDown={(event) => { if (event.key === 'Enter') event.currentTarget.blur(); else if (event.key === 'Escape') { setNameDraft(track.name); setRenaming(false); event.currentTarget.blur() } }} onClick={(event) => event.stopPropagation()} />
        <button title="트랙 메뉴" aria-haspopup="menu" onClick={(event) => { event.stopPropagation(); const rect = event.currentTarget.getBoundingClientRect(); setMenu({ x: rect.left, y: rect.bottom + 2 }) }}><MoreHorizontal size={13} /></button>
        {menu && <MenuPanel items={menuItems()} anchor={menu} onClose={() => setMenu(null)} />}
      </div>
      <div className="track-header-controls">
        <button className={`automation-toggle ${track.automationOpen ? 'active' : ''}`} title="오토메이션 레인 토글" onClick={(event) => { event.stopPropagation(); const store = useProjectStore.getState(); if (!track.automationOpen && !(track.automationLanes?.length)) { const volume = automationOptionsForTrack(track).find((option) => option.parameterId === 'volumeDb'); if (volume) store.addAutomationLane(track.id, volume) } setAutomationOpen(track.id, !track.automationOpen) }}><ChevronDown size={10} /></button>
        {track.kind === 'instrument' && <button className="instrument-button" title={`${track.instrument?.type ?? 'VST / Instrument'} 설정 열기`} onClick={(event) => { event.stopPropagation(); selectTrack(track.id); useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id }) }}><Piano size={11} /></button>}
        <button className={track.muted ? 'active mute' : ''} onClick={(event) => { event.stopPropagation(); updateTrack(track.id, { muted: !track.muted }); engine.setTrackMute(track.id, !track.muted) }}>M</button>
        <button className={track.solo ? 'active solo' : ''} onClick={(event) => { event.stopPropagation(); updateTrack(track.id, { solo: !track.solo }); engine.setTrackSolo(track.id, !track.solo) }}>S</button>
        <button className={track.armed ? 'active arm' : ''} onClick={(event) => { event.stopPropagation(); updateTrack(track.id, { armed: !track.armed }) }}><Radio size={10} /></button>
      </div>
      <SignalBar trackId={track.id} kind={track.kind} />
    </div>
  )
}

const TrackHeader = memo(TrackHeaderView)

function TrackResizeHandle({ track, height }: { track: Track; height: number }) {
  const setHeight = useProjectStore((state) => state.setTrackViewHeight)
  const begin = (event: ReactPointerEvent<HTMLButtonElement>) => {
    event.preventDefault(); event.stopPropagation()
    const originY = event.clientY
    const originHeight = height
    const element = event.currentTarget
    element.setPointerCapture(event.pointerId)
    const move = (pointer: PointerEvent) => setHeight(track.id, originHeight + pointer.clientY - originY)
    const finish = () => {
      element.removeEventListener('pointermove', move)
      element.removeEventListener('pointerup', finish)
      element.removeEventListener('pointercancel', finish)
    }
    element.addEventListener('pointermove', move)
    element.addEventListener('pointerup', finish)
    element.addEventListener('pointercancel', finish)
  }
  return <button className="track-resize-handle" aria-label={`${track.name} 높이 조절`} title="드래그하여 트랙 높이 조절" onPointerDown={begin} />
}

const AUTOMATION_HEIGHT = 54

function AutomationSection({ track, width, pixelsPerSecond }: { track: Track; width: number; pixelsPerSecond: number }) {
  const lanes = useMemo(() => track.automationLanes ?? [], [track.automationLanes])
  const addLane = useProjectStore((state) => state.addAutomationLane)
  const removeLane = useProjectStore((state) => state.removeAutomationLane)
  const [pickerOpen, setPickerOpen] = useState(false)
  const [query, setQuery] = useState('')
  const addButton = useRef<HTMLButtonElement>(null)
  const getAnchor = useCallback(() => addButton.current, [])
  const existing = useMemo(() => new Set(lanes.map((lane) => `${lane.targetKind}:${lane.targetId}:${lane.parameterId}`)), [lanes])
  const options = useMemo(() => automationOptionsForTrack(track).filter((option) => !existing.has(`${option.targetKind}:${option.targetId}:${option.parameterId}`) && `${option.category} ${option.label}`.toLowerCase().includes(query.toLowerCase())), [existing, query, track])
  const categories = useMemo(() => options.reduce((groups, option) => {
    const items = groups.get(option.category) ?? []
    items.push(option)
    groups.set(option.category, items)
    return groups
  }, new Map<string, typeof options>()), [options])
  return <div className="automation-section">
    {!lanes.length && <div className="automation-row automation-empty-row" style={{ height: AUTOMATION_HEIGHT }}><div className="automation-lane-header"><span className="automation-color" style={{ background: track.color }} /><div><small>AUTOMATION</small><strong>레인을 추가하세요</strong></div><button ref={addButton} className="automation-add" title="오토메이션 파라미터 추가" onClick={() => setPickerOpen((open) => !open)}><Plus size={11} /></button></div><div className="automation-empty-canvas" style={{ width }} /></div>}
    {lanes.map((lane, index) => { const laneHeight = lane.height ?? AUTOMATION_HEIGHT; return <div className="automation-row" key={lane.id} style={{ height: laneHeight }}>
      <div className="automation-lane-header">
        <span className="automation-color" style={{ background: track.color }} />
        <div><small>{lane.category}</small><strong>{lane.label}</strong></div>
        {index === 0 && <button ref={addButton} className="automation-add" title="오토메이션 파라미터 추가" onClick={() => setPickerOpen((open) => !open)}><Plus size={11} /></button>}
        <button className="automation-remove" title="오토메이션 레인 제거" onClick={() => removeLane(track.id, lane.id)}><span>×</span></button>
      </div>
      <AutomationCurve trackId={track.id} lane={lane} width={width} height={laneHeight} pixelsPerSecond={pixelsPerSecond} />
      <AutomationResizeHandle trackId={track.id} laneId={lane.id} height={laneHeight} />
    </div>})}
    {pickerOpen && <FloatingPanel getAnchorElement={getAnchor} onClose={() => setPickerOpen(false)} className="automation-picker">
      <header><strong>AUTOMATION</strong><span>{track.name}</span></header>
      <input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="파라미터 검색" />
      <div>{[...categories].map(([category, items]) => <section key={category}><strong>{category}</strong>{items.map((option) => <button key={`${option.targetKind}:${option.targetId}:${option.parameterId}`} onClick={() => { addLane(track.id, option); setPickerOpen(false); setQuery('') }}><span>{option.label}</span><small>{option.parameterId}</small></button>)}</section>)}{!options.length && <small className="automation-empty">추가할 파라미터가 없습니다.</small>}</div>
    </FloatingPanel>}
  </div>
}

function AutomationResizeHandle({ trackId, laneId, height }: { trackId: string; laneId: string; height: number }) {
  const setHeight = useProjectStore((state) => state.setAutomationLaneHeight)
  const begin = (event: ReactPointerEvent<HTMLButtonElement>) => {
    event.preventDefault(); event.stopPropagation()
    const originY = event.clientY; const element = event.currentTarget
    element.setPointerCapture(event.pointerId)
    const move = (pointer: PointerEvent) => setHeight(trackId, laneId, height + pointer.clientY - originY)
    const finish = () => {
      element.removeEventListener('pointermove', move)
      element.removeEventListener('pointerup', finish)
      element.removeEventListener('pointercancel', finish)
    }
    element.addEventListener('pointermove', move)
    element.addEventListener('pointerup', finish)
    element.addEventListener('pointercancel', finish)
  }
  return <button className="automation-resize-handle" aria-label="오토메이션 레인 높이 조절" title="드래그하여 오토메이션 레인 높이 조절" onPointerDown={begin} />
}

function AutomationCurve({ trackId, lane, width, height, pixelsPerSecond }: { trackId: string; lane: AutomationLane; width: number; height: number; pixelsPerSecond: number }) {
  const upsert = useProjectStore((state) => state.upsertAutomationPoint)
  const setCurve = useProjectStore((state) => state.setAutomationCurve)
  const selectedPoints = useProjectStore((state) => state.selectedAutomationPoints)
  const svgRef = useRef<SVGSVGElement>(null)
  const dragging = useRef<{ kind: 'point'; id: string; pointerId: number } | { kind: 'curve'; id: string; pointerId: number; linearY: number } | null>(null)
  const hoverTimer = useRef<number>(0)
  const [hoveredSegment, setHoveredSegment] = useState<string | null>(null)
  const [pointMenu, setPointMenu] = useState<{ x: number; y: number; pointId: string } | null>(null)
  const valueY = useCallback((value: number) => 5 + (1 - (value - lane.min) / Math.max(0.0001, lane.max - lane.min)) * (height - 10), [height, lane.max, lane.min])
  const pointAt = useCallback((clientX: number, clientY: number, bypassSnap = false) => {
    const bounds = svgRef.current?.getBoundingClientRect()
    if (!bounds) return { timeSec: 0, value: lane.defaultValue }
    const x = Math.max(0, Math.min(bounds.width, clientX - bounds.left))
    const y = Math.max(5, Math.min(height - 5, clientY - bounds.top))
    const rawTime = x / pixelsPerSecond
    const state = useProjectStore.getState()
    const step = state.snapEnabled && !bypassSnap ? snapSeconds(state.gridTicks, state.project.transport.bpm) : 0
    const timeSec = step ? Math.round(rawTime / step) * step : rawTime
    const value = lane.max - (y - 5) / (height - 10) * (lane.max - lane.min)
    return { timeSec, value }
  }, [height, lane.defaultValue, lane.max, lane.min, pixelsPerSecond])
  const sorted = [...lane.points].sort((left, right) => left.timeSec - right.timeSec)
  const segments = sorted.slice(0, -1).map((from, index) => {
    const to = sorted[index + 1]!
    const x0 = from.timeSec * pixelsPerSecond; const x1 = to.timeSec * pixelsPerSecond
    const y0 = valueY(from.value); const y1 = valueY(to.value)
    const curve = Math.max(-1, Math.min(1, from.curve ?? 0))
    const handleX = (x0 + x1) / 2
    const linearY = (y0 + y1) / 2
    const handleY = linearY - curve * (height - 10) * .35
    const controlY = 2 * handleY - linearY
    return { from, to, x0, x1, y0, y1, curve, handleX, handleY, linearY, path: Math.abs(curve) < .001 ? `M${x0} ${y0} L${x1} ${y1}` : `M${x0} ${y0} Q${handleX} ${controlY} ${x1} ${y1}` }
  })
  const path = sorted.length ? `M0 ${valueY(sorted[0]!.value)} L${sorted[0]!.timeSec * pixelsPerSecond} ${valueY(sorted[0]!.value)} ${segments.map((segment) => segment.path.replace(/^M[^LQ]+/, '')).join(' ')} L${width} ${valueY(sorted.at(-1)!.value)}` : `M0 ${valueY(lane.defaultValue)} L${width} ${valueY(lane.defaultValue)}`
  const move = (event: ReactPointerEvent<SVGSVGElement>) => {
    const drag = dragging.current
    if (!drag || drag.pointerId !== event.pointerId) return
    if (drag.kind === 'point') upsert(trackId, lane.id, { id: drag.id, ...pointAt(event.clientX, event.clientY, event.shiftKey) })
    else {
      const bounds = svgRef.current?.getBoundingClientRect()
      if (!bounds) return
      const y = Math.max(5, Math.min(height - 5, event.clientY - bounds.top))
      setCurve(trackId, lane.id, drag.id, -(y - drag.linearY) / ((height - 10) * .35))
    }
  }
  const finish = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (dragging.current?.pointerId === event.pointerId) dragging.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  const beginSegmentHover = (id: string) => {
    window.clearTimeout(hoverTimer.current)
    hoverTimer.current = window.setTimeout(() => setHoveredSegment(id), 300)
  }
  const endSegmentHover = (id: string) => {
    window.clearTimeout(hoverTimer.current)
    hoverTimer.current = window.setTimeout(() => {
      if (Math.abs(sorted.find((point) => point.id === id)?.curve ?? 0) < .001) setHoveredSegment((current) => current === id ? null : current)
    }, 160)
  }
  const selected = new Set(selectedPoints.filter((point) => point.trackId === trackId && point.laneId === lane.id).map((point) => point.pointId))
  const menuPoint = pointMenu ? lane.points.find((point) => point.id === pointMenu.pointId) : undefined
  const pointMenuItems: MenuItem[] = menuPoint ? [
    { kind: 'label', label: lane.label },
    { kind: 'item', label: '수치 입력…', run: () => { const value = promptClippedNumber(lane.label, menuPoint.value, lane.min, lane.max); if (value !== null) upsert(trackId, lane.id, { id: menuPoint.id, timeSec: menuPoint.timeSec, value }) } },
    { kind: 'item', label: '시간 입력…', run: () => { const timeSec = promptClippedNumber('오토메이션 시간 (초)', menuPoint.timeSec, 0, Math.max(3600, menuPoint.timeSec)); if (timeSec !== null) upsert(trackId, lane.id, { id: menuPoint.id, timeSec, value: menuPoint.value }) } },
    { kind: 'separator' },
    { kind: 'item', label: '포인트 삭제', keys: 'Delete', danger: true, run: () => useProjectStore.getState().deleteSelectedAutomationPoints() },
  ] : []
  return <><svg ref={svgRef} className="automation-curve" width={width} height={height} onDoubleClick={(event) => { if ((event.target as Element).closest('circle')) return; const id = upsert(trackId, lane.id, pointAt(event.clientX, event.clientY, event.shiftKey)); if (id) useProjectStore.getState().selectAutomationPoint({ trackId, laneId: lane.id, pointId: id }) }} onPointerMove={move} onPointerUp={finish} onPointerCancel={finish} onPointerLeave={() => { window.clearTimeout(hoverTimer.current) }}>
    <path d={path} />
    {segments.map((segment) => <path key={`hit:${segment.from.id}`} className="automation-segment-hit" d={segment.path} onPointerEnter={() => beginSegmentHover(segment.from.id)} onPointerLeave={() => endSegmentHover(segment.from.id)} />)}
    {segments.map((segment) => <g key={`curve:${segment.from.id}`}>
      <circle className="automation-curve-handle-hit" cx={segment.handleX} cy={segment.handleY} r="14" onPointerEnter={() => { window.clearTimeout(hoverTimer.current); setHoveredSegment(segment.from.id) }} onPointerLeave={() => endSegmentHover(segment.from.id)} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation(); window.clearTimeout(hoverTimer.current); setHoveredSegment(segment.from.id); dragging.current = { kind: 'curve', id: segment.from.id, pointerId: event.pointerId, linearY: segment.linearY }; event.currentTarget.ownerSVGElement?.setPointerCapture(event.pointerId) }} onDoubleClick={(event) => { event.preventDefault(); event.stopPropagation(); setCurve(trackId, lane.id, segment.from.id, 0) }} />
      {(hoveredSegment === segment.from.id || Math.abs(segment.curve) >= .001) && <circle className="automation-curve-handle" cx={segment.handleX} cy={segment.handleY} r="4" pointerEvents="none"><title>드래그하여 구간 곡률 조절 · 더블클릭 리셋</title></circle>}
    </g>)}
    {sorted.map((point) => <circle key={point.id} className={selected.has(point.id) ? 'selected' : ''} cx={point.timeSec * pixelsPerSecond} cy={valueY(point.value)} r={selected.has(point.id) ? 5 : 4} onPointerDown={(event) => { event.stopPropagation(); let pointId = point.id; if (event.altKey) pointId = upsert(trackId, lane.id, { timeSec: point.timeSec, value: point.value }) ?? point.id; useProjectStore.getState().selectAutomationPoint({ trackId, laneId: lane.id, pointId }, event.ctrlKey || event.metaKey || event.shiftKey); dragging.current = { kind: 'point', id: pointId, pointerId: event.pointerId }; event.currentTarget.ownerSVGElement?.setPointerCapture(event.pointerId) }} onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); useProjectStore.getState().selectAutomationPoint({ trackId, laneId: lane.id, pointId: point.id }); setPointMenu({ x: event.clientX, y: event.clientY, pointId: point.id }) }}><title>{`${point.value.toFixed(2)} · ${point.timeSec.toFixed(2)} s`}</title></circle>)}
  </svg>{pointMenu && <MenuPanel items={pointMenuItems} anchor={pointMenu} onClose={() => setPointMenu(null)} />}</>
}

function TrackLaneView({ track, width, height, scrollRef }: { track: Track; width: number; height: number; scrollRef: React.RefObject<HTMLDivElement> }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const gestureRef = useRef<Gesture | null>(null)
  const listenPlayRef = useRef<Promise<void> | null>(null)
  const dropLaneRef = useRef<HTMLElement | null>(null)
  const [overlay, setOverlay] = useState<{ start: number; end: number; kind: 'range' | 'paint' } | null>(null)
  const [cursor, setCursor] = useState('default')
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; sec: number; hit: ReturnType<typeof hitTestTrack>; envelope?: ClipEnvelopeHit } | null>(null)
  const [clipProperties, setClipProperties] = useState<{ x: number; y: number; clipId: string } | null>(null)
  const assets = useProjectStore((state) => state.project.assets)
  const selectedClipIds = useProjectStore((state) => state.selectedClipIds)
  const selectedClipGainPoint = useProjectStore((state) => state.selectedClipGainPoint)
  const pixelsPerSecond = useProjectStore((state) => state.pixelsPerSecond)
  const snapEnabled = useProjectStore((state) => state.snapEnabled)
  const gridTicks = useProjectStore((state) => state.gridTicks)
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const signature = useProjectStore((state) => state.project.transport.timeSignature)
  const engine = useEngine()
  const drawFrameRef = useRef(0)
  const latestDrawRef = useRef<() => void>(() => undefined)
  latestDrawRef.current = () => {
    const canvas = canvasRef.current
    if (canvas) drawTrackLane(canvas, track, assets, selectedClipIds, selectedClipGainPoint, pixelsPerSecond, bpm, signature, gridTicks, scrollRef.current)
  }
  const requestDraw = useCallback(() => {
    if (drawFrameRef.current) return
    drawFrameRef.current = requestAnimationFrame(() => { drawFrameRef.current = 0; latestDrawRef.current() })
  }, [])

  useEffect(() => {
    requestDraw()
  }, [track, assets, selectedClipIds, selectedClipGainPoint, pixelsPerSecond, bpm, signature, gridTicks, width, height, scrollRef, requestDraw])

  useEffect(() => {
    horizontalCanvasDraws.add(requestDraw)
    return () => { horizontalCanvasDraws.delete(requestDraw); if (drawFrameRef.current) cancelAnimationFrame(drawFrameRef.current) }
  }, [requestDraw])

  const xToSec = (x: number) => x / pixelsPerSecond
  const snap = (sec: number, bypass = false) => {
    if (!snapEnabled || bypass) return sec
    const step = snapSeconds(gridTicks, bpm)
    return Math.round(sec / step) * step
  }
  const pointX = (event: ReactPointerEvent<HTMLCanvasElement>) => event.clientX - event.currentTarget.getBoundingClientRect().left
  const setDropLane = (lane: HTMLElement | null, valid: boolean) => {
    if (dropLaneRef.current !== lane) dropLaneRef.current?.classList.remove('clip-drop-target', 'invalid')
    dropLaneRef.current = lane
    lane?.classList.toggle('clip-drop-target', valid)
    lane?.classList.toggle('invalid', !valid)
  }

  const onPointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    if (event.button !== 0) return
    const store = useProjectStore.getState()
    const x = pointX(event)
    const sec = xToSec(x)
    const hit = hitTestTrack(track, x, pixelsPerSecond)
    const envelope = hit && !hit.midi ? hitTestClipEnvelope(track, x, event.clientY - event.currentTarget.getBoundingClientRect().top, pixelsPerSecond, height, selectedClipIds) : null
    const tool = useToolStore.getState().latchGesture()
    event.currentTarget.setPointerCapture(event.pointerId)
    if (!store.selectedTrackIds.includes(track.id)) store.selectTrack(track.id)

    if (tool === 'split') {
      if (hit) store.splitClip(track.id, hit.clip.id, snap(sec, event.shiftKey))
      useToolStore.getState().releaseGesture(); return
    }
    if (tool === 'erase') {
      if (hit) { store.selectClip(hit.clip.id); store.deleteSelectedClips() }
      useToolStore.getState().releaseGesture(); return
    }
    if (tool === 'mute') {
      if (hit) store.updateClip(track.id, hit.clip.id, { muted: !hit.clip.muted })
      useToolStore.getState().releaseGesture(); return
    }
    if (tool === 'listen') {
      gestureRef.current = { tool, mode: 'listen', startX: x, currentX: x }
      store.setPlayhead(sec)
      const start = engine.play(sec)
      listenPlayRef.current = start
      void start.then(() => { if (listenPlayRef.current === start) store.setPlaying(true) }).catch((error) => store.showToast(`오디션 재생 실패: ${describeEngineError(error)}`))
      return
    }
    if (tool === 'paint') {
      gestureRef.current = { tool, mode: 'paint', startX: x, currentX: x }
      setOverlay({ start: x, end: x, kind: 'paint' }); return
    }
    if (tool === 'range' || !hit) {
      gestureRef.current = { tool, mode: 'range', startX: x, currentX: x }
      setOverlay({ start: x, end: x, kind: 'range' })
      if (!event.ctrlKey && !event.metaKey) store.clearClipSelection()
      return
    }

    if (tool === 'arrow' && envelope) {
      let pointId = envelope.pointId
      if (envelope.kind === 'gain-point' && pointId && event.altKey) {
        const point = envelope.clip.gainPoints?.find((item) => item.id === pointId)
        if (point) pointId = store.upsertClipGainPoint(track.id, envelope.clip.id, { timeSec: point.timeSec, valueDb: point.valueDb }) ?? pointId
      }
      if ((envelope.kind === 'gain-point' || envelope.kind === 'gain-curve') && pointId) store.selectClipGainPoint({ trackId: track.id, clipId: envelope.clip.id, pointId })
      else store.selectClipGainPoint(null)
      gestureRef.current = { tool, mode: envelope.kind, startX: x, startY: event.clientY - event.currentTarget.getBoundingClientRect().top, currentX: x, clipId: envelope.clip.id, pointId, original: { ...envelope.clip, gainPoints: envelope.clip.gainPoints?.map((point) => ({ ...point })) }, midi: false }
      return
    }

    const relativeY = event.clientY - event.currentTarget.getBoundingClientRect().top
    let mode: Gesture['mode'] = hit.edge === 'left' ? 'trim-left' : hit.edge === 'right' ? 'trim-right' : 'move'
    if (!hit.midi && event.altKey && hit.edge === 'left') mode = 'stretch-left'
    else if (!hit.midi && event.altKey && hit.edge === 'right') mode = 'stretch-right'
    else if (!hit.midi && relativeY <= 14 && hit.edge === 'left') mode = 'fade-in'
    else if (!hit.midi && relativeY <= 14 && hit.edge === 'right') mode = 'fade-out'
    let targetClip = hit.clip
    if (event.altKey && mode === 'move') {
      const duplicateId = store.duplicateClip(track.id, hit.clip.id)
      if (duplicateId) targetClip = { ...hit.clip, id: duplicateId }
    } else store.selectClip(hit.clip.id, event.ctrlKey || event.metaKey)
    gestureRef.current = { tool, mode, startX: x, currentX: x, clipId: targetClip.id, original: { ...targetClip }, midi: hit.midi }
  }

  const onPointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const store = useProjectStore.getState()
    const x = pointX(event)
    const gesture = gestureRef.current
    if (!gesture) {
      const tool = getEffectiveTool(useToolStore.getState())
      const hit = hitTestTrack(track, x, pixelsPerSecond)
      const envelope = hit && !hit.midi ? hitTestClipEnvelope(track, x, event.clientY - event.currentTarget.getBoundingClientRect().top, pixelsPerSecond, height, selectedClipIds) : null
      setCursor(envelope ? (envelope.kind.includes('curve') ? 'ns-resize' : 'move') : event.altKey && !hit?.midi && (hit?.edge === 'left' || hit?.edge === 'right') ? 'col-resize' : cursorFor(tool, hit?.edge))
      return
    }
    gesture.currentX = x
    if (gesture.mode === 'range' || gesture.mode === 'paint') { setOverlay({ start: gesture.startX, end: x, kind: gesture.mode }); return }
    if (!gesture.original || !gesture.clipId) return
    const delta = (x - gesture.startX) / pixelsPerSecond
    const original = gesture.original
    if (gesture.mode === 'move') {
      store.updateClip(track.id, gesture.clipId, { startSec: Math.max(0, snap(original.startSec + delta, event.shiftKey)) })
      const lane = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('.track-lane') ?? null
      const target = store.project.tracks.find((candidate) => candidate.id === lane?.dataset.trackId)
      const valid = Boolean(target && (gesture.midi ? target.kind === 'instrument' : target.kind === 'audio'))
      setDropLane(lane, valid)
    }
    if (gesture.mode === 'trim-left') {
      const nextStart = Math.min(original.startSec + original.durationSec - 0.1, Math.max(0, snap(original.startSec + delta, event.shiftKey)))
      const consumed = nextStart - original.startSec
      const sourceStep = !gesture.midi && 'offsetSec' in original ? clipSourceStep(original, bpm) : 1
      store.updateClip(track.id, gesture.clipId, { startSec: nextStart, ...(!gesture.midi && 'offsetSec' in original ? { offsetSec: Math.max(0, original.offsetSec + consumed * sourceStep), gainPoints: (original.gainPoints ?? []).filter((point) => point.timeSec >= consumed).map((point) => ({ ...point, timeSec: point.timeSec - consumed })) } : {}), durationSec: original.durationSec - consumed })
    }
    if (gesture.mode === 'trim-right') { const durationSec = Math.max(0.1, snap(original.startSec + original.durationSec + delta, event.shiftKey) - original.startSec); store.updateClip(track.id, gesture.clipId, { durationSec, ...(!gesture.midi && 'gainPoints' in original ? { gainPoints: (original.gainPoints ?? []).filter((point) => point.timeSec <= durationSec) } : {}) }) }
    if (gesture.mode === 'stretch-left' && !gesture.midi && 'offsetSec' in original) {
      const end = original.startSec + original.durationSec
      const nextStart = Math.max(0, Math.min(end - 0.1, snap(original.startSec + delta, event.shiftKey)))
      const durationSec = end - nextStart
      store.updateClip(track.id, gesture.clipId, { startSec: nextStart, durationSec, playbackRate: Math.max(.05, Math.min(8, (original.playbackRate ?? 1) * original.durationSec / durationSec)), gainPoints: (original.gainPoints ?? []).map((point) => ({ ...point, timeSec: point.timeSec * durationSec / original.durationSec })) })
    }
    if (gesture.mode === 'stretch-right' && !gesture.midi && 'offsetSec' in original) {
      const durationSec = Math.max(0.1, snap(original.startSec + original.durationSec + delta, event.shiftKey) - original.startSec)
      store.updateClip(track.id, gesture.clipId, { durationSec, playbackRate: Math.max(.05, Math.min(8, (original.playbackRate ?? 1) * original.durationSec / durationSec)), gainPoints: (original.gainPoints ?? []).map((point) => ({ ...point, timeSec: point.timeSec * durationSec / original.durationSec })) })
    }
    if (gesture.mode === 'fade-in' && !gesture.midi) store.updateClip(track.id, gesture.clipId, { fadeInSec: Math.max(0, Math.min(original.durationSec, xToSec(x) - original.startSec)) })
    if (gesture.mode === 'fade-out' && !gesture.midi) store.updateClip(track.id, gesture.clipId, { fadeOutSec: Math.max(0, Math.min(original.durationSec, original.startSec + original.durationSec - xToSec(x))) })
    if (gesture.mode === 'gain-base' && !gesture.midi && 'gainDb' in original) store.updateClipGain(track.id, gesture.clipId, Math.max(-60, Math.min(0, original.gainDb - (event.clientY - event.currentTarget.getBoundingClientRect().top - (gesture.startY ?? 0)) * 60 / Math.max(1, height - 10))))
    if (gesture.mode === 'gain-point' && !gesture.midi && gesture.pointId && 'gainDb' in original) {
      const rawLocal = Math.max(0, Math.min(original.durationSec, xToSec(x) - original.startSec))
      const step = snapSeconds(gridTicks, bpm)
      const timeSec = snapEnabled && !event.shiftKey ? Math.round(rawLocal / step) * step : rawLocal
      store.upsertClipGainPoint(track.id, gesture.clipId, { id: gesture.pointId, timeSec, valueDb: yToClipGainDb(original, timeSec, event.clientY - event.currentTarget.getBoundingClientRect().top, height) })
    }
    if (gesture.mode === 'gain-curve' && !gesture.midi && gesture.pointId) {
      const audio = original as Clip; const nodes = prepareClipGainNodes(audio); const index = nodes.findIndex((node) => node.id === gesture.pointId); const from = nodes[index]; const to = nodes[index + 1]
      if (from?.id && to) { const localSec = (from.timeSec + to.timeSec) / 2; const targetDb = yToClipGainDb(audio, localSec, event.clientY - event.currentTarget.getBoundingClientRect().top, height); const curve = (targetDb - (from.valueDb + to.valueDb) / 2) / CLIP_GAIN_CURVE_MID_DB; store.upsertClipGainPoint(track.id, gesture.clipId, { id: from.id, timeSec: from.timeSec, valueDb: from.valueDb, curve }) }
    }
    if (gesture.mode === 'fade-in-curve' && !gesture.midi) { const audio = original as Clip; store.updateClip(track.id, gesture.clipId, { fadeInCurve: Math.max(-1, Math.min(1, (audio.fadeInCurve ?? 0) + (event.clientY - event.currentTarget.getBoundingClientRect().top - (gesture.startY ?? 0)) / Math.max(10, height) * 2)) }) }
    if (gesture.mode === 'fade-out-curve' && !gesture.midi) { const audio = original as Clip; store.updateClip(track.id, gesture.clipId, { fadeOutCurve: Math.max(-1, Math.min(1, (audio.fadeOutCurve ?? 0) + (event.clientY - event.currentTarget.getBoundingClientRect().top - (gesture.startY ?? 0)) / Math.max(10, height) * 2)) }) }
  }

  const onPointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const store = useProjectStore.getState()
    const gesture = gestureRef.current
    if (gesture?.mode === 'paint') {
      const start = snap(xToSec(Math.min(gesture.startX, gesture.currentX)), event.shiftKey)
      const end = snap(xToSec(Math.max(gesture.startX, gesture.currentX)), event.shiftKey)
      if (end - start > 0.05) {
        if (track.kind === 'instrument') store.addMidiClip(track.id, start, end - start)
        else store.addSilentClip(track.id, start, end - start)
      }
    }
    if (gesture?.mode === 'range') {
      const start = xToSec(Math.min(gesture.startX, gesture.currentX))
      const end = xToSec(Math.max(gesture.startX, gesture.currentX))
      for (const clip of [...track.clips, ...track.midiClips].filter((item) => item.startSec < end && item.startSec + item.durationSec > start)) store.selectClip(clip.id, true)
    }
    if (gesture?.mode === 'listen') {
      const start = listenPlayRef.current
      listenPlayRef.current = null
      void (start ?? Promise.resolve()).then(() => engine.pause()).catch(() => undefined)
      store.setPlaying(false)
    }
    if (gesture?.mode === 'move' && gesture.clipId && dropLaneRef.current?.classList.contains('clip-drop-target')) {
      const targetTrackId = dropLaneRef.current.dataset.trackId
      if (targetTrackId && targetTrackId !== track.id) store.moveClipToTrack(track.id, targetTrackId, gesture.clipId)
    }
    dropLaneRef.current?.classList.remove('clip-drop-target', 'invalid')
    dropLaneRef.current = null
    gestureRef.current = null
    setOverlay(null)
    useToolStore.getState().releaseGesture()
    event.currentTarget.releasePointerCapture(event.pointerId)
  }

  const onCanvasDoubleClick = (event: React.MouseEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect(); const x = event.clientX - rect.left; const y = event.clientY - rect.top
    const midi = [...track.midiClips].reverse().find((clip) => x >= clip.startSec * pixelsPerSecond && x <= (clip.startSec + clip.durationSec) * pixelsPerSecond)
    if (midi) { useProjectStore.getState().openMidiEditor(track.id, midi.id); return }
    const audio = hitTest(track.clips, x, pixelsPerSecond)?.clip
    if (!audio || hitTestClipEnvelope(track, x, y, pixelsPerSecond, height, selectedClipIds)) return
    const localSec = Math.max(0, Math.min(audio.durationSec, x / pixelsPerSecond - audio.startSec))
    if (Math.abs(y - envelopeYAt(audio, localSec, height)) > 8) return
    const step = snapSeconds(gridTicks, bpm); const timeSec = snapEnabled && !event.shiftKey ? Math.round(localSec / step) * step : localSec
    const id = useProjectStore.getState().upsertClipGainPoint(track.id, audio.id, { timeSec, valueDb: sampleClipGainDb(prepareClipGainNodes(audio), localSec) })
    if (id) useProjectStore.getState().selectClipGainPoint({ trackId: track.id, clipId: audio.id, pointId: id })
  }

  const onCanvasContextMenu = (event: React.MouseEvent<HTMLCanvasElement>) => {
    event.preventDefault(); const store = useProjectStore.getState()
    if (!store.selectedTrackIds.includes(track.id)) store.selectTrack(track.id)
    const rect = event.currentTarget.getBoundingClientRect(); const x = event.clientX - rect.left; const y = event.clientY - rect.top
    const sec = Math.max(0, snap(xToSec(x), event.shiftKey)); const hit = hitTestTrack(track, x, pixelsPerSecond)
    const envelope = hit && !hit.midi ? hitTestClipEnvelope(track, x, y, pixelsPerSecond, height, selectedClipIds) ?? undefined : undefined
    if ((envelope?.kind === 'gain-point' || envelope?.kind === 'gain-curve') && envelope.pointId) store.selectClipGainPoint({ trackId: track.id, clipId: envelope.clip.id, pointId: envelope.pointId })
    else if (envelope) store.selectClipGainPoint(null); else if (hit) store.selectClip(hit.clip.id)
    setContextMenu({ x: event.clientX, y: event.clientY, sec, hit, envelope })
  }

  const contextItems: MenuItem[] = contextMenu?.envelope ? contextMenu.envelope.kind === 'gain-base' ? [
    { kind: 'label', label: '기본 클립 게인' },
    { kind: 'item', label: `${contextMenu.envelope.clip.gainDb.toFixed(1)} dB`, disabled: true, run: () => undefined },
    { kind: 'item', label: '게인 수치 입력…', run: () => { const value = promptClippedNumber('클립 게인 (dB)', contextMenu.envelope!.clip.gainDb, -60, 0); if (value !== null) useProjectStore.getState().updateClipGain(track.id, contextMenu.envelope!.clip.id, value) } },
    { kind: 'item', label: '0 dB로 초기화', run: () => useProjectStore.getState().updateClipGain(track.id, contextMenu.envelope!.clip.id, 0) },
  ] : contextMenu.envelope.kind === 'gain-point' ? [
    { kind: 'label', label: '클립 게인 오토메이션' },
    { kind: 'item', label: '게인 수치 입력…', run: () => { const envelope = contextMenu.envelope!; const point = envelope.clip.gainPoints?.find((item) => item.id === envelope.pointId); if (!point) return; const valueDb = promptClippedNumber('게인 포인트 (dB)', point.valueDb, -60, 0); if (valueDb !== null) useProjectStore.getState().upsertClipGainPoint(track.id, envelope.clip.id, { ...point, valueDb }) } },
    { kind: 'item', label: '위치 입력…', run: () => { const envelope = contextMenu.envelope!; const point = envelope.clip.gainPoints?.find((item) => item.id === envelope.pointId); if (!point) return; const timeSec = promptClippedNumber('클립 내부 위치 (초)', point.timeSec, 0, envelope.clip.durationSec); if (timeSec !== null) useProjectStore.getState().upsertClipGainPoint(track.id, envelope.clip.id, { ...point, timeSec }) } },
    { kind: 'separator' },
    { kind: 'item', label: '게인 포인트 삭제', keys: 'Delete', danger: true, run: () => { const envelope = contextMenu.envelope!; if (envelope.pointId) useProjectStore.getState().removeClipGainPoint(track.id, envelope.clip.id, envelope.pointId) } },
  ] : contextMenu.envelope.kind === 'gain-curve' ? [
    { kind: 'label', label: '클립 게인 곡률' },
    { kind: 'item', label: '곡률 초기화', run: () => { const envelope = contextMenu.envelope!; const point = envelope.clip.gainPoints?.find((item) => item.id === envelope.pointId); if (point) useProjectStore.getState().upsertClipGainPoint(track.id, envelope.clip.id, { ...point, curve: 0 }) } },
  ] : [
    { kind: 'label', label: contextMenu.envelope.kind === 'fade-in-curve' ? '페이드 인 커브' : '페이드 아웃 커브' },
    { kind: 'item', label: '커브 초기화', run: () => useProjectStore.getState().updateClip(track.id, contextMenu.envelope!.clip.id, contextMenu.envelope!.kind === 'fade-in-curve' ? { fadeInCurve: 0 } : { fadeOutCurve: 0 }) },
  ] : contextMenu?.hit ? [
    { kind: 'item', label: contextMenu.hit.midi ? '피아노롤에서 열기' : '클립 선택', run: () => { const store = useProjectStore.getState(); store.selectClip(contextMenu.hit!.clip.id); if (contextMenu.hit!.midi) store.openMidiEditor(track.id, contextMenu.hit!.clip.id) } },
    { kind: 'item', label: '플레이헤드에서 분할', keys: 'S', run: () => useProjectStore.getState().splitClip(track.id, contextMenu.hit!.clip.id, contextMenu.sec) },
    { kind: 'item', label: '복제', keys: 'Ctrl+D', run: () => useProjectStore.getState().duplicateClip(track.id, contextMenu.hit!.clip.id) },
    { kind: 'item', label: contextMenu.hit.clip.muted ? '뮤트 해제' : '뮤트', keys: 'M', checked: Boolean(contextMenu.hit.clip.muted), run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { muted: !contextMenu.hit!.clip.muted }) },
    ...(!contextMenu.hit.midi ? [{ kind: 'item' as const, label: '피치 · 파인 · 배속…', run: () => setClipProperties({ x: contextMenu.x, y: contextMenu.y, clipId: contextMenu.hit!.clip.id }) }, {
      kind: 'submenu' as const, label: '프로세싱', children: [
        { kind: 'item' as const, label: (contextMenu.hit.clip as Clip).reversed ? '리버스 해제' : '리버스', checked: Boolean((contextMenu.hit.clip as Clip).reversed), run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { reversed: !(contextMenu.hit!.clip as Clip).reversed }) },
        { kind: 'item' as const, label: '노멀라이즈', run: () => { const clip = contextMenu.hit!.clip as Clip; useProjectStore.getState().updateClipGain(track.id, clip.id, normalizeGain(assets[clip.assetId]?.peaks)) } },
        { kind: 'item' as const, label: '피치/배속 초기화', run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { playbackRate: 1, pitchSemitones: 0, fineCents: 0, reversed: false }) },
      ],
    }, {
      kind: 'submenu' as const, label: '워프 설정', children: [
        { kind: 'item' as const, label: '설정 없음', checked: ((contextMenu.hit.clip as Clip).warpMode ?? 'none') === 'none', run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { warpMode: 'none' }) },
        { kind: 'item' as const, label: '프로젝트 템포에 맞춤', checked: (contextMenu.hit.clip as Clip).warpMode === 'project', run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { warpMode: 'project', warpSourceBpm: (contextMenu.hit!.clip as Clip).warpSourceBpm ?? bpm }) },
        { kind: 'item' as const, label: '하프타임', checked: (contextMenu.hit.clip as Clip).warpMode === 'half', run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { warpMode: 'half', warpSourceBpm: (contextMenu.hit!.clip as Clip).warpSourceBpm ?? bpm }) },
        { kind: 'item' as const, label: '더블타임', checked: (contextMenu.hit.clip as Clip).warpMode === 'double', run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { warpMode: 'double', warpSourceBpm: (contextMenu.hit!.clip as Clip).warpSourceBpm ?? bpm }) },
        { kind: 'separator' as const },
        { kind: 'item' as const, label: `소스 템포 ${((contextMenu.hit.clip as Clip).warpSourceBpm ?? bpm).toFixed(1)} BPM…`, run: () => { const clip = contextMenu.hit!.clip as Clip; const value = promptClippedNumber('원본 오디오 템포 (BPM)', clip.warpSourceBpm ?? bpm, 20, 300); if (value !== null) useProjectStore.getState().updateClip(track.id, clip.id, { warpSourceBpm: value }) } },
      ],
    }] : []),
    { kind: 'item', label: '선택 트랙으로 버스 채널 생성', disabled: useProjectStore.getState().selectedTrackIds.length < 2, run: () => { useProjectStore.getState().createBusFromSelectedTracks() } },
    { kind: 'separator' },
    { kind: 'item', label: '클립 삭제', keys: 'Delete', danger: true, run: () => { const store = useProjectStore.getState(); store.selectClip(contextMenu.hit!.clip.id); store.deleteSelectedClips() } },
  ] : contextMenu ? [
    { kind: 'label', label: track.name },
    { kind: 'item', label: '플레이헤드를 여기로', run: () => seekTo(engine, contextMenu.sec) },
    { kind: 'item', label: track.kind === 'instrument' ? 'MIDI 클립 추가' : '빈 오디오 클립 추가', run: () => { const store = useProjectStore.getState(); if (track.kind === 'instrument') store.addMidiClip(track.id, contextMenu.sec, secondsPerBar(bpm, signature)); else store.addSilentClip(track.id, contextMenu.sec, secondsPerBar(bpm, signature)) } },
    { kind: 'item', label: '여기에 붙여넣기', keys: 'Ctrl+V', disabled: !useProjectStore.getState().clipboard, run: () => useProjectStore.getState().pasteClipboard(contextMenu.sec) },
    { kind: 'item', label: '선택 트랙으로 버스 채널 생성', disabled: useProjectStore.getState().selectedTrackIds.length < 2, run: () => { useProjectStore.getState().createBusFromSelectedTracks() } },
    { kind: 'separator' },
    { kind: 'item', label: '한 마디 루프 설정', run: () => useProjectStore.getState().setLoopRange(contextMenu.sec, contextMenu.sec + secondsPerBar(bpm, signature)) },
  ] : []

  return (
    <div className="track-lane" data-track-id={track.id} style={{ width, height }}>
      <canvas ref={canvasRef} width={Math.min(width, MAX_CANVAS_WIDTH)} height={height} style={{ width, height, cursor }} onDoubleClick={onCanvasDoubleClick} onContextMenu={onCanvasContextMenu} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp} />
      {overlay && <div className={`gesture-overlay ${overlay.kind}`} style={{ left: Math.min(overlay.start, overlay.end), width: Math.abs(overlay.end - overlay.start) }} />}
      {track.armed && <span className="input-monitor"><Headphones size={10} /> IN</span>}
      {contextMenu && <MenuPanel items={contextItems} anchor={{ x: contextMenu.x, y: contextMenu.y }} onClose={() => setContextMenu(null)} />}
      {clipProperties && (() => { const clip = track.clips.find((item) => item.id === clipProperties.clipId); return clip ? <AudioClipProperties clip={clip} anchor={clipProperties} onClose={() => setClipProperties(null)} onChange={(patch) => useProjectStore.getState().updateClip(track.id, clip.id, patch)} /> : null })()}
    </div>
  )
}

function AudioClipProperties({ clip, anchor, onClose, onChange }: { clip: Clip; anchor: { x: number; y: number }; onClose(): void; onChange(patch: Partial<Pick<Clip, 'pitchSemitones' | 'fineCents' | 'playbackRate' | 'warpSourceBpm'>>): void }) {
  return <FloatingPanel anchor={anchor} onClose={onClose} className="clip-properties"><header><strong>오디오 클립</strong><button onClick={onClose}>×</button></header>
    <label><span>피치</span><input type="number" min={-48} max={48} step={1} value={clip.pitchSemitones ?? 0} onChange={(event) => onChange({ pitchSemitones: Math.max(-48, Math.min(48, Number(event.target.value) || 0)) })} /><small>semitones</small></label>
    <label><span>파인</span><input type="number" min={-100} max={100} step={1} value={clip.fineCents ?? 0} onChange={(event) => onChange({ fineCents: Math.max(-100, Math.min(100, Number(event.target.value) || 0)) })} /><small>cents</small></label>
    <label><span>배속</span><input type="number" min={.05} max={8} step={.01} value={clip.playbackRate ?? 1} onChange={(event) => onChange({ playbackRate: Math.max(.05, Math.min(8, Number(event.target.value) || 1)) })} /><small>×</small></label>
    {(clip.warpMode ?? 'none') !== 'none' && <label><span>소스 BPM</span><input type="number" min={20} max={300} step={.1} value={clip.warpSourceBpm ?? 120} onChange={(event) => onChange({ warpSourceBpm: Math.max(20, Math.min(300, Number(event.target.value) || 120)) })} /><small>BPM</small></label>}
    <footer>피치와 배속은 고품질 선형 보간 varispeed로 처리됩니다.</footer>
  </FloatingPanel>
}

function normalizeGain(peaks?: Float32Array): number {
  let peak = 0
  if (peaks) for (const value of peaks) peak = Math.max(peak, Math.abs(value))
  return peak > 0.000_001 ? Math.max(-24, Math.min(24, -20 * Math.log10(peak))) : 0
}

function promptClippedNumber(label: string, value: number, min: number, max: number): number | null {
  const entered = window.prompt(`${label}\n범위: ${min} ~ ${max}`, String(Number(value.toFixed(3))))
  if (entered === null) return null
  const parsed = Number(entered)
  return Number.isFinite(parsed) ? Math.max(min, Math.min(max, parsed)) : null
}

const TrackLane = memo(TrackLaneView, (previous, next) => (
  previous.track.id === next.track.id
  && previous.track.color === next.track.color
  && previous.track.armed === next.track.armed
  && previous.track.clips === next.track.clips
  && previous.track.midiClips === next.track.midiClips
  && previous.width === next.width
  && previous.height === next.height
  && previous.scrollRef === next.scrollRef
))

function drawTrackLane(canvas: HTMLCanvasElement, track: Track, assets: Record<string, { durationSec: number; peaks: Float32Array }>, selected: string[], selectedGainPoint: ClipGainPointRef | null, pps: number, bpm: number, signature: TimeSignature, gridTicks: number, scroll: HTMLDivElement | null): void {
  const ratioY = Math.min(window.devicePixelRatio || 1, 1.5)
  const width = canvas.clientWidth
  const height = canvas.clientHeight
  const ratioX = Math.min(ratioY, MAX_CANVAS_WIDTH / Math.max(1, width))
  const pixelWidth = Math.max(1, Math.floor(width * ratioX)); const pixelHeight = Math.max(1, Math.floor(height * ratioY))
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) { canvas.width = pixelWidth; canvas.height = pixelHeight }
  const context = canvas.getContext('2d')
  if (!context) return
  context.setTransform(pixelWidth / Math.max(1, width), 0, 0, pixelHeight / Math.max(1, height), 0, 0)
  const visibleStart = Math.max(0, (scroll?.scrollLeft ?? 0) - HEADER_WIDTH)
  const visibleEnd = Math.min(width, visibleStart + (scroll?.clientWidth ?? width) + 2)
  context.clearRect(visibleStart, 0, visibleEnd - visibleStart, height)
  context.fillStyle = '#171c23'
  context.fillRect(visibleStart, 0, visibleEnd - visibleStart, height)
  // Grid lines follow the shared musical grid, with bar lines drawn brightest.
  const barWidth = secondsPerBar(bpm, signature) * pps
  const gridStep = Math.max(3, gridTicks / 960 * secondsPerBeat(bpm) * pps)
  for (let x = Math.floor(visibleStart / gridStep) * gridStep; x < visibleEnd; x += gridStep) {
    const onBar = barWidth > 0 && Math.abs(x / barWidth - Math.round(x / barWidth)) < 1e-6
    context.fillStyle = onBar ? '#3a4552' : '#232b35'
    context.fillRect(Math.round(x), 0, 1, height)
  }
  for (const clip of track.clips) {
    const x = clip.startSec * pps
    const clipWidth = Math.max(3, clip.durationSec * pps)
    if (x + clipWidth < visibleStart || x > visibleEnd) continue
    const isSelected = selected.includes(clip.id)
    context.save()
    context.globalAlpha = clip.muted ? 0.38 : 1
    context.fillStyle = `${track.color}cc`
    context.strokeStyle = isSelected ? '#ffffff' : track.color
    context.lineWidth = isSelected ? 2 : 1
    context.beginPath(); context.roundRect(x + 1, 4, clipWidth - 2, height - 8, 3); context.fill(); context.stroke()
    context.fillStyle = 'rgba(6,12,18,.78)'
    context.font = '600 10px Inter, sans-serif'
    context.fillText(clip.name ?? 'Audio clip', x + 7, 17, Math.max(0, clipWidth - 14))
    const gainNodes = prepareClipGainNodes(clip)
    const peaks = assets[clip.assetId]?.peaks
    if (peaks && peaks.length) {
      context.strokeStyle = 'rgba(5, 15, 24, .66)'
      context.lineWidth = 1
      context.beginPath()
      const usableHeight = Math.max(10, height - 28)
      const center = 22 + usableHeight / 2
      const columns = Math.max(1, Math.floor(clipWidth - 8))
      const firstColumn = Math.max(0, Math.floor(visibleStart - x - 4))
      const lastColumn = Math.min(columns, Math.ceil(visibleEnd - x - 4))
      const sampleGain = createForwardClipGainSampler(gainNodes)
      for (let column = firstColumn; column < lastColumn; column += 1) {
        const sourceIndex = Math.min(peaks.length / 2 - 1, Math.floor(column / columns * peaks.length / 2)) * 2
        const min = peaks[sourceIndex] ?? 0
        const max = peaks[sourceIndex + 1] ?? 0
        const localSec = (column + .5) / columns * clip.durationSec
        const amplitude = dbToLinearFast(sampleGain(localSec)) * clipFadeAt(clip, localSec)
        context.moveTo(x + 4 + column, center + min * usableHeight * .45 * amplitude)
        context.lineTo(x + 4 + column, center + max * usableHeight * .45 * amplitude)
      }
      context.stroke()
    }
    drawClipEnvelope(context, clip, gainNodes, x, clipWidth, height, pps, isSelected, selectedGainPoint)
    context.fillStyle = '#f7fbff'; context.beginPath(); context.moveTo(x + 2, 4); context.lineTo(x + 10, 4); context.lineTo(x + 2, 12); context.fill()
    context.beginPath(); context.moveTo(x + clipWidth - 2, 4); context.lineTo(x + clipWidth - 10, 4); context.lineTo(x + clipWidth - 2, 12); context.fill()
    context.restore()
  }
  for (const clip of track.midiClips) {
    const x = clip.startSec * pps
    const clipWidth = Math.max(3, clip.durationSec * pps)
    if (x + clipWidth < visibleStart || x > visibleEnd) continue
    context.save(); context.globalAlpha = clip.muted ? .38 : 1
    context.fillStyle = clip.color ?? `${track.color}d8`; context.strokeStyle = '#aeeaff'; context.lineWidth = 1
    context.beginPath(); context.roundRect(x + 1, 4, clipWidth - 2, height - 8, 3); context.fill(); context.stroke()
    context.fillStyle = '#07131acc'; context.font = '600 10px Inter, sans-serif'; context.fillText(`♪ ${clip.name}`, x + 7, 17, Math.max(0, clipWidth - 14))
    const pitches = clip.notes.map((note) => note.pitch); const low = Math.min(36, ...pitches); const high = Math.max(84, ...pitches); const span = Math.max(1, high - low)
    context.fillStyle = '#07131aaa'
    for (const note of clip.notes) { const noteX = x + note.startTicks / 960 * (60 / bpm) * pps; const noteW = Math.max(2, note.lengthTicks / 960 * (60 / bpm) * pps); if (noteX > x + clipWidth) continue; const noteY = height - 8 - (note.pitch - low) / span * (height - 29); context.fillRect(noteX, noteY, Math.min(noteW, x + clipWidth - noteX), 2) }
    context.restore()
  }
}

function gainDbToY(valueDb: number, height: number): number { return 5 + -Math.max(-60, Math.min(0, valueDb)) / 60 * Math.max(1, height - 10) }
function envelopeYAt(clip: Clip, localSec: number, height: number, nodes: readonly ClipGainNode[] = prepareClipGainNodes(clip)): number {
  const gainY = gainDbToY(sampleClipGainDb(nodes, localSec), height)
  const fade = clipFadeAt(clip, localSec)
  const bottom = height - 5
  return bottom - (bottom - gainY) * fade
}

function yToClipGainDb(clip: Clip, localSec: number, y: number, height: number): number {
  const bottom = height - 5
  const fade = Math.max(.05, clipFadeAt(clip, localSec))
  const gainY = bottom - (bottom - Math.max(5, Math.min(bottom, y))) / fade
  return Math.max(-60, Math.min(0, -(gainY - 5) / Math.max(1, height - 10) * 60))
}

function drawClipEnvelope(context: CanvasRenderingContext2D, clip: Clip, nodes: readonly ClipGainNode[], x: number, width: number, height: number, pps: number, clipSelected: boolean, selected: ClipGainPointRef | null): void {
  const showEnvelope = clipSelected || Math.abs(clip.gainDb) >= .01 || Boolean(clip.gainPoints?.length) || clip.fadeInSec > 0 || clip.fadeOutSec > 0
  if (showEnvelope) {
    context.strokeStyle = '#ffffffdd'; context.lineWidth = 1.25; context.beginPath()
    const steps = Math.max(2, Math.min(320, Math.ceil(width / 3)))
    const sampleGain = createForwardClipGainSampler(nodes)
    for (let index = 0; index <= steps; index += 1) { const localSec = clip.durationSec * index / steps; const px = x + localSec * pps; const gainY = gainDbToY(sampleGain(localSec), height); const bottom = height - 5; const py = bottom - (bottom - gainY) * clipFadeAt(clip, localSec); if (index === 0) context.moveTo(px, py); else context.lineTo(px, py) }
    context.stroke()
  }
  const showBaseGain = clipSelected || Math.abs(clip.gainDb) >= .01 || Boolean(clip.gainPoints?.length)
  const centerX = x + width / 2; const centerY = envelopeYAt(clip, clip.durationSec / 2, height, nodes)
  if (showBaseGain) { context.fillStyle = '#fff'; context.strokeStyle = '#17222a'; context.lineWidth = 1.5; context.beginPath(); context.arc(centerX, centerY, 4, 0, Math.PI * 2); context.fill(); context.stroke() }
  for (const localSec of [clip.fadeInSec > 0 ? clip.fadeInSec / 2 : -1, clip.fadeOutSec > 0 ? clip.durationSec - clip.fadeOutSec / 2 : -1]) if (localSec >= 0) { context.fillStyle = '#17222a'; context.strokeStyle = '#ffffffcc'; context.lineWidth = 1; context.beginPath(); context.arc(x + localSec * pps, envelopeYAt(clip, localSec, height, nodes), 4, 0, Math.PI * 2); context.fill(); context.stroke() }
  for (let index = 1; index + 1 < nodes.length; index += 1) { const from = nodes[index]!; const to = nodes[index + 1]!; if (!from.id) continue; const localSec = (from.timeSec + to.timeSec) / 2; context.fillStyle = '#17222a'; context.strokeStyle = '#f2be55'; context.lineWidth = 1.25; context.beginPath(); context.arc(x + localSec * pps, envelopeYAt(clip, localSec, height, nodes), 4, 0, Math.PI * 2); context.fill(); context.stroke() }
  for (const point of clip.gainPoints ?? []) { const selectedPoint = selected?.clipId === clip.id && selected.pointId === point.id; context.fillStyle = selectedPoint ? '#58d9ff' : '#fff'; context.strokeStyle = selectedPoint ? '#e9fbff' : '#17222a'; context.lineWidth = selectedPoint ? 2 : 1.5; context.beginPath(); context.arc(x + point.timeSec * pps, envelopeYAt(clip, point.timeSec, height, nodes), selectedPoint ? 5 : 4, 0, Math.PI * 2); context.fill(); context.stroke() }
}

function hitTestClipEnvelope(track: Track, x: number, y: number, pps: number, height: number, selectedClipIds: readonly string[]): ClipEnvelopeHit | null {
  for (const clip of [...track.clips].reverse()) {
    const left = clip.startSec * pps; const width = clip.durationSec * pps
    if (x < left || x > left + width) continue
    const nodes = prepareClipGainNodes(clip)
    for (const point of [...(clip.gainPoints ?? [])].reverse()) if (Math.hypot(x - (left + point.timeSec * pps), y - envelopeYAt(clip, point.timeSec, height, nodes)) <= 8) return { kind: 'gain-point', clip, pointId: point.id }
    for (let index = 1; index + 1 < nodes.length; index += 1) { const from = nodes[index]!; const to = nodes[index + 1]!; if (!from.id) continue; const local = (from.timeSec + to.timeSec) / 2; if (Math.hypot(x - (left + local * pps), y - envelopeYAt(clip, local, height, nodes)) <= 12) return { kind: 'gain-curve', clip, pointId: from.id } }
    const showBaseGain = selectedClipIds.includes(clip.id) || Math.abs(clip.gainDb) >= .01 || Boolean(clip.gainPoints?.length)
    if (showBaseGain && Math.hypot(x - (left + width / 2), y - envelopeYAt(clip, clip.durationSec / 2, height, nodes)) <= 8) return { kind: 'gain-base', clip }
    if (clip.fadeInSec > 0) { const local = clip.fadeInSec / 2; if (Math.hypot(x - (left + local * pps), y - envelopeYAt(clip, local, height, nodes)) <= 12) return { kind: 'fade-in-curve', clip } }
    if (clip.fadeOutSec > 0) { const local = clip.durationSec - clip.fadeOutSec / 2; if (Math.hypot(x - (left + local * pps), y - envelopeYAt(clip, local, height, nodes)) <= 12) return { kind: 'fade-out-curve', clip } }
  }
  return null
}

function hitTest(clips: readonly Clip[], x: number, pps: number): { clip: Clip; edge: 'left' | 'right' | 'body' } | null {
  for (const clip of [...clips].reverse()) {
    const left = clip.startSec * pps
    const right = (clip.startSec + clip.durationSec) * pps
    if (x < left || x > right) continue
    return { clip, edge: x - left <= 8 ? 'left' : right - x <= 8 ? 'right' : 'body' }
  }
  return null
}

function hitTestTrack(track: Track, x: number, pps: number): { clip: Clip | MidiClip; edge: 'left' | 'right' | 'body'; midi: boolean } | null {
  const midi = hitTestTimed(track.midiClips, x, pps)
  if (midi) return { ...midi, midi: true }
  const audio = hitTest(track.clips, x, pps)
  return audio ? { ...audio, midi: false } : null
}

function hitTestTimed<T extends { startSec: number; durationSec: number }>(clips: readonly T[], x: number, pps: number): { clip: T; edge: 'left' | 'right' | 'body' } | null {
  for (const clip of [...clips].reverse()) {
    const left = clip.startSec * pps
    const right = (clip.startSec + clip.durationSec) * pps
    if (x < left || x > right) continue
    return { clip, edge: x - left <= 8 ? 'left' : right - x <= 8 ? 'right' : 'body' }
  }
  return null
}

function cursorFor(tool: ToolId, edge?: 'left' | 'right' | 'body'): string {
  if (tool === 'arrow' && (edge === 'left' || edge === 'right')) return 'ew-resize'
  return ({ arrow: edge ? 'grab' : 'default', range: 'crosshair', split: 'col-resize', erase: 'not-allowed', paint: 'crosshair', mute: 'pointer', listen: 'ew-resize' } as Record<ToolId, string>)[tool]
}
