import { Piano, PlugZap, RefreshCw, Search, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import type { EffectType, ExternalPluginRef, PluginDescriptor } from '../engine'
import { COLORIZER_NAME } from '../effects/builtinEffects'
import { useEngine } from '../hooks/useEngine'
import { rescanPlugins, scanPluginsOnce } from '../plugins/scan'
import { useProjectStore } from '../store/projectStore'
import { MediaTreeBrowser } from './MediaTreeBrowser'
import { writeBrowserDrag, type BrowserDragPayload } from './browserPayload'

type BrowserTab = 'instrument' | 'effect' | 'media'

const BUILTIN_EFFECTS: Array<{ type: EffectType; name: string; description: string }> = [
  { type: 'builtin:eq', name: '4band-EQ', description: '4-band parametric EQ' },
  { type: 'builtin:eq8', name: '8band-EQ', description: '8-band parametric EQ' },
  { type: 'builtin:utility', name: 'Utility', description: 'Stereo width and gain' },
  { type: 'builtin:compressor', name: 'Compressor', description: 'Dynamics processor' },
  { type: 'builtin:upward-compressor', name: 'Upward Compressor', description: 'Low-level detail recovery' },
  { type: 'builtin:multiband-compressor', name: 'Multiband Compressor', description: '3-band dynamics' },
  { type: 'builtin:clipper', name: 'Clipper', description: '4× oversampled peak clipper' },
  { type: 'builtin:distortion', name: 'Distortion', description: '3-band colour processor' },
  { type: 'builtin:disperser', name: 'Disperser', description: 'All-pass phase dispersion' },
  { type: 'builtin:mastering-limiter', name: 'Mastering Limiter', description: 'True-peak limiting and LUFS metering' },
  { type: 'builtin:vocoder', name: 'Vocoder', description: '24-band sidechain / oscillator vocoder' },
  { type: 'builtin:lfo-tremolo', name: 'LFO Tremolo', description: 'Volume and pan modulation' },
  { type: 'builtin:roboter', name: 'Roboter', description: 'Auto-key pitch correction and harmonizer' },
  { type: 'builtin:resonator', name: COLORIZER_NAME, description: 'Harmonic spectral resonator' },
  { type: 'builtin:delay', name: 'Echo Space', description: 'Stereo echo' },
  { type: 'builtin:reverb', name: 'Room Reverb', description: 'FDN room reverb' },
  { type: 'builtin:waveshaper', name: 'Drive Shaper', description: 'Oversampled waveshaper' },
]

type BrowserEffectEntry = { key: string; name: string; detail: string; manufacturer: string; payload: Extract<BrowserDragPayload, { kind: 'effect' }> }

export function BrowserPanel() {
  const engine = useEngine()
  const [tab, setTab] = useState<BrowserTab>('instrument')
  const [query, setQuery] = useState('')
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [loading, setLoading] = useState(false)
  const [scanError, setScanError] = useState('')
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

  const normalized = query.trim().toLocaleLowerCase()
  const visibleInstruments = useMemo(() => plugins.filter((plugin) => plugin.isInstrument && matches(`${plugin.name} ${plugin.vendor} ${plugin.category} ${plugin.format}`, normalized)), [normalized, plugins])
  const effectEntries = useMemo<BrowserEffectEntry[]>(() => [
    ...BUILTIN_EFFECTS.map((effect) => ({ key: effect.type, name: effect.name, detail: `DSP · ${effect.description}`, manufacturer: 'Mini', payload: { kind: 'effect' as const, type: effect.type } })),
    ...plugins.filter((plugin) => !plugin.isInstrument).map((plugin) => ({ key: `${plugin.format}:${plugin.uid}:${plugin.path}`, name: plugin.name, detail: `${plugin.format.toUpperCase()} · ${plugin.category || 'Effect'}`, manufacturer: plugin.vendor?.trim() || 'Unknown', payload: { kind: 'effect' as const, type: `${plugin.format}:${plugin.uid}` as EffectType, plugin: pluginRef(plugin) } })),
  ].filter((effect) => matches(`${effect.name} ${effect.detail} ${effect.manufacturer}`, normalized)), [normalized, plugins])
  const manufacturerGroups = useMemo(() => [...new Set(effectEntries.map((effect) => effect.manufacturer))].sort((left, right) => left === 'Mini' ? -1 : right === 'Mini' ? 1 : left.localeCompare(right)).map((manufacturer) => ({ manufacturer, effects: effectEntries.filter((effect) => effect.manufacturer === manufacturer) })), [effectEntries])

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
        <button className={tab === 'media' ? 'active' : ''} onClick={() => setTab('media')}>미디어</button>
      </nav>
      {tab === 'media'
        ? <MediaTreeBrowser query={query} onQueryChange={setQuery} />
        : <>
          <label className="browser-search"><Search size={13} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="검색" /></label>
          <div className="browser-actions"><button onClick={() => scanPlugins(true)} disabled={loading}><RefreshCw size={12} /> 다시 스캔</button><span>{plugins.length} plug-ins</span></div>
          <div className={`browser-list ${tab === 'effect' && normalized ? 'compact-results' : ''}`}>
            {tab === 'instrument' && <BrowserItem icon={<Piano size={15} />} name="DefaultSynth" detail="BUILT-IN · POLY SYNTH" payload={{ kind: 'instrument' }} onOpen={() => addInstrumentToRack()} />}
            {tab === 'instrument' && visibleInstruments.map((plugin) => { const payload: BrowserDragPayload = { kind: 'instrument', plugin: pluginRef(plugin) }; return <BrowserItem key={`${plugin.format}:${plugin.uid}:${plugin.path}`} icon={<Piano size={15} />} name={plugin.name} detail={`${plugin.format.toUpperCase()} · ${plugin.vendor || plugin.category || 'Unknown'}`} payload={payload} onOpen={() => addInstrumentToRack(payload.plugin)} /> })}
            {tab === 'effect' && normalized && effectEntries.map((effect) => <BrowserItem compact key={effect.key} icon={<PlugZap size={13} />} name={effect.name} detail={`${effect.manufacturer} · ${effect.detail}`} payload={effect.payload} onOpen={() => addEffectToFocused(effect.payload)} />)}
            {tab === 'effect' && !normalized && manufacturerGroups.map((group) => <section className="browser-manufacturer" key={group.manufacturer}><header><span>{group.manufacturer}</span><b>{group.effects.length}</b></header>{group.effects.map((effect) => <BrowserItem key={effect.key} icon={<PlugZap size={15} />} name={effect.name} detail={effect.detail} payload={effect.payload} onOpen={() => addEffectToFocused(effect.payload)} />)}</section>)}
            {loading && <div className="browser-empty">검색 중…</div>}
            {!loading && scanError && <div className="browser-empty error">플러그인 검색 실패<br />{scanError}</div>}
          </div>
        </>}
      <footer>타임라인으로 드래그 · 더블클릭으로 바로 추가</footer>
    </aside>
  )
}

function BrowserItem({ icon, name, detail, payload, onOpen, compact = false }: { icon: React.ReactNode; name: string; detail: string; payload: BrowserDragPayload; onOpen(): void; compact?: boolean }) {
  return <button className={`browser-item ${compact ? 'compact' : ''}`} draggable onDragStart={(event) => writeBrowserDrag(event, payload)} onDoubleClick={onOpen}><i>{icon}</i><span><strong>{name}</strong><small>{detail}</small></span><b>›</b></button>
}

function pluginRef(plugin: PluginDescriptor): ExternalPluginRef {
  return { format: plugin.format, uid: plugin.uid, name: plugin.name, vendor: plugin.vendor, path: plugin.path, audioInputBuses: plugin.audioInputBuses, audioOutputBuses: plugin.audioOutputBuses, supportsSidechain: plugin.supportsSidechain, paramCount: plugin.paramCount, parameters: plugin.parameters }
}

function addEffectToFocused(payload: Extract<BrowserDragPayload, { kind: 'effect' }>): void {
  const store = useProjectStore.getState()
  const trackId = store.selectedTrackId
  if (!trackId) { store.showToast('이펙터를 넣을 트랙을 먼저 선택하세요.'); return }
  store.addEffect(trackId, payload.type, payload.plugin)
  store.setRackTarget({ kind: 'track', id: trackId })
}

function addInstrumentToRack(plugin?: ExternalPluginRef): void {
  const store = useProjectStore.getState()
  const trackId = store.addInstrumentTrack(plugin)
  store.setRackTarget({ kind: 'track', id: trackId })
}

function matches(value: string, query: string): boolean { return !query || value.toLocaleLowerCase().includes(query) }
