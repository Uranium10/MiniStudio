import { join } from '@tauri-apps/api/path'
import { open } from '@tauri-apps/plugin-dialog'
import { readDir } from '@tauri-apps/plugin-fs'
import { AudioLines, FolderPlus, Piano, PlugZap, RefreshCw, Search, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import type { EffectType, ExternalPluginRef, PluginDescriptor } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { rescanPlugins, scanPluginsOnce } from '../plugins/scan'
import { useProjectStore } from '../store/projectStore'
import { writeBrowserDrag, type BrowserDragPayload } from './browserPayload'

type BrowserTab = 'instrument' | 'effect' | 'media'
type MediaSample = { path: string; name: string; folder: string }

const AUDIO_PATTERN = /\.(wav|mp3|flac|ogg|m4a)$/i
const MEDIA_STORAGE_KEY = 'minidaw.media-browser.samples.v1'
const MAX_MEDIA_ITEMS = 5000

const BUILTIN_EFFECTS: Array<{ type: EffectType; name: string; description: string }> = [
  { type: 'builtin:eq', name: '4band-EQ', description: '4-band parametric EQ' },
  { type: 'builtin:eq8', name: '8band-EQ', description: '8-band parametric EQ' },
  { type: 'builtin:utility', name: 'Utility', description: 'Stereo width and gain' },
  { type: 'builtin:compressor', name: 'Compressor', description: 'Dynamics processor' },
  { type: 'builtin:multiband-compressor', name: 'Multiband Compressor', description: '3-band dynamics' },
  { type: 'builtin:distortion', name: 'Distortion', description: '3-band colour processor' },
  { type: 'builtin:disperser', name: 'Disperser', description: 'All-pass phase dispersion' },
  { type: 'builtin:delay', name: 'Echo Space', description: 'Stereo echo' },
  { type: 'builtin:reverb', name: 'Room Reverb', description: 'FDN room reverb' },
  { type: 'builtin:waveshaper', name: 'Drive Shaper', description: 'Oversampled waveshaper' },
]

export function BrowserPanel() {
  const engine = useEngine()
  const [tab, setTab] = useState<BrowserTab>('instrument')
  const [query, setQuery] = useState('')
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [samples, setSamples] = useState<MediaSample[]>(loadSamples)
  const [loading, setLoading] = useState(false)
  const [scanError, setScanError] = useState('')
  const toggle = useProjectStore((state) => state.toggleBrowser)

  const scanPlugins = (force = false) => {
    setLoading(true)
    setScanError('')
    void (force ? rescanPlugins(engine) : scanPluginsOnce(engine))
      .then(setPlugins)
      .catch((error) => setScanError(String(error)))
      .finally(() => setLoading(false))
  }

  useEffect(scanPlugins, [engine])
  useEffect(() => { localStorage.setItem(MEDIA_STORAGE_KEY, JSON.stringify(samples)) }, [samples])

  const normalized = query.trim().toLocaleLowerCase()
  const visiblePlugins = useMemo(() => plugins.filter((plugin) => (tab === 'instrument') === plugin.isInstrument && matches(`${plugin.name} ${plugin.vendor} ${plugin.category} ${plugin.format}`, normalized)), [normalized, plugins, tab])
  const visibleEffects = useMemo(() => BUILTIN_EFFECTS.filter((effect) => matches(`${effect.name} ${effect.description}`, normalized)), [normalized])
  const visibleSamples = useMemo(() => samples.filter((sample) => matches(`${sample.name} ${sample.folder} ${sample.path}`, normalized)), [normalized, samples])

  const addFolder = async () => {
    try {
      const selected = await open({ directory: true, multiple: false, title: '샘플 폴더 선택' })
      if (typeof selected !== 'string') return
      setLoading(true)
      const found = await scanAudioFolder(selected)
      setSamples((current) => mergeSamples(current, found))
      useProjectStore.getState().showToast(`${found.length}개의 오디오 샘플을 찾았습니다.`)
    } catch (error) {
      useProjectStore.getState().showToast(`샘플 폴더를 읽지 못했습니다: ${String(error)}`)
    } finally { setLoading(false) }
  }

  const addFiles = async () => {
    try {
      const selected = await open({ directory: false, multiple: true, title: '오디오 샘플 추가', filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'flac', 'ogg', 'm4a'] }] })
      const paths = Array.isArray(selected) ? selected : typeof selected === 'string' ? [selected] : []
      setSamples((current) => mergeSamples(current, paths.map(sampleFromPath)))
    } catch (error) { useProjectStore.getState().showToast(`샘플을 추가하지 못했습니다: ${String(error)}`) }
  }

  return (
    <aside className="media-browser" aria-label="미디어 브라우저">
      <header><strong>BROWSER</strong><kbd>F5</kbd><button title="브라우저 닫기" onClick={toggle}><X size={14} /></button></header>
      <nav>
        <button className={tab === 'instrument' ? 'active' : ''} onClick={() => setTab('instrument')}>악기</button>
        <button className={tab === 'effect' ? 'active' : ''} onClick={() => setTab('effect')}>이펙트</button>
        <button className={tab === 'media' ? 'active' : ''} onClick={() => setTab('media')}>미디어</button>
      </nav>
      <label className="browser-search"><Search size={13} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="검색" /></label>
      {tab === 'media' && <div className="browser-actions"><button onClick={() => { void addFolder() }}><FolderPlus size={12} /> 폴더</button><button onClick={() => { void addFiles() }}><AudioLines size={12} /> 파일</button><button title="목록 비우기" onClick={() => setSamples([])}><X size={12} /></button></div>}
      {tab !== 'media' && <div className="browser-actions"><button onClick={() => scanPlugins(true)} disabled={loading}><RefreshCw size={12} /> 다시 스캔</button><span>{plugins.length} plug-ins</span></div>}
      <div className="browser-list">
        {tab === 'instrument' && <BrowserItem icon={<Piano size={15} />} name="DefaultSynth" detail="BUILT-IN · POLY SYNTH" payload={{ kind: 'instrument' }} onOpen={() => useProjectStore.getState().addInstrumentTrack()} />}
        {tab === 'effect' && visibleEffects.map((effect) => <BrowserItem key={effect.type} icon={<PlugZap size={15} />} name={effect.name} detail={`DSP · ${effect.description}`} payload={{ kind: 'effect', type: effect.type }} onOpen={() => addEffectToFocused({ kind: 'effect', type: effect.type })} />)}
        {tab !== 'media' && visiblePlugins.map((plugin) => {
          const payload: BrowserDragPayload = plugin.isInstrument ? { kind: 'instrument', plugin: pluginRef(plugin) } : { kind: 'effect', type: `${plugin.format}:${plugin.uid}`, plugin: pluginRef(plugin) }
          return <BrowserItem key={`${plugin.format}:${plugin.uid}:${plugin.path}`} icon={plugin.isInstrument ? <Piano size={15} /> : <PlugZap size={15} />} name={plugin.name} detail={`${plugin.format.toUpperCase()} · ${plugin.vendor || plugin.category || 'Unknown'}`} payload={payload} onOpen={() => payload.kind === 'instrument' ? useProjectStore.getState().addInstrumentTrack(payload.plugin) : addEffectToFocused(payload)} />
        })}
        {tab === 'media' && visibleSamples.map((sample) => <BrowserItem key={sample.path} icon={<AudioLines size={15} />} name={sample.name} detail={sample.folder} payload={{ kind: 'media', path: sample.path, name: sample.name }} onOpen={() => { void loadSample(engine, sample) }} />)}
        {loading && <div className="browser-empty">검색 중…</div>}
        {!loading && scanError && tab !== 'media' && <div className="browser-empty error">플러그인 검색 실패<br />{scanError}</div>}
        {!loading && tab === 'media' && !visibleSamples.length && <div className="browser-empty">폴더나 파일을 추가하면<br />샘플을 여기서 드래그할 수 있습니다.</div>}
      </div>
      <footer>타임라인으로 드래그 · 더블클릭으로 바로 추가</footer>
    </aside>
  )
}

function BrowserItem({ icon, name, detail, payload, onOpen }: { icon: React.ReactNode; name: string; detail: string; payload: BrowserDragPayload; onOpen(): void }) {
  return <button className="browser-item" draggable onDragStart={(event) => writeBrowserDrag(event, payload)} onDoubleClick={onOpen}><i>{icon}</i><span><strong>{name}</strong><small>{detail}</small></span><b>⠿</b></button>
}

function pluginRef(plugin: PluginDescriptor): ExternalPluginRef {
  return { format: plugin.format, uid: plugin.uid, name: plugin.name, vendor: plugin.vendor, path: plugin.path, audioInputBuses: plugin.audioInputBuses, audioOutputBuses: plugin.audioOutputBuses, supportsSidechain: plugin.supportsSidechain, paramCount: plugin.paramCount, parameters: plugin.parameters }
}

function addEffectToFocused(payload: Extract<BrowserDragPayload, { kind: 'effect' }>): void {
  const store = useProjectStore.getState()
  const trackId = store.selectedTrackId
  if (!trackId) { store.showToast('이펙트를 넣을 트랙을 먼저 선택하세요.'); return }
  store.addEffect(trackId, payload.type, payload.plugin)
  store.setRackTarget({ kind: 'track', id: trackId })
}

async function loadSample(engine: ReturnType<typeof useEngine>, sample: MediaSample): Promise<void> {
  try { useProjectStore.getState().addAssetAsTrack(await engine.loadAudioFile(sample.path)) }
  catch (error) { useProjectStore.getState().showToast(`${sample.name}을 불러오지 못했습니다: ${String(error)}`) }
}

async function scanAudioFolder(root: string): Promise<MediaSample[]> {
  const results: MediaSample[] = []
  const queue: Array<{ path: string; depth: number }> = [{ path: root, depth: 0 }]
  while (queue.length && results.length < MAX_MEDIA_ITEMS) {
    const current = queue.shift()!
    const entries = await readDir(current.path)
    for (const entry of entries) {
      const path = await join(current.path, entry.name)
      if (entry.isDirectory && !entry.isSymlink && current.depth < 16) queue.push({ path, depth: current.depth + 1 })
      else if (entry.isFile && AUDIO_PATTERN.test(entry.name)) results.push({ path, name: entry.name, folder: folderName(current.path) })
      if (results.length >= MAX_MEDIA_ITEMS) break
    }
  }
  return results
}

function loadSamples(): MediaSample[] {
  try {
    const value = JSON.parse(localStorage.getItem(MEDIA_STORAGE_KEY) ?? '[]') as MediaSample[]
    return Array.isArray(value) ? value.filter((item) => typeof item.path === 'string' && AUDIO_PATTERN.test(item.path)).slice(0, MAX_MEDIA_ITEMS) : []
  } catch { return [] }
}

function sampleFromPath(path: string): MediaSample {
  const segments = path.split(/[\\/]/)
  return { path, name: segments.at(-1) ?? path, folder: segments.at(-2) ?? 'Samples' }
}

function mergeSamples(current: MediaSample[], added: MediaSample[]): MediaSample[] {
  return [...new Map([...current, ...added].map((sample) => [sample.path.toLocaleLowerCase(), sample])).values()].sort((left, right) => left.name.localeCompare(right.name)).slice(0, MAX_MEDIA_ITEMS)
}

function folderName(path: string): string { return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path }
function matches(value: string, query: string): boolean { return !query || value.toLocaleLowerCase().includes(query) }
