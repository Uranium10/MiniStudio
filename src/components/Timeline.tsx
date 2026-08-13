// Canvas-based arrangement view with Studio One-style track lanes and tool gestures.
import { ChevronDown, GripVertical, Headphones, Layers3, MoreHorizontal, Piano, Plus, Radio } from 'lucide-react'
import { memo, useCallback, useEffect, useMemo, useRef, useState, type DragEvent as ReactDragEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { describeEngineError, gridLabel, secondsPerBar, secondsPerBeat, type AutomationLane, type Clip, type MidiClip, type TimeSignature, type Track } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { seekTo } from '../store/commands'
import { automationOptionsForTrack, snapSeconds, useProjectStore } from '../store/projectStore'
import { getEffectiveTool, type ToolId, useToolStore } from '../store/toolStore'
import { FloatingPanel, MenuPanel, type MenuItem } from './Menu'
import { SignalBar } from './controls'
import { buildRulerTicks } from './rulerMath'
import { beginPointerReorder } from './pointerReorder'
import { BROWSER_DRAG_TYPE, readBrowserDrag } from './browserPayload'

const HEADER_WIDTH = 188
/** Keep the playhead this far from the viewport edge before scrolling ahead. */
const FOLLOW_MARGIN = 96

type Gesture = {
  tool: ToolId
  mode: 'move' | 'trim-left' | 'trim-right' | 'fade-in' | 'fade-out' | 'range' | 'paint' | 'listen'
  startX: number
  currentX: number
  clipId?: string
  original?: Clip | MidiClip
  midi?: boolean
}

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
  const scrollRef = useRef<HTMLDivElement>(null)
  const maxEnd = Math.max(60, ...tracks.flatMap((track) => [...track.clips, ...track.midiClips].map((clip) => clip.startSec + clip.durationSec + 4)))
  const timelineWidth = Math.ceil(maxEnd * pixelsPerSecond)

  useFollowPlayhead(scrollRef)
  useTimelineWheel(scrollRef)

  const onBrowserDrop = (event: ReactDragEvent<HTMLElement>) => {
    const payload = readBrowserDrag(event.dataTransfer)
    setBrowserDragOver(false)
    if (!payload) return
    event.preventDefault()
    const store = useProjectStore.getState()
    if (payload.kind === 'instrument') {
      store.addInstrumentTrack(payload.plugin)
      store.showToast(`${payload.plugin?.name ?? 'DefaultSynth'} 트랙을 추가했습니다.`)
      return
    }
    if (payload.kind === 'media') {
      void engine.loadAudioFile(payload.path).then((asset) => store.addAssetAsTrack(asset)).catch((error) => store.showToast(`${payload.name}을 불러오지 못했습니다: ${String(error)}`))
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
      onDragOver={(event) => { if (event.dataTransfer.types.includes(BROWSER_DRAG_TYPE)) { event.preventDefault(); event.dataTransfer.dropEffect = 'copy' } }}
      onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setBrowserDragOver(false) }}
      onDrop={onBrowserDrop}
    >
      <div className="arrangement-topline"><span>ARRANGEMENT</span><div><span className="legend-grid" />그리드 {gridLabel(gridTicks)}{snapEnabled ? '' : ' (스냅 꺼짐)'} <span className="legend-loop" />루프 영역 · Ctrl+휠 줌</div></div>
      <div className="timeline-scroll" ref={scrollRef}>
        <div className="timeline-content" style={{ width: HEADER_WIDTH + timelineWidth }}>
          <div className="ruler-row">
            <div className="track-list-heading"><span>트랙</span><button onClick={addTrack} title="오디오 트랙 추가"><Plus size={14} /></button><button onClick={() => addInstrumentTrack()} title="인스트루먼트 트랙 추가"><Piano size={13} /></button></div>
            <Ruler width={timelineWidth} pixelsPerSecond={pixelsPerSecond} />
          </div>
          <LoopStrip width={timelineWidth} pixelsPerSecond={pixelsPerSecond} />
          {tracks.map((track, index) => <div className="track-stack" data-track-id={track.id} key={track.id}>
            <div className="track-row" style={{ height: trackHeight }}>
              <TrackHeader track={track} index={index} />
              <TrackLane track={track} width={timelineWidth} height={trackHeight} scrollRef={scrollRef} />
            </div>
            {track.automationOpen && <AutomationSection track={track} width={timelineWidth} pixelsPerSecond={pixelsPerSecond} />}
          </div>)}
          <Playhead pixelsPerSecond={pixelsPerSecond} />
          <button className="add-track-row" onClick={addTrack}><Plus size={14} /> 오디오 트랙 추가 · 상단 건반 버튼으로 인스트루먼트 추가</button>
        </div>
      </div>
    </section>
  )
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
        store.setTrackHeight(store.trackHeight + (event.deltaY < 0 ? 6 : -6))
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
  const snap = (sec: number) => snapEnabled ? Math.round(sec / snapSeconds(gridTicks, bpm)) * snapSeconds(gridTicks, bpm) : sec

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
      if (mode === 'start') setLoopRange(snap(sec), endSec)
      else if (mode === 'end') setLoopRange(startSec, snap(sec))
      else if (mode === 'draw') setLoopRange(snap(originSec), snap(sec))
      else {
        const delta = sec - originSec
        const nextStart = Math.max(0, snap(startSec + delta))
        const appliedDelta = nextStart - startSec
        setLoopRange(nextStart, snap(endSec + appliedDelta))
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
  const ticks = useMemo(() => buildRulerTicks(width, pixelsPerSecond, bpm, signature), [width, pixelsPerSecond, bpm, signature])
  return (
    <div className="ruler" style={{ width }} onPointerDown={(event) => seekTo(engine, event.nativeEvent.offsetX / pixelsPerSecond)}>
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
  const engine = useEngine()

  const menuItems = (): MenuItem[] => {
    const store = useProjectStore.getState()
    const run = (label: string, action: () => void, extra?: Partial<Extract<MenuItem, { kind: 'item' }>>): MenuItem => ({ kind: 'item', label, run: action, ...extra })
    return [
      run('이펙트 체인 열기', () => store.setRackTarget({ kind: 'track', id: track.id })),
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
        <input value={track.name} onChange={(event) => updateTrack(track.id, { name: event.target.value })} onClick={(event) => event.stopPropagation()} />
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
    {lanes.map((lane, index) => <div className="automation-row" key={lane.id} style={{ height: AUTOMATION_HEIGHT }}>
      <div className="automation-lane-header">
        <span className="automation-color" style={{ background: track.color }} />
        <div><small>{lane.category}</small><strong>{lane.label}</strong></div>
        {index === 0 && <button ref={addButton} className="automation-add" title="오토메이션 파라미터 추가" onClick={() => setPickerOpen((open) => !open)}><Plus size={11} /></button>}
        <button className="automation-remove" title="오토메이션 레인 제거" onClick={() => removeLane(track.id, lane.id)}><span>×</span></button>
      </div>
      <AutomationCurve trackId={track.id} lane={lane} width={width} pixelsPerSecond={pixelsPerSecond} />
    </div>)}
    {pickerOpen && <FloatingPanel getAnchorElement={getAnchor} onClose={() => setPickerOpen(false)} className="automation-picker">
      <header><strong>AUTOMATION</strong><span>{track.name}</span></header>
      <input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="파라미터 검색" />
      <div>{[...categories].map(([category, items]) => <section key={category}><strong>{category}</strong>{items.map((option) => <button key={`${option.targetKind}:${option.targetId}:${option.parameterId}`} onClick={() => { addLane(track.id, option); setPickerOpen(false); setQuery('') }}><span>{option.label}</span><small>{option.parameterId}</small></button>)}</section>)}{!options.length && <small className="automation-empty">추가할 파라미터가 없습니다.</small>}</div>
    </FloatingPanel>}
  </div>
}

function AutomationCurve({ trackId, lane, width, pixelsPerSecond }: { trackId: string; lane: AutomationLane; width: number; pixelsPerSecond: number }) {
  const upsert = useProjectStore((state) => state.upsertAutomationPoint)
  const remove = useProjectStore((state) => state.removeAutomationPoint)
  const svgRef = useRef<SVGSVGElement>(null)
  const dragging = useRef<{ id: string; pointerId: number } | null>(null)
  const valueY = useCallback((value: number) => 5 + (1 - (value - lane.min) / Math.max(0.0001, lane.max - lane.min)) * (AUTOMATION_HEIGHT - 10), [lane.max, lane.min])
  const pointAt = useCallback((clientX: number, clientY: number) => {
    const bounds = svgRef.current?.getBoundingClientRect()
    if (!bounds) return { timeSec: 0, value: lane.defaultValue }
    const x = Math.max(0, Math.min(bounds.width, clientX - bounds.left))
    const y = Math.max(5, Math.min(AUTOMATION_HEIGHT - 5, clientY - bounds.top))
    const rawTime = x / pixelsPerSecond
    const state = useProjectStore.getState()
    const step = state.snapEnabled ? snapSeconds(state.gridTicks, state.project.transport.bpm) : 0
    const timeSec = step ? Math.round(rawTime / step) * step : rawTime
    const value = lane.max - (y - 5) / (AUTOMATION_HEIGHT - 10) * (lane.max - lane.min)
    return { timeSec, value }
  }, [lane.defaultValue, lane.max, lane.min, pixelsPerSecond])
  const sorted = [...lane.points].sort((left, right) => left.timeSec - right.timeSec)
  const path = sorted.length
    ? `M0 ${valueY(sorted[0]!.value)} ${sorted.map((point) => `L${point.timeSec * pixelsPerSecond} ${valueY(point.value)}`).join(' ')} L${width} ${valueY(sorted.at(-1)!.value)}`
    : `M0 ${valueY(lane.defaultValue)} L${width} ${valueY(lane.defaultValue)}`
  const move = (event: ReactPointerEvent<SVGSVGElement>) => {
    const drag = dragging.current
    if (!drag || drag.pointerId !== event.pointerId) return
    upsert(trackId, lane.id, { id: drag.id, ...pointAt(event.clientX, event.clientY) })
  }
  const finish = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (dragging.current?.pointerId === event.pointerId) dragging.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  return <svg ref={svgRef} className="automation-curve" width={width} height={AUTOMATION_HEIGHT} onDoubleClick={(event) => { if ((event.target as Element).closest('circle')) return; upsert(trackId, lane.id, pointAt(event.clientX, event.clientY)) }} onPointerMove={move} onPointerUp={finish} onPointerCancel={finish}>
    <path d={path} />
    {sorted.map((point) => <circle key={point.id} cx={point.timeSec * pixelsPerSecond} cy={valueY(point.value)} r="4" onPointerDown={(event) => { event.stopPropagation(); dragging.current = { id: point.id, pointerId: event.pointerId }; event.currentTarget.ownerSVGElement?.setPointerCapture(event.pointerId) }} onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); remove(trackId, lane.id, point.id) }}><title>{`${point.value.toFixed(2)} · ${point.timeSec.toFixed(2)} s`}</title></circle>)}
  </svg>
}

function TrackLaneView({ track, width, height, scrollRef }: { track: Track; width: number; height: number; scrollRef: React.RefObject<HTMLDivElement> }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const gestureRef = useRef<Gesture | null>(null)
  const listenPlayRef = useRef<Promise<void> | null>(null)
  const dropLaneRef = useRef<HTMLElement | null>(null)
  const [overlay, setOverlay] = useState<{ start: number; end: number; kind: 'range' | 'paint' } | null>(null)
  const [cursor, setCursor] = useState('default')
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; sec: number; hit: ReturnType<typeof hitTestTrack> } | null>(null)
  const assets = useProjectStore((state) => state.project.assets)
  const selectedClipIds = useProjectStore((state) => state.selectedClipIds)
  const pixelsPerSecond = useProjectStore((state) => state.pixelsPerSecond)
  const snapEnabled = useProjectStore((state) => state.snapEnabled)
  const gridTicks = useProjectStore((state) => state.gridTicks)
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const signature = useProjectStore((state) => state.project.transport.timeSignature)
  const engine = useEngine()

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    let pendingFrame = 0
    const draw = () => drawTrackLane(canvas, track, assets, selectedClipIds, pixelsPerSecond, bpm, signature, gridTicks, scrollRef.current)
    const requestDraw = () => {
      if (pendingFrame) return
      pendingFrame = requestAnimationFrame(() => { pendingFrame = 0; draw() })
    }
    draw()
    const scroll = scrollRef.current
    scroll?.addEventListener('scroll', requestDraw, { passive: true })
    return () => {
      scroll?.removeEventListener('scroll', requestDraw)
      if (pendingFrame) cancelAnimationFrame(pendingFrame)
    }
  }, [track, assets, selectedClipIds, pixelsPerSecond, bpm, signature, gridTicks, width, height, scrollRef])

  const xToSec = (x: number) => x / pixelsPerSecond
  const snap = (sec: number) => {
    if (!snapEnabled) return sec
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
    const tool = useToolStore.getState().latchGesture()
    event.currentTarget.setPointerCapture(event.pointerId)
    store.selectTrack(track.id)

    if (tool === 'split') {
      if (hit) store.splitClip(track.id, hit.clip.id, snap(sec))
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

    let targetClip = hit.clip
    if (event.altKey) {
      const duplicateId = store.duplicateClip(track.id, hit.clip.id)
      if (duplicateId) targetClip = { ...hit.clip, id: duplicateId }
    } else store.selectClip(hit.clip.id, event.ctrlKey || event.metaKey)
    const relativeY = event.clientY - event.currentTarget.getBoundingClientRect().top
    let mode: Gesture['mode'] = hit.edge === 'left' ? 'trim-left' : hit.edge === 'right' ? 'trim-right' : 'move'
    if (!hit.midi && relativeY <= 14 && hit.edge === 'left') mode = 'fade-in'
    if (!hit.midi && relativeY <= 14 && hit.edge === 'right') mode = 'fade-out'
    gestureRef.current = { tool, mode, startX: x, currentX: x, clipId: targetClip.id, original: { ...targetClip }, midi: hit.midi }
  }

  const onPointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const store = useProjectStore.getState()
    const x = pointX(event)
    const gesture = gestureRef.current
    if (!gesture) {
      const tool = getEffectiveTool(useToolStore.getState())
      const hit = hitTestTrack(track, x, pixelsPerSecond)
      setCursor(cursorFor(tool, hit?.edge))
      return
    }
    gesture.currentX = x
    if (gesture.mode === 'range' || gesture.mode === 'paint') { setOverlay({ start: gesture.startX, end: x, kind: gesture.mode }); return }
    if (!gesture.original || !gesture.clipId) return
    const delta = (x - gesture.startX) / pixelsPerSecond
    const original = gesture.original
    if (gesture.mode === 'move') {
      store.updateClip(track.id, gesture.clipId, { startSec: Math.max(0, snap(original.startSec + delta)) })
      const lane = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('.track-lane') ?? null
      const target = store.project.tracks.find((candidate) => candidate.id === lane?.dataset.trackId)
      const valid = Boolean(target && (gesture.midi ? target.kind === 'instrument' : target.kind === 'audio'))
      setDropLane(lane, valid)
    }
    if (gesture.mode === 'trim-left') {
      const nextStart = Math.min(original.startSec + original.durationSec - 0.1, Math.max(0, snap(original.startSec + delta)))
      const consumed = nextStart - original.startSec
      store.updateClip(track.id, gesture.clipId, { startSec: nextStart, ...(!gesture.midi && 'offsetSec' in original ? { offsetSec: Math.max(0, original.offsetSec + consumed) } : {}), durationSec: original.durationSec - consumed })
    }
    if (gesture.mode === 'trim-right') store.updateClip(track.id, gesture.clipId, { durationSec: Math.max(0.1, snap(original.durationSec + delta)) })
    if (gesture.mode === 'fade-in' && !gesture.midi) store.updateClip(track.id, gesture.clipId, { fadeInSec: Math.max(0, Math.min(original.durationSec, xToSec(x) - original.startSec)) })
    if (gesture.mode === 'fade-out' && !gesture.midi) store.updateClip(track.id, gesture.clipId, { fadeOutSec: Math.max(0, Math.min(original.durationSec, original.startSec + original.durationSec - xToSec(x))) })
  }

  const onPointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const store = useProjectStore.getState()
    const gesture = gestureRef.current
    if (gesture?.mode === 'paint') {
      const start = snap(xToSec(Math.min(gesture.startX, gesture.currentX)))
      const end = snap(xToSec(Math.max(gesture.startX, gesture.currentX)))
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

  const contextItems: MenuItem[] = contextMenu?.hit ? [
    { kind: 'item', label: contextMenu.hit.midi ? '피아노롤에서 열기' : '클립 선택', run: () => { const store = useProjectStore.getState(); store.selectClip(contextMenu.hit!.clip.id); if (contextMenu.hit!.midi) store.openMidiEditor(track.id, contextMenu.hit!.clip.id) } },
    { kind: 'item', label: '플레이헤드에서 분할', keys: 'S', run: () => useProjectStore.getState().splitClip(track.id, contextMenu.hit!.clip.id, contextMenu.sec) },
    { kind: 'item', label: '복제', keys: 'Ctrl+D', run: () => useProjectStore.getState().duplicateClip(track.id, contextMenu.hit!.clip.id) },
    { kind: 'item', label: contextMenu.hit.clip.muted ? '뮤트 해제' : '뮤트', keys: 'M', checked: Boolean(contextMenu.hit.clip.muted), run: () => useProjectStore.getState().updateClip(track.id, contextMenu.hit!.clip.id, { muted: !contextMenu.hit!.clip.muted }) },
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
      <canvas ref={canvasRef} width={width} height={height} style={{ width, height, cursor }} onDoubleClick={(event) => { const x = event.clientX - event.currentTarget.getBoundingClientRect().left; const midi = [...track.midiClips].reverse().find((clip) => x >= clip.startSec * pixelsPerSecond && x <= (clip.startSec + clip.durationSec) * pixelsPerSecond); if (midi) useProjectStore.getState().openMidiEditor(track.id, midi.id); else if (hitTest(track.clips, x, pixelsPerSecond)) useProjectStore.getState().showToast('이 오디오 클립은 아직 편집할 수 없습니다.') }} onContextMenu={(event) => { event.preventDefault(); const store = useProjectStore.getState(); if (!store.selectedTrackIds.includes(track.id)) store.selectTrack(track.id); const x = event.clientX - event.currentTarget.getBoundingClientRect().left; const sec = Math.max(0, snap(xToSec(x))); const hit = hitTestTrack(track, x, pixelsPerSecond); if (hit) store.selectClip(hit.clip.id); setContextMenu({ x: event.clientX, y: event.clientY, sec, hit }) }} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp} />
      {overlay && <div className={`gesture-overlay ${overlay.kind}`} style={{ left: Math.min(overlay.start, overlay.end), width: Math.abs(overlay.end - overlay.start) }} />}
      {track.armed && <span className="input-monitor"><Headphones size={10} /> IN</span>}
      {contextMenu && <MenuPanel items={contextItems} anchor={{ x: contextMenu.x, y: contextMenu.y }} onClose={() => setContextMenu(null)} />}
    </div>
  )
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

function drawTrackLane(canvas: HTMLCanvasElement, track: Track, assets: Record<string, { durationSec: number; peaks: Float32Array }>, selected: string[], pps: number, bpm: number, signature: TimeSignature, gridTicks: number, scroll: HTMLDivElement | null): void {
  const ratio = Math.min(window.devicePixelRatio || 1, 1.5)
  const width = canvas.clientWidth
  const height = canvas.clientHeight
  if (canvas.width !== width * ratio || canvas.height !== height * ratio) { canvas.width = width * ratio; canvas.height = height * ratio }
  const context = canvas.getContext('2d')
  if (!context) return
  context.setTransform(ratio, 0, 0, ratio, 0, 0)
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
    const peaks = assets[clip.assetId]?.peaks
    if (peaks && peaks.length) {
      context.strokeStyle = 'rgba(5, 15, 24, .66)'
      context.lineWidth = 1
      context.beginPath()
      const usableHeight = Math.max(10, height - 28)
      const center = 22 + usableHeight / 2
      const columns = Math.max(1, Math.floor(clipWidth - 8))
      for (let column = 0; column < columns; column += 1) {
        const sourceIndex = Math.min(peaks.length / 2 - 1, Math.floor(column / columns * peaks.length / 2)) * 2
        const min = peaks[sourceIndex] ?? 0
        const max = peaks[sourceIndex + 1] ?? 0
        context.moveTo(x + 4 + column, center + min * usableHeight * 0.45)
        context.lineTo(x + 4 + column, center + max * usableHeight * 0.45)
      }
      context.stroke()
    }
    if (clip.fadeInSec > 0) { context.strokeStyle = '#fff9'; context.beginPath(); context.moveTo(x + 2, height - 5); context.lineTo(x + clip.fadeInSec * pps, 5); context.stroke() }
    if (clip.fadeOutSec > 0) { context.strokeStyle = '#fff9'; context.beginPath(); context.moveTo(x + clipWidth - clip.fadeOutSec * pps, 5); context.lineTo(x + clipWidth - 2, height - 5); context.stroke() }
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
