import { CheckCircle2, Download, LoaderCircle, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import type { ExportProgress, ExportSettings } from '../engine'
import { describeEngineError } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { chooseExportPath } from '../io/projectFiles'
import { getProjectSnapshot, useProjectStore } from '../store/projectStore'

type Phase = 'settings' | 'rendering' | 'done' | 'error'

export function ExportDialog() {
  const open = useProjectStore((state) => state.exportDialogOpen)
  const setOpen = useProjectStore((state) => state.setExportDialogOpen)
  const projectName = useProjectStore((state) => state.project.meta.name)
  const engine = useEngine()
  const [settings, setSettings] = useState<ExportSettings>({ format: 'wav', sampleRate: 48_000, bitDepth: 24, mp3BitrateKbps: 320, normalize: false })
  const [phase, setPhase] = useState<Phase>('settings')
  const [progress, setProgress] = useState<ExportProgress | null>(null)
  const [message, setMessage] = useState('')

  useEffect(() => {
    if (!open) return
    setPhase('settings')
    setProgress(null)
    setMessage('')
    void engine.getAudioSettings().then((audio) => setSettings((current) => ({ ...current, sampleRate: audio.sampleRate })))
  }, [engine, open])
  useEffect(() => engine.onExportProgress(setProgress), [engine])

  if (!open) return null
  const close = () => { if (phase === 'rendering') return; setOpen(false) }
  const start = async () => {
    const path = await chooseExportPath(projectName, settings.format)
    if (!path) return
    setPhase('rendering')
    setMessage(path)
    try {
      await engine.exportProject(getProjectSnapshot(), path, settings)
      setPhase('done')
      useProjectStore.getState().showToast('내보내기를 완료했습니다')
    } catch (error) {
      setPhase('error')
      setMessage(describeEngineError(error))
    }
  }
  const fraction = Math.max(0, Math.min(1, progress?.fraction ?? 0))

  return <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) close() }}>
    <section className="export-dialog" role="dialog" aria-modal="true" aria-label="프로젝트 내보내기">
      <header><Download size={18} /><div><strong>내보내기</strong><small>마스터 출력을 오프라인 렌더링합니다</small></div><button title="닫기" disabled={phase === 'rendering'} onClick={close}><X size={15} /></button></header>
      {phase === 'settings' && <div className="export-settings">
        <label><span>출력 형식</span><select value={settings.format} onChange={(event) => setSettings({ ...settings, format: event.target.value as 'wav' | 'mp3' })}><option value="wav">WAV · 무손실</option><option value="mp3">MP3 · 호환용</option><option disabled>FLAC — 준비 중</option><option disabled>OGG — 준비 중</option></select></label>
        <div className="export-grid">
          <label><span>샘플레이트</span><select value={settings.sampleRate} onChange={(event) => setSettings({ ...settings, sampleRate: Number(event.target.value) })}>{[44_100, 48_000, 88_200, 96_000, 192_000].map((rate) => <option key={rate} value={rate} disabled={rate !== settings.sampleRate}>{(rate / 1000).toFixed(1)} kHz{rate === settings.sampleRate ? ' · 현재 엔진' : ''}</option>)}</select><small>다른 레이트는 오디오 설정에서 엔진 레이트를 바꾼 뒤 선택할 수 있습니다.</small></label>
          {settings.format === 'wav' ? <label><span>비트 깊이</span><select value={settings.bitDepth} onChange={(event) => setSettings({ ...settings, bitDepth: Number(event.target.value) as 16 | 24 | 32 })}><option value="16">16-bit PCM</option><option value="24">24-bit PCM</option><option value="32">32-bit Float</option></select></label> : <label><span>MP3 비트레이트</span><select value={settings.mp3BitrateKbps} onChange={(event) => setSettings({ ...settings, mp3BitrateKbps: Number(event.target.value) as 128 | 192 | 256 | 320 })}><option value="128">128 kbps</option><option value="192">192 kbps</option><option value="256">256 kbps</option><option value="320">320 kbps · 최고</option></select></label>}
        </div>
        <label><span>렌더 범위</span><select><option>전체 프로젝트 + 이펙트 테일</option></select></label>
        <label className="export-check"><input type="checkbox" checked={settings.normalize} onChange={(event) => setSettings({ ...settings, normalize: event.target.checked })} /><span><strong>피크 정규화</strong><small>클리핑 없이 최대 피크를 0 dBFS에 맞춥니다.</small></span></label>
      </div>}
      {phase === 'rendering' && <div className="export-progress-modal"><LoaderCircle className="spin" size={34} /><strong>{progress?.stage || '마스터 렌더링 준비 중'}</strong><span>{Math.round(fraction * 100)}%</span><div><i style={{ width: `${fraction * 100}%` }} /></div><small>{message}</small></div>}
      {phase === 'done' && <div className="export-result success"><CheckCircle2 size={36} /><strong>내보내기 완료</strong><span>{message}</span></div>}
      {phase === 'error' && <div className="export-result error"><X size={36} /><strong>내보내기 실패</strong><span>{message}</span></div>}
      <footer>{phase === 'settings' ? <><button onClick={close}>취소</button><button className="primary" onClick={() => { void start() }}>내보내기</button></> : phase === 'rendering' ? <button className="danger" onClick={() => engine.cancelExport()}>렌더링 취소</button> : <button className="primary" onClick={close}>닫기</button>}</footer>
    </section>
  </div>
}
