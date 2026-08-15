// Fixed transport controls driven by the audio-engine clock.
import { Activity, ChevronUp, Circle, Gauge, ListMusic, Pause, Play, Repeat2, SkipBack, Square, Waves, X } from 'lucide-react'
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { useEngine } from '../hooks/useEngine'
import { toBarsBeats, type StreamStatus } from '../engine'
import { seekTo, stopPlayback, togglePlayback } from '../store/commands'
import { useProjectStore } from '../store/projectStore'
import { EditableNumber } from './controls'
import { FloatingPanel } from './Menu'

export function TransportBar() {
  const playheadSec = useProjectStore((state) => state.playheadSec)
  const isPlaying = useProjectStore((state) => state.project.transport.isPlaying)
  const bpm = useProjectStore((state) => state.project.transport.bpm)
  const signature = useProjectStore((state) => state.project.transport.timeSignature)
  const loopEnabled = useProjectStore((state) => state.project.transport.loop.enabled)
  const sampleRate = useProjectStore((state) => state.project.meta.sampleRate)
  const setBpm = useProjectStore((state) => state.setBpm)
  const setTimeSignature = useProjectStore((state) => state.setTimeSignature)
  const recordingEnabled = useProjectStore((state) => state.recordingEnabled)
  const overdubMode = useProjectStore((state) => state.overdubMode)
  const setRecordingEnabled = useProjectStore((state) => state.setRecordingEnabled)
  const toggleLoop = useProjectStore((state) => state.toggleLoop)
  const togglePanel = useProjectStore((state) => state.toggleLowerPanel)
  const engine = useEngine()

  return (
    <footer className="transport-bar">
      <button className="panel-up" onClick={togglePanel} title="하단 패널 토글 (Tab)"><ChevronUp size={14} /></button>
      <div className="transport-readout"><small>POSITION</small><EditableNumber className="transport-position" value={playheadSec} min={0} max={86_400} step={0.001} onChange={(sec) => seekTo(engine, sec)} format={formatTime} /><span>{formatBars(playheadSec, bpm, signature)}</span></div>
      <div className="transport-controls">
        <button title="처음으로 (Enter)" onClick={() => seekTo(engine, 0)}><SkipBack size={16} /></button>
        <button title="정지" onClick={() => stopPlayback(engine)}><Square size={15} fill="currentColor" /></button>
        <button className={`play-button ${isPlaying ? 'playing' : ''}`} title="재생 / 일시정지 (Space)" onClick={() => void togglePlayback(engine)}>{isPlaying ? <Pause size={18} fill="currentColor" /> : <Play size={19} fill="currentColor" />}</button>
        <button className={`record-button ${recordingEnabled ? 'recording' : ''}`} title="MIDI 녹음 대기 · 암드 인스트루먼트 트랙에 연주를 기록합니다" onClick={() => setRecordingEnabled(!recordingEnabled)}><Circle size={17} fill="currentColor" /></button>
        <button className={loopEnabled ? 'loop-active' : ''} title="루프 (L) · 룰러 아래 루프 바를 끌어 구간을 조정합니다" onClick={toggleLoop}><Repeat2 size={17} /></button>
      </div>
      <div className="transport-settings">
        <div className="tempo-setting"><Gauge size={14} /><span><small>TEMPO</small><TempoControl value={bpm} onChange={setBpm} /></span></div>
        <SignatureControl value={signature} onChange={setTimeSignature} />
        <div><Waves size={14} /><span><small>SAMPLE RATE</small><strong>{sampleRate / 1000} kHz</strong></span></div>
        {recordingEnabled && <label><span><small>OVERDUB</small><select value={overdubMode} onChange={(event) => useProjectStore.setState({ overdubMode: event.target.value as 'merge' | 'new' })}><option value="merge">Merge</option><option value="new">New clip</option></select></span></label>}
      </div>
      <StreamReadout />
    </footer>
  )
}

function SignatureControl({ value, onChange }: { value: { numerator: number; denominator: number }; onChange(value: { numerator: number; denominator: number }): void }) {
  const [open, setOpen] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null)
  const anchor = () => trigger.current
  const preset = (numerator: number) => onChange({ numerator, denominator: 4 })
  return <div className="signature-setting"><ListMusic size={14} /><button ref={trigger} className="signature-compact" title="박자표 설정" onClick={() => setOpen((current) => !current)}><small>SIGNATURE</small><strong>{value.numerator}/{value.denominator}</strong></button><div className="signature-quick"><button className={value.numerator === 3 && value.denominator === 4 ? 'active' : ''} onClick={() => preset(3)}>3/4</button><button className={value.numerator === 4 && value.denominator === 4 ? 'active' : ''} onClick={() => preset(4)}>4/4</button></div>
    {open && <FloatingPanel getAnchorElement={anchor} onClose={() => setOpen(false)} className="signature-popup"><header><div><small>TIME SIGNATURE</small><strong>박자표 설정</strong></div><button title="닫기" onClick={() => setOpen(false)}><X size={14} /></button></header><div className="signature-large"><label><span>박자 수</span><input autoFocus type="number" min="1" max="32" value={value.numerator} onChange={(event) => onChange({ ...value, numerator: Number(event.target.value) })} /></label><i>/</i><label><span>음표 단위</span><select value={value.denominator} onChange={(event) => onChange({ ...value, denominator: Number(event.target.value) })}>{[1, 2, 4, 8, 16].map((item) => <option key={item} value={item}>{item}</option>)}</select></label></div><div className="signature-beat-length"><span>비트 길이</span><strong>1/{value.denominator}</strong><i>•</i></div><footer><button onClick={() => preset(3)}>3/4 WALTZ</button><button onClick={() => preset(4)}>4/4 COMMON</button></footer></FloatingPanel>}
  </div>
}

function TempoControl({ value, onChange }: { value: number; onChange(value: number): void }) {
  const [editingPart, setEditingPart] = useState<'integer' | 'fraction' | null>(null)
  const [draft, setDraft] = useState('')
  const drag = useRef<{ pointerId: number; y: number; value: number; fine: boolean } | null>(null)
  const hundredths = Math.round(value * 100)
  const integer = Math.floor(hundredths / 100)
  const fraction = String(hundredths % 100).padStart(2, '0')
  const commit = (part: 'integer' | 'fraction') => {
    const parsed = Number(draft)
    if (Number.isFinite(parsed)) onChange(part === 'integer' ? Math.round(parsed) + Number(fraction) / 100 : integer + Math.max(0, Math.min(99, Math.round(parsed))) / 100)
    setEditingPart(null)
  }
  const beginEdit = (part: 'integer' | 'fraction') => { setDraft(part === 'integer' ? String(integer) : fraction); setEditingPart(part) }
  const begin = (event: ReactPointerEvent<HTMLButtonElement>, part: 'integer' | 'fraction') => {
    if (event.button !== 0) return
    event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId)
    drag.current = { pointerId: event.pointerId, y: event.clientY, value, fine: part === 'fraction' || event.shiftKey }
    const move = (pointer: PointerEvent) => {
      const current = drag.current
      if (!current || current.pointerId !== pointer.pointerId) return
      const fine = part === 'fraction' || pointer.shiftKey
      if (fine !== current.fine) { current.fine = fine; current.y = pointer.clientY; current.value = useProjectStore.getState().project.transport.bpm }
      const pixelsPerStep = fine ? 2 : 4
      const step = fine ? .01 : 1
      onChange(current.value + Math.round((current.y - pointer.clientY) / pixelsPerStep) * step)
    }
    const finish = () => { drag.current = null; event.currentTarget.removeEventListener('pointermove', move); event.currentTarget.removeEventListener('pointerup', finish); event.currentTarget.removeEventListener('pointercancel', finish) }
    event.currentTarget.addEventListener('pointermove', move)
    event.currentTarget.addEventListener('pointerup', finish)
    event.currentTarget.addEventListener('pointercancel', finish)
  }
  const editor = (part: 'integer' | 'fraction') => <input className={`tempo-editor ${part}`} autoFocus type="number" min={part === 'integer' ? 20 : 0} max={part === 'integer' ? 300 : 99} step="1" value={draft} aria-label={part === 'integer' ? 'BPM 정수 입력' : 'BPM 소수 입력'} onChange={(event) => setDraft(event.target.value)} onBlur={() => commit(part)} onKeyDown={(event) => { if (event.key === 'Enter') event.currentTarget.blur(); else if (event.key === 'Escape') setEditingPart(null) }} />
  return <output className="tempo-control" title="위아래 드래그 · Shift 미세 조절 · 더블클릭 직접 입력">
    {editingPart === 'integer' ? editor('integer') : <button aria-label="BPM 정수" onDoubleClick={(event) => { event.preventDefault(); event.stopPropagation(); beginEdit('integer') }} onPointerDown={(event) => begin(event, 'integer')}>{integer}</button>}
    <i>.</i>
    {editingPart === 'fraction' ? editor('fraction') : <button aria-label="BPM 소수" onDoubleClick={(event) => { event.preventDefault(); event.stopPropagation(); beginEdit('fraction') }} onPointerDown={(event) => begin(event, 'fraction')}>{fraction}</button>}
  </output>
}

/**
 * Real stream telemetry from the native engine. There is no CPU-load counter in
 * the engine snapshot yet, so this reports what the engine actually measures:
 * output latency and dropout count.
 */
function StreamReadout() {
  const engine = useEngine()
  const openSettings = useProjectStore((state) => state.setAudioSettingsOpen)
  const [status, setStatus] = useState<StreamStatus>(() => engine.getStreamStatus())
  useEffect(() => {
    const timer = window.setInterval(() => setStatus({ ...engine.getStreamStatus() }), 500)
    return () => window.clearInterval(timer)
  }, [engine])
  const tone = status.error ? 'error' : status.xruns > 0 ? 'warn' : status.running ? 'ok' : 'idle'
  return (
    <button className={`stream-readout ${tone}`} onClick={() => openSettings(true)} title={status.error ?? '오디오 설정 열기'}>
      <Activity size={12} />
      <span><small>LATENCY</small><strong>{status.running ? `${status.latencyMs.toFixed(1)} ms` : '—'}</strong></span>
      <span><small>XRUNS</small><strong>{status.xruns}</strong></span>
      <span><small>PDC</small><strong>{status.pdcSamples}</strong></span>
    </button>
  )
}

function formatTime(sec: number): string {
  const minutes = Math.floor(sec / 60)
  const seconds = Math.floor(sec % 60)
  const millis = Math.floor((sec % 1) * 1000)
  return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}.${String(millis).padStart(3, '0')}`
}

function formatBars(sec: number, bpm: number, signature: { numerator: number; denominator: number }): string {
  const { bar, beat, tick } = toBarsBeats(sec, bpm, signature)
  return `${String(bar).padStart(3, '0')} · ${beat} · ${String(tick).padStart(3, '0')}`
}
