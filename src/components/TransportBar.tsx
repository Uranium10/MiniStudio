// Fixed transport controls driven by the audio-engine clock.
import { Activity, ChevronUp, Circle, Gauge, ListMusic, Pause, Play, Repeat2, SkipBack, Square, Waves } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useEngine } from '../hooks/useEngine'
import { toBarsBeats, type StreamStatus } from '../engine'
import { seekTo, stopPlayback, togglePlayback } from '../store/commands'
import { useProjectStore } from '../store/projectStore'
import { EditableNumber } from './controls'

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
        <label><Gauge size={14} /><span><small>TEMPO</small><input type="number" min="20" max="300" step="0.1" value={bpm} onChange={(event) => setBpm(Number(event.target.value))} /></span></label>
        <label><ListMusic size={14} /><span><small>SIGNATURE</small><span className="signature-input">
          <input type="number" min="1" max="32" value={signature.numerator} aria-label="박자 분자" onChange={(event) => setTimeSignature({ ...signature, numerator: Number(event.target.value) })} />
          <i>/</i>
          <select value={signature.denominator} aria-label="박자 분모" onChange={(event) => setTimeSignature({ ...signature, denominator: Number(event.target.value) })}>{[1, 2, 4, 8, 16].map((value) => <option key={value} value={value}>{value}</option>)}</select>
        </span></span></label>
        <div><Waves size={14} /><span><small>SAMPLE RATE</small><strong>{sampleRate / 1000} kHz</strong></span></div>
        {recordingEnabled && <label><span><small>OVERDUB</small><select value={overdubMode} onChange={(event) => useProjectStore.setState({ overdubMode: event.target.value as 'merge' | 'new' })}><option value="merge">Merge</option><option value="new">New clip</option></select></span></label>}
      </div>
      <StreamReadout />
    </footer>
  )
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
