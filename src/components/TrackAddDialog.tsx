import { AudioLines, LoaderCircle, Piano, Plus, Search, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import type { ExternalPluginRef, PluginDescriptor } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { hydratePlugin, scanPluginsOnce } from '../plugins/scan'
import { useProjectStore } from '../store/projectStore'

type TrackChoice = 'audio' | 'instrument'

export function TrackAddDialog({ open, onClose }: { open: boolean; onClose(): void }) {
  const engine = useEngine()
  const [choice, setChoice] = useState<TrackChoice>('instrument')
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [selected, setSelected] = useState<ExternalPluginRef | undefined>()
  const [query, setQuery] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!open) return
    setQuery('')
    setError('')
    if (plugins.length) return
    let cancelled = false
    setLoading(true)
    void scanPluginsOnce(engine)
      .then((items) => { if (!cancelled) setPlugins(items.filter((plugin) => plugin.isInstrument)) })
      .catch((reason) => { if (!cancelled) setError(String(reason)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [engine, open, plugins.length])

  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase()
    return plugins.filter((plugin) => !needle || `${plugin.name} ${plugin.vendor} ${plugin.category} ${plugin.format}`.toLocaleLowerCase().includes(needle))
  }, [plugins, query])

  if (!open) return null
  const addInstrument = (plugin = selected) => {
    const store = useProjectStore.getState()
    const id = store.addInstrumentTrack(plugin)
    store.setRackTarget({ kind: 'track', id })
    onClose()
  }
  const add = () => {
    if (choice === 'audio') { useProjectStore.getState().addTrack(); onClose() }
    else addInstrument()
  }

  return <div className="modal-backdrop track-add-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose() }}>
    <section className="track-add-dialog" role="dialog" aria-modal="true" aria-label="트랙 추가">
      <header><Plus size={18} /><div><strong>트랙 추가</strong><small>트랙 유형과 사용할 악기를 한 화면에서 선택합니다.</small></div><button title="닫기" onClick={onClose}><X size={15} /></button></header>
      <div className="track-kind-picker">
        <button className={choice === 'audio' ? 'active' : ''} onClick={() => setChoice('audio')}><AudioLines size={22} /><span><strong>오디오 트랙</strong><small>녹음 및 오디오 클립용 빈 트랙</small></span></button>
        <button className={choice === 'instrument' ? 'active' : ''} onClick={() => setChoice('instrument')}><Piano size={22} /><span><strong>인스트루먼트 트랙</strong><small>DefaultSynth 또는 VST3 / CLAP 악기</small></span></button>
      </div>
      {choice === 'audio' ? <div className="audio-track-choice"><AudioLines size={42} /><strong>빈 오디오 트랙</strong><span>현재 프로젝트의 샘플레이트와 버스 구성을 사용합니다.</span></div> : <div className="track-instrument-choice">
        <label className="track-instrument-search"><Search size={13} /><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="악기 검색" /></label>
        <div className="track-instrument-list">
          <button className={!selected ? 'selected' : ''} onClick={() => setSelected(undefined)} onDoubleClick={() => addInstrument(undefined)}><Piano size={16} /><span><strong>DefaultSynth</strong><small>BUILT-IN · POLY SYNTH</small></span><b>{!selected ? '선택됨' : ''}</b></button>
          {visible.map((plugin) => { const ref = pluginRef(plugin); const active = selected?.format === ref.format && selected.uid === ref.uid && selected.path === ref.path; return <button key={`${plugin.format}:${plugin.uid}:${plugin.path}`} className={active ? 'selected' : ''} onClick={() => { setSelected(ref); void hydratePlugin(engine, plugin) }} onDoubleClick={() => { void hydratePlugin(engine, plugin); addInstrument(ref) }}><Piano size={16} /><span><strong>{plugin.name}</strong><small>{plugin.format.toUpperCase()} · {plugin.vendor || plugin.category || 'Unknown'}</small></span><b>{active ? '선택됨' : ''}</b></button> })}
          {loading && <div className="track-add-state"><LoaderCircle className="spin" size={18} /> 플러그인 검색 중…</div>}
          {!loading && error && <div className="track-add-state error">플러그인 검색 실패 · {error}</div>}
          {!loading && !error && !visible.length && query && <div className="track-add-state">검색 결과가 없습니다.</div>}
        </div>
      </div>}
      <footer><span>{choice === 'audio' ? 'AUDIO' : selected ? `${selected.format.toUpperCase()} · ${selected.name}` : 'BUILT-IN · DefaultSynth'}</span><div><button onClick={onClose}>취소</button><button className="primary" onClick={add}>트랙 추가</button></div></footer>
    </section>
  </div>
}

function pluginRef(plugin: PluginDescriptor): ExternalPluginRef {
  return { format: plugin.format, uid: plugin.uid, name: plugin.name, vendor: plugin.vendor, path: plugin.path, audioInputBuses: plugin.audioInputBuses, audioOutputBuses: plugin.audioOutputBuses, supportsSidechain: plugin.supportsSidechain, hasEditor: plugin.hasEditor, paramCount: plugin.paramCount, parameters: plugin.parameters }
}
