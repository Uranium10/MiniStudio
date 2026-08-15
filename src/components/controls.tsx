// Shared DAW knobs and level meters with fine-drag and reset behavior.
/* oxlint-disable react/only-export-components */
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { useEngine } from '../hooks/useEngine'
import { MenuPanel, type MenuItem } from './Menu'

// Pointer capture keeps a knob gesture alive outside the element and outside the
// window. preventDefault on pointerdown stops any ancestor from starting a native
// HTML5 drag, which would otherwise fire pointercancel and kill the gesture.
type KnobDrag = { pointerId: number; anchorY: number; anchorValue: number; fine: boolean; moved: boolean }
type KnobScale = 'linear' | 'log'

type ParameterAutomationContextValue = {
  add(parameterId: string | undefined, label: string, value: number): void
}

const ParameterAutomationContext = createContext<ParameterAutomationContextValue | null>(null)

export function ParameterAutomationProvider({ add, children }: { add: ParameterAutomationContextValue['add']; children: ReactNode }) {
  return <ParameterAutomationContext.Provider value={{ add }}>{children}</ParameterAutomationContext.Provider>
}

export function Knob({ value, min, max, step, scale = 'linear', label, format = (current) => current.toFixed(1), defaultValue, onChange, parameterId }: { value: number; min: number; max: number; step: number; scale?: KnobScale; label: string; format?: (value: number) => string; defaultValue: number; onChange(value: number): void; parameterId?: string }) {
  const dragRef = useRef<KnobDrag | null>(null)
  const elementRef = useRef<HTMLButtonElement>(null)
  const automation = useContext(ParameterAutomationContext)
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const normalized = controlNormalized(value, min, max, scale)
  const angle = -135 + normalized * 270

  const endDrag = () => {
    const drag = dragRef.current
    dragRef.current = null
    if (!drag) return
    const element = elementRef.current
    if (element?.hasPointerCapture(drag.pointerId)) element.releasePointerCapture(drag.pointerId)
    document.body.classList.remove('param-dragging')
  }
  useEffect(() => endDrag, [])

  const onPointerDown = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    event.stopPropagation()
    const element = event.currentTarget
    element.focus({ preventScroll: true })
    element.setPointerCapture(event.pointerId)
    dragRef.current = { pointerId: event.pointerId, anchorY: event.clientY, anchorValue: value, fine: event.shiftKey, moved: false }
    document.body.classList.add('param-dragging')
  }

  const onPointerMove = (event: React.PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    event.preventDefault()
    // Re-anchor when Shift is toggled mid-drag so fine mode adjusts from here on
    // instead of rescaling the whole travel and jumping the value.
    if (event.shiftKey !== drag.fine) {
      drag.fine = event.shiftKey
      drag.anchorY = event.clientY
      drag.anchorValue = value
    }
    drag.moved = true
    const travel = (drag.anchorY - event.clientY) / 90 * (drag.fine ? 0.15 : 1)
    const next = controlValue(controlNormalized(drag.anchorValue, min, max, scale) + travel, min, max, scale)
    onChange(clampStep(next, min, max, step))
  }

  const onKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    const coarse = event.shiftKey ? 0.1 : 1
    const pageValue = (direction: number) => controlValue(controlNormalized(value, min, max, scale) + direction / 20, min, max, scale) - value
    const delta = event.key === 'ArrowUp' || event.key === 'ArrowRight' ? step * coarse
      : event.key === 'ArrowDown' || event.key === 'ArrowLeft' ? -step * coarse
        : event.key === 'PageUp' ? pageValue(1) : event.key === 'PageDown' ? pageValue(-1) : 0
    if (delta) { event.preventDefault(); onChange(clampStep(value + delta, min, max, step)); return }
    if (event.key === 'Home' || event.key === 'Backspace') { event.preventDefault(); onChange(defaultValue) }
  }

  return (
    <div className="knob-control">
      <button
        ref={elementRef}
        className="knob"
        role="slider"
        aria-label={label}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={value}
        aria-valuetext={format(value)}
        style={{ '--knob-angle': `${angle}deg`, '--knob-fill': `${normalized * 75}%` } as React.CSSProperties}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
        onDragStart={(event) => event.preventDefault()}
        onKeyDown={onKeyDown}
        onDoubleClick={() => onChange(defaultValue)}
        onContextMenu={automation ? (event) => { event.preventDefault(); event.stopPropagation(); setMenu({ x: event.clientX, y: event.clientY }) } : undefined}
        title="세로 드래그 · Shift 미세 조절 · 더블클릭/Home 리셋 · 방향키 조절"
      ><i /></button>
      <EditableNumber value={value} min={min} max={max} step={step} onChange={onChange} format={format} ariaLabel={`${label} 값`} />
      <span>{label}</span>
      {menu && <MenuPanel items={[{ kind: 'item', label: `${label} 오토메이션 추가`, run: () => automation?.add(parameterId, label, value) } satisfies MenuItem]} anchor={menu} onClose={() => setMenu(null)} className="parameter-context-menu" />}
    </div>
  )
}

export function EditableNumber({ value, min, max, step, onChange, format = String, className = '', ariaLabel = '수치', children }: { value: number; min: number; max: number; step: number; onChange(value: number): void; format?: (value: number) => string; className?: string; ariaLabel?: string; children?: ReactNode }) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(String(value))
  const commit = () => {
    const parsed = Number(draft)
    if (Number.isFinite(parsed)) onChange(clampStep(parsed, min, max, step))
    setEditing(false)
  }
  if (editing) return <input className={`editable-number editing ${className}`} aria-label={ariaLabel} autoFocus type="number" min={min} max={max} step={step} value={draft} onChange={(event) => setDraft(event.target.value)} onBlur={commit} onKeyDown={(event) => { if (event.key === 'Enter') commit(); if (event.key === 'Escape') setEditing(false) }} onClick={(event) => event.stopPropagation()} />
  return <output className={`editable-number ${className}`} title="더블클릭하여 직접 입력" onDoubleClick={(event) => { event.stopPropagation(); setDraft(String(value)); setEditing(true) }}>{children ?? format(value)}</output>
}

function clampStep(value: number, min: number, max: number, step: number): number {
  const clipped = Math.max(min, Math.min(max, value))
  return Number((Math.round((clipped - min) / step) * step + min).toFixed(8))
}

function controlNormalized(value: number, min: number, max: number, scale: KnobScale): number {
  const clipped = Math.max(min, Math.min(max, value))
  if (scale === 'log' && min > 0 && max > min) return Math.log(clipped / min) / Math.log(max / min)
  return (clipped - min) / Math.max(Number.EPSILON, max - min)
}

function controlValue(normalized: number, min: number, max: number, scale: KnobScale): number {
  const ratio = Math.max(0, Math.min(1, normalized))
  if (scale === 'log' && min > 0 && max > min) return min * (max / min) ** ratio
  return min + (max - min) * ratio
}

export function LevelMeter({ trackId }: { trackId?: string }) {
  const engine = useEngine()
  const rootRef = useRef<HTMLButtonElement>(null)
  const rmsRef = useRef<HTMLElement>(null)
  const peakRef = useRef<HTMLElement>(null)
  const holdRef = useRef<HTMLElement>(null)
  const heldPeak = useRef(0)
  const holdUntil = useRef(0)

  useEffect(() => {
    return subscribeMeterFrame((time) => {
      const next = trackId ? engine.getTrackLevel(trackId) : engine.getMasterLevel()
      if (next.peak >= heldPeak.current || time > holdUntil.current) {
        heldPeak.current = next.peak
        holdUntil.current = time + 1500
      }
      rmsRef.current?.style.setProperty('height', `${Math.min(100, next.rms * 125)}%`)
      peakRef.current?.style.setProperty('height', `${Math.min(100, next.peak * 100)}%`)
      holdRef.current?.style.setProperty('bottom', `${Math.min(100, heldPeak.current * 100)}%`)
      if (next.peak >= 0.999) rootRef.current?.classList.add('clipped')
    })
  }, [engine, trackId])

  return (
    <button ref={rootRef} className="level-meter" onClick={() => rootRef.current?.classList.remove('clipped')} title="Peak + RMS · 클릭하여 클립 표시 해제">
      <i ref={rmsRef} className="meter-rms" />
      <i ref={peakRef} className="meter-peak" />
      <i ref={holdRef} className="meter-hold" />
    </button>
  )
}

/** Compact live activity display for arrangement track headers. */
export function SignalBar({ trackId, kind }: { trackId: string; kind: 'audio' | 'instrument' }) {
  const engine = useEngine()
  const rootRef = useRef<HTMLSpanElement>(null)
  const fillRef = useRef<HTMLElement>(null)
  const displayed = useRef(0)
  const lastFrame = useRef(0)

  useEffect(() => subscribeMeterFrame((time) => {
    const elapsed = lastFrame.current ? Math.min(100, time - lastFrame.current) : 1000 / 30
    lastFrame.current = time
    if (kind === 'instrument') {
      const liveGate = engine.getMidiGateCount(trackId)
      const active = liveGate === null ? engine.getActiveVoiceCount(trackId) > 0 : liveGate > 0
      displayed.current = active ? 1 : displayed.current * Math.exp(-elapsed / 420)
    } else {
      const level = engine.getTrackLevel(trackId)
      const target = Math.min(1, Math.max(level.peak, level.rms * 1.25))
      displayed.current = target >= displayed.current ? target : Math.max(target, displayed.current * Math.exp(-elapsed / 190))
    }
    const amount = displayed.current < .003 ? 0 : displayed.current
    fillRef.current?.style.setProperty('transform', `scaleY(${amount})`)
    rootRef.current?.classList.toggle('signal-active', amount > .01)
  }), [engine, kind, trackId])

  return <span ref={rootRef} className={`track-signal ${kind}`} title={kind === 'instrument' ? 'MIDI activity · note-off decay' : 'Audio peak / RMS level'}><i ref={fillRef} /></span>
}

type MeterFrameTask = (time: number) => void
const meterFrameTasks = new Set<MeterFrameTask>()
const analyzerFrameTasks = new Set<MeterFrameTask>()
let meterAnimationFrame = 0
let lastMeterFrame = 0
let lastAnalyzerFrame = 0

function runMeterFrames(time: number): void {
  if (time - lastMeterFrame >= 1000 / 30) {
    for (const task of meterFrameTasks) task(time)
    lastMeterFrame = time
  }
  // Analyzer canvases are imperative: a faster visual refresh never enters
  // React and therefore cannot invalidate the rack, cards, knobs, or layout.
  if (time - lastAnalyzerFrame >= 1000 / 60) {
    for (const task of analyzerFrameTasks) task(time)
    lastAnalyzerFrame = time
  }
  if (meterFrameTasks.size || analyzerFrameTasks.size) meterAnimationFrame = requestAnimationFrame(runMeterFrames)
  else meterAnimationFrame = 0
}

export function subscribeMeterFrame(task: MeterFrameTask): () => void {
  meterFrameTasks.add(task)
  if (!meterAnimationFrame) meterAnimationFrame = requestAnimationFrame(runMeterFrames)
  return () => {
    meterFrameTasks.delete(task)
    if (!meterFrameTasks.size && !analyzerFrameTasks.size && meterAnimationFrame) {
      cancelAnimationFrame(meterAnimationFrame)
      meterAnimationFrame = 0
    }
  }
}

export function subscribeAnalyzerFrame(task: MeterFrameTask): () => void {
  analyzerFrameTasks.add(task)
  if (!meterAnimationFrame) meterAnimationFrame = requestAnimationFrame(runMeterFrames)
  return () => {
    analyzerFrameTasks.delete(task)
    if (!meterFrameTasks.size && !analyzerFrameTasks.size && meterAnimationFrame) {
      cancelAnimationFrame(meterAnimationFrame)
      meterAnimationFrame = 0
    }
  }
}
