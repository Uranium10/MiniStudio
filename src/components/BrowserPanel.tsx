import { ArrowDownAZ, ArrowUpAZ, Piano, PlugZap, RefreshCw, Search, Star, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import type { EffectType, ExternalPluginRef, PluginDescriptor } from '../engine'
import { COLORIZER_NAME } from '../effects/builtinEffects'
import { useEngine } from '../hooks/useEngine'
import { hydratePlugin, rescanPlugins, scanPluginsOnce, subscribePluginScan } from '../plugins/scan'
import { openPluginEditorWhenReady } from '../plugins/editor'
import { useProjectStore } from '../store/projectStore'
import { MediaTreeBrowser } from './MediaTreeBrowser'
import { beginBrowserDrag, type BrowserDragPayload } from './browserPayload'

type BrowserTab = 'instrument' | 'effect' | 'files'
type EffectView = 'all' | 'manufacturer' | 'type' | 'favorites'
const EFFECT_FAVORITES_KEY = 'ministudio.browser.effect-favorites.v1'

const BUILTIN_EFFECTS: Array<{ type: EffectType; name: string; description: string }> = [
  { type: 'builtin:eq', name: '4band-EQ', description: '4-band parametric EQ' },
  { type: 'builtin:eq8', name: '8band-EQ', description: '8-band parametric EQ' },
  { type: 'builtin:utility', name: 'Utility', description: 'Stereo width and gain' },
  { type: 'builtin:compressor', name: 'Compressor', description: 'Dynamics processor' },
  { type: 'builtin:upward-compressor', name: 'Upward Compressor', description: 'Low-level detail recovery' },
  { type: 'builtin:transient-shaper', name: 'Transient Shaper', description: 'Attack and sustain contouring' },
  { type: 'builtin:multiband-compressor', name: 'Multiband Compressor', description: '3-band dynamics' },
  { type: 'builtin:clipper', name: 'Clipper', description: '4× oversampled peak clipper' },
  { type: 'builtin:distortion', name: 'Distortion', description: '3-band colour processor' },
  { type: 'builtin:disperser', name: 'Disperser', description: 'All-pass phase dispersion' },
  { type: 'builtin:mastering-limiter', name: 'Mastering Limiter', description: 'True-peak limiting and LUFS metering' },
  { type: 'builtin:vocoder', name: 'Vocoder', description: '24-band sidechain / oscillator vocoder' },
  { type: 'builtin:lfo-tremolo', name: 'LFO Tremolo', description: 'Volume and pan modulation' },
  { type: 'builtin:roboter', name: 'Roboter', description: 'Auto-key pitch correction and harmonizer' },
  { type: 'builtin:resonator', name: COLORIZER_NAME, description: 'Harmonic spectral resonator' },
  { type: 'builtin:formant-shifter', name: 'Formant Shifter', description: 'Auto Mono/Poly pitch + formant shift' },
  { type: 'builtin:delay', name: 'Echo Space', description: 'Stereo echo' },
  { type: 'builtin:reverb', name: 'Room Reverb', description: 'FDN room reverb' },
  { type: 'builtin:waveshaper', name: 'Drive Shaper', description: 'Oversampled waveshaper' },
]

type BrowserEffectEntry = { key: string; name: string; detail: string; manufacturer: string; category: string; payload: Extract<BrowserDragPayload, { kind: 'effect' }>; plugin?: PluginDescriptor }

export function BrowserPanel() {
  const engine = useEngine()
  const [tab, setTab] = useState<BrowserTab>('instrument')
  const [query, setQuery] = useState('')
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [loading, setLoading] = useState(false)
  const [scanError, setScanError] = useState('')
  const [effectView, setEffectView] = useState<EffectView>('manufacturer')
  const [ascending, setAscending] = useState(true)
  const [favorites, setFavorites] = useState<Set<string>>(() => loadFavorites())
  const toggle = useProjectStore((state) => state.toggleBrowser)
  const dock = useProjectStore((state) => state.browserDock)
  const setDock = useProjectStore((state) => state.setBrowserDock)
  const dockDrag = useRef<number | null>(null)

  const scanPlugins = (force = false) => {
    setLoading(true)
    setScanError('')
    void (force ? rescanPlugins(engine) : scanPluginsOnce(engine))
      .then(setPlugins)
      .catch((error) => setScanError(String(error)))
      .finally(() => setLoading(false))
  }

  useEffect(scanPlugins, [engine])
  useEffect(() => subscribePluginScan(setPlugins), [])
  useEffect(() => localStorage.setItem(EFFECT_FAVORITES_KEY, JSON.stringify([...favorites])), [favorites])

  const normalized = query.trim().toLocaleLowerCase()
  const visibleInstruments = useMemo(() => plugins.filter((plugin) => plugin.isInstrument && matches(`${plugin.name} ${plugin.vendor} ${plugin.category} ${plugin.format}`, normalized)), [normalized, plugins])
  const effectEntries = useMemo<BrowserEffectEntry[]>(() => [
    ...BUILTIN_EFFECTS.map((effect) => ({ key: effect.type, name: effect.name, detail: `DSP · ${effect.description}`, manufacturer: 'Mini', category: builtinEffectCategory(effect.type), payload: { kind: 'effect' as const, type: effect.type } })),
    ...plugins.filter((plugin) => !plugin.isInstrument).map((plugin) => ({ key: `${plugin.format}:${plugin.uid}:${plugin.path}`, name: plugin.name, detail: `${plugin.format.toUpperCase()} · ${plugin.category || 'Effect'}`, manufacturer: plugin.vendor?.trim() || 'Unknown', category: plugin.category?.trim() || 'Effect', payload: { kind: 'effect' as const, type: `${plugin.format}:${plugin.uid}` as EffectType, plugin: pluginRef(plugin) }, plugin })),
  ].filter((effect) => matches(`${effect.name} ${effect.detail} ${effect.manufacturer} ${effect.category}`, normalized)), [normalized, plugins])
  const sortedEffects = useMemo(() => [...effectEntries].sort((left, right) => (ascending ? 1 : -1) * left.name.localeCompare(right.name)), [ascending, effectEntries])
  const visibleEffects = useMemo(() => effectView === 'favorites' ? sortedEffects.filter((effect) => favorites.has(effect.key)) : sortedEffects, [effectView, favorites, sortedEffects])
  const effectGroups = useMemo(() => {
    const keyOf = effectView === 'type' ? (effect: BrowserEffectEntry) => effect.category : (effect: BrowserEffectEntry) => effect.manufacturer
    if (effectView !== 'manufacturer' && effectView !== 'type') return []
    return [...new Set(visibleEffects.map(keyOf))].sort((left, right) => left === 'Mini' ? -1 : right === 'Mini' ? 1 : left.localeCompare(right)).map((label) => ({ label, effects: visibleEffects.filter((effect) => keyOf(effect) === label) }))
  }, [effectView, visibleEffects])
  const toggleFavorite = (key: string) => setFavorites((current) => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next })
  const prepareEffect = (effect: BrowserEffectEntry) => {
    if (!effect.plugin) return Promise.resolve(effect.payload)
    return hydratePlugin(engine, effect.plugin).then((detailed) => {
      effect.payload.plugin = pluginRef(detailed)
      return effect.payload
    })
  }

  return (
    <aside className={`media-browser dock-${dock}`} aria-label="미디어 브라우저">
      <header
        title="좌우로 드래그해 브라우저 도킹 위치 변경"
        onPointerDown={(event) => { if ((event.target as Element).closest('button')) return; dockDrag.current = event.pointerId; event.currentTarget.setPointerCapture(event.pointerId); event.currentTarget.parentElement?.classList.add('docking') }}
        onPointerMove={(event) => { if (dockDrag.current === event.pointerId) event.currentTarget.style.setProperty('--dock-x', `${event.clientX}px`) }}
        onPointerUp={(event) => { if (dockDrag.current !== event.pointerId) return; dockDrag.current = null; event.currentTarget.releasePointerCapture(event.pointerId); event.currentTarget.parentElement?.classList.remove('docking'); event.currentTarget.style.removeProperty('--dock-x'); setDock(event.clientX < window.innerWidth / 2 ? 'left' : 'right') }}
        onPointerCancel={(event) => { dockDrag.current = null; event.currentTarget.parentElement?.classList.remove('docking'); event.currentTarget.style.removeProperty('--dock-x') }}
      ><strong>BROWSER <small>{dock === 'right' ? 'RIGHT' : 'LEFT'}</small></strong><kbd>F5</kbd><button title="브라우저 닫기" onClick={toggle}><X size={14} /></button></header>
      <nav>
        <button className={tab === 'instrument' ? 'active' : ''} onClick={() => setTab('instrument')}>악기</button>
        <button className={tab === 'effect' ? 'active' : ''} onClick={() => setTab('effect')}>이펙터</button>
        <button className={tab === 'files' ? 'active' : ''} onClick={() => setTab('files')}>파일</button>
      </nav>
      {tab === 'files'
        ? <MediaTreeBrowser query={query} onQueryChange={setQuery} />
        : <>
          <label className="browser-search"><Search size={13} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="검색" /></label>
          <div className="browser-actions"><button onClick={() => scanPlugins(true)} disabled={loading}><RefreshCw size={12} /> 다시 스캔</button>{tab === 'effect' && <><select aria-label="이펙터 정렬 방식" value={effectView} onChange={(event) => setEffectView(event.target.value as EffectView)}><option value="all">전체</option><option value="manufacturer">제조사</option><option value="type">종류</option><option value="favorites">즐겨찾기</option></select><button className="browser-sort-direction" title={ascending ? '이름 내림차순' : '이름 오름차순'} onClick={() => setAscending((value) => !value)}>{ascending ? <ArrowDownAZ size={12} /> : <ArrowUpAZ size={12} />}</button></>}<span>{plugins.length} plug-ins</span></div>
          <div className={`browser-list ${tab === 'effect' && normalized ? 'compact-results' : ''}`}>
            {tab === 'instrument' && <BrowserItem icon={<Piano size={15} />} name="DefaultSynth" detail="BUILT-IN · POLY SYNTH" payload={{ kind: 'instrument' }} onOpen={() => addInstrumentToRack(engine)} />}
            {tab === 'instrument' && visibleInstruments.map((plugin) => { const payload: Extract<BrowserDragPayload, { kind: 'instrument' }> = { kind: 'instrument', plugin: pluginRef(plugin) }; const prepare = () => hydratePlugin(engine, plugin).then((detailed) => { payload.plugin = pluginRef(detailed); return detailed }); return <BrowserItem key={`${plugin.format}:${plugin.uid}:${plugin.path}`} icon={<Piano size={15} />} name={plugin.name} detail={`${plugin.format.toUpperCase()} · ${plugin.vendor || plugin.category || 'Unknown'}`} payload={payload} onPrepare={() => { void prepare() }} onOpen={() => { void prepare().then((detailed) => addInstrumentToRack(engine, pluginRef(detailed))) }} /> })}
            {tab === 'effect' && normalized && visibleEffects.map((effect) => <BrowserItem compact key={effect.key} icon={<PlugZap size={13} />} name={effect.name} detail={`${effect.manufacturer} · ${effect.detail}`} payload={effect.payload} favorite={favorites.has(effect.key)} onFavorite={() => toggleFavorite(effect.key)} onPrepare={() => { void prepareEffect(effect) }} onOpen={() => { void prepareEffect(effect).then(addEffectToFocused) }} />)}
            {tab === 'effect' && !normalized && (effectView === 'all' || effectView === 'favorites') && visibleEffects.map((effect) => <BrowserItem key={effect.key} icon={<PlugZap size={15} />} name={effect.name} detail={`${effect.manufacturer} · ${effect.category}`} payload={effect.payload} favorite={favorites.has(effect.key)} onFavorite={() => toggleFavorite(effect.key)} onPrepare={() => { void prepareEffect(effect) }} onOpen={() => { void prepareEffect(effect).then(addEffectToFocused) }} />)}
            {tab === 'effect' && !normalized && effectGroups.map((group) => <details className="browser-manufacturer" key={group.label}><summary><span>{group.label}</span><b>{group.effects.length}</b></summary>{group.effects.map((effect) => <BrowserItem key={effect.key} icon={<PlugZap size={15} />} name={effect.name} detail={effect.detail} payload={effect.payload} favorite={favorites.has(effect.key)} onFavorite={() => toggleFavorite(effect.key)} onPrepare={() => { void prepareEffect(effect) }} onOpen={() => { void prepareEffect(effect).then(addEffectToFocused) }} />)}</details>)}
            {tab === 'effect' && !loading && !visibleEffects.length && <div className="browser-empty">{effectView === 'favorites' ? '즐겨찾기한 이펙터가 없습니다.' : '표시할 이펙터가 없습니다.'}</div>}
            {loading && <div className="browser-empty">검색 중…</div>}
            {!loading && scanError && <div className="browser-empty error">플러그인 검색 실패<br />{scanError}</div>}
          </div>
        </>}
      <footer>타임라인으로 드래그 · 더블클릭으로 바로 추가</footer>
    </aside>
  )
}

function BrowserItem({ icon, name, detail, payload, onOpen, onPrepare, compact = false, favorite, onFavorite }: { icon: React.ReactNode; name: string; detail: string; payload: BrowserDragPayload; onOpen(): void; onPrepare?(): void; compact?: boolean; favorite?: boolean; onFavorite?(): void }) {
  return <button className={`browser-item ${compact ? 'compact' : ''}`} onPointerDown={(event) => { onPrepare?.(); beginBrowserDrag(event, payload) }} onDoubleClick={() => { onPrepare?.(); onOpen() }}><i>{icon}</i><span><strong>{name}</strong><small>{detail}</small></span>{onFavorite ? <b className={`browser-favorite ${favorite ? 'active' : ''}`} role="checkbox" aria-checked={favorite} title={favorite ? '즐겨찾기 해제' : '즐겨찾기 추가'} onPointerDown={(event) => event.stopPropagation()} onClick={(event) => { event.preventDefault(); event.stopPropagation(); onFavorite() }}><Star size={11} fill={favorite ? 'currentColor' : 'none'} /></b> : <b>›</b>}</button>
}

function pluginRef(plugin: PluginDescriptor): ExternalPluginRef {
  return { format: plugin.format, uid: plugin.uid, name: plugin.name, vendor: plugin.vendor, path: plugin.path, audioInputBuses: plugin.audioInputBuses, audioOutputBuses: plugin.audioOutputBuses, supportsSidechain: plugin.supportsSidechain, hasEditor: plugin.hasEditor, paramCount: plugin.paramCount, parameters: plugin.parameters }
}

function addEffectToFocused(payload: Extract<BrowserDragPayload, { kind: 'effect' }>): void {
  const store = useProjectStore.getState()
  const trackId = store.selectedTrackId
  if (!trackId) { store.showToast('이펙터를 넣을 트랙을 먼저 선택하세요.'); return }
  store.addEffect(trackId, payload.type, payload.plugin)
  store.setRackTarget({ kind: 'track', id: trackId })
}

function addInstrumentToRack(engine: ReturnType<typeof useEngine>, plugin?: ExternalPluginRef): void {
  const store = useProjectStore.getState()
  const trackId = store.addInstrumentTrack(plugin)
  store.setRackTarget({ kind: 'track', id: trackId })
  if (plugin && plugin.hasEditor !== false) openPluginEditorWhenReady(engine, 'instrument', trackId, (error) => store.showToast(`${plugin.name} 편집기를 열 수 없습니다: ${String(error)}`))
}

function matches(value: string, query: string): boolean { return !query || value.toLocaleLowerCase().includes(query) }

function loadFavorites(): Set<string> {
  try { const value = JSON.parse(localStorage.getItem(EFFECT_FAVORITES_KEY) ?? '[]'); return new Set(Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : []) } catch { return new Set() }
}

function builtinEffectCategory(type: EffectType): string {
  if (type === 'builtin:eq' || type === 'builtin:eq8') return 'EQ'
  if (type.includes('compressor') || type.includes('transient') || type.includes('limiter') || type.includes('clipper')) return 'Dynamics'
  if (type.includes('distortion') || type.includes('waveshaper') || type.includes('disperser') || type.includes('resonator')) return 'Color'
  if (type.includes('delay') || type.includes('reverb')) return 'Time'
  if (type.includes('vocoder') || type.includes('tremolo') || type.includes('roboter')) return 'Modulation'
  return 'Utility'
}
