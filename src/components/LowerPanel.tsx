// Resizable Ableton-inspired mixer, device rack, and phase-two plug-in pane.
import { ChevronDown, CirclePower, Copy, FolderOpen, GripVertical, Layers3, Minus, Piano, Plus, Power, Save, Trash2, X } from 'lucide-react'
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { Bus, EffectInstance, EffectType, ExternalPluginRef, PluginDescriptor, Track } from '../engine'
import { describeEngineError, effectiveMasterGainDb, MUTE_GAIN_DB } from '../engine'
import { COLORIZER_NAME } from '../effects/builtinEffects'
import { useEngine } from '../hooks/useEngine'
import { hydratePlugin, hydratePluginRef, scanPluginsOnce } from '../plugins/scan'
import { openPluginEditorWhenReady } from '../plugins/editor'
import { openDevicePreset, saveDevicePreset } from '../io/projectFiles'
import { automationOptionsForTrack, useProjectStore, type LowerTab, type RackTarget } from '../store/projectStore'
import { AutomationButton, EditableNumber, Knob, LevelMeter, ParameterAutomationProvider, subscribeAnalyzerFrame, subscribeMeterFrame } from './controls'
import { subscribeBrowserDrag } from './browserPayload'
import { FloatingPanel, MenuPanel, type MenuItem } from './Menu'
import { beginPointerReorder } from './pointerReorder'
import { displayFrequencyAtX, FREQUENCY_TICKS, frequencyToX, parameterFrequencyAtX, spectrumFrequencyAtIndex } from './frequencyScale'

export function LowerPanel() {
  const collapsed = useProjectStore((state) => state.lowerPanelCollapsed)
  const tab = useProjectStore((state) => state.lowerTab)
  const setTab = useProjectStore((state) => state.setLowerTab)
  const setHeight = useProjectStore((state) => state.setLowerPanelHeight)
  const toggle = useProjectStore((state) => state.toggleLowerPanel)
  const trackCount = useProjectStore((state) => state.project.tracks.length)
  const busCount = useProjectStore((state) => state.project.buses.length)

  const beginResize = (event: React.PointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId)
    const move = (pointer: PointerEvent) => setHeight(window.innerHeight - pointer.clientY - 48)
    const up = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up) }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <section className={`lower-panel ${collapsed ? 'collapsed' : ''}`}>
      <div className="panel-splitter" onPointerDown={beginResize}><span /></div>
      {!collapsed && <>
        <div className="panel-tabs">
          {(['mixer', 'effects'] as LowerTab[]).map((item) => <button key={item} className={tab === item ? 'active' : ''} onClick={() => setTab(item)}>{item === 'mixer' ? '믹서' : '이펙트'}</button>)}
          <div className="panel-tabs-spacer" /><span>{tab === 'mixer' ? `${trackCount} Tracks · ${busCount} Returns · Master` : 'Signal flows left → right'}</span>
          <button className="collapse-panel" onClick={toggle} title="하단 패널 접기 (Tab)"><ChevronDown size={15} /></button>
        </div>
        <div className="panel-content">{tab === 'mixer' ? <Mixer /> : <DeviceRack />}</div>
      </>}
    </section>
  )
}

function Mixer() {
  const tracks = useProjectStore((state) => state.project.tracks)
  const buses = useProjectStore((state) => state.project.buses)
  const clearTrackSelection = useProjectStore((state) => state.selectTrack)
  return (
    <div className="mixer-shell">
      <div className="mixer-scroll" onClick={(event) => { if (event.target === event.currentTarget) clearTrackSelection(null) }}>
        {tracks.map((track, index) => <TrackStrip key={track.id} track={track} index={index} />)}
        {buses.map((bus) => <BusStrip key={bus.id} bus={bus} />)}
      </div>
      <MasterStrip />
    </div>
  )
}

function TrackStripView({ track, index }: { track: Track; index: number }) {
  const selected = useProjectStore((state) => state.selectedTrackIds.includes(track.id))
  const selectedTrackIds = useProjectStore((state) => state.selectedTrackIds)
  const selectTrack = useProjectStore((state) => state.selectTrack)
  const updateTrack = useProjectStore((state) => state.updateTrack)
  const updateVolumes = useProjectStore((state) => state.updateTrackVolumes)
  const createBus = useProjectStore((state) => state.createBusFromSelectedTracks)
  const updateSend = useProjectStore((state) => state.updateSend)
  const addSend = useProjectStore((state) => state.addSend)
  const removeSend = useProjectStore((state) => state.removeSend)
  const updateSendRoute = useProjectStore((state) => state.updateSendRoute)
  const buses = useProjectStore((state) => state.project.buses)
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const [renaming, setRenaming] = useState(false)
  const [nameDraft, setNameDraft] = useState(track.name)
  const nameRef = useRef<HTMLInputElement>(null)
  const faderStart = useRef<{ source: number; values: Map<string, number> } | null>(null)
  const engine = useEngine()
  const focusSingle = () => selectTrack(track.id)
  const openFx = () => { focusSingle(); useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id }) }
  const beginRename = () => { setNameDraft(track.name); setRenaming(true); queueMicrotask(() => { nameRef.current?.focus(); nameRef.current?.select() }) }
  const finishRename = () => { if (!renaming) return; const name = nameDraft.trim(); if (name && name !== track.name) updateTrack(track.id, { name }); setRenaming(false) }
  const beginFader = (event: React.PointerEvent<HTMLInputElement>) => {
    event.stopPropagation()
    const state = useProjectStore.getState()
    if (!state.selectedTrackIds.includes(track.id)) state.selectTrack(track.id)
    const current = useProjectStore.getState()
    faderStart.current = { source: track.volumeDb, values: new Map(current.project.tracks.filter((item) => current.selectedTrackIds.includes(item.id)).map((item) => [item.id, item.volumeDb])) }
  }
  const applyGroupVolume = (value: number, fromGesture = true) => {
    const state = useProjectStore.getState()
    const start = fromGesture ? faderStart.current : { source: track.volumeDb, values: new Map(state.project.tracks.filter((item) => state.selectedTrackIds.includes(item.id)).map((item) => [item.id, item.volumeDb])) }
    if (!start) return
    const delta = value - start.source
    const updates = [...start.values].map(([id, initial]) => ({ id, volumeDb: Math.max(-60, Math.min(12, initial + delta)) }))
    updateVolumes(updates)
    for (const update of updates) engine.setTrackVolume(update.id, update.volumeDb)
  }
  const contextItems: MenuItem[] = menu ? [
    { kind: 'label', label: selectedTrackIds.length > 1 ? `${selectedTrackIds.length}개 트랙 선택됨` : track.name },
    { kind: 'item', label: '이펙트 체인 열기', run: openFx },
    { kind: 'item', label: '이름 바꾸기', run: beginRename },
    { kind: 'item', label: '선택 트랙으로 버스 채널 생성', icon: <Layers3 size={13} />, disabled: selectedTrackIds.length < 2, run: () => { createBus() } },
    { kind: 'separator' },
    { kind: 'item', label: '트랙 삭제', danger: true, run: () => useProjectStore.getState().removeTrack(track.id) },
  ] : []
  return (
    <article className={`channel-strip ${selected ? 'selected' : ''}`} onClick={(event) => selectTrack(track.id, event.shiftKey)} onContextMenu={(event) => { event.preventDefault(); if (!selected) selectTrack(track.id); setMenu({ x: event.clientX, y: event.clientY }) }}>
      <div className="channel-color" style={{ background: track.color }} />
      <div className="channel-title"><span>{String(index + 1).padStart(2, '0')}</span><input ref={nameRef} value={renaming ? nameDraft : track.name} readOnly={!renaming} onClick={(event) => { event.stopPropagation(); selectTrack(track.id, event.shiftKey) }} onDoubleClick={(event) => { event.stopPropagation(); selectTrack(track.id); beginRename() }} onChange={(event) => setNameDraft(event.target.value)} onBlur={finishRename} onKeyDown={(event) => { if (event.key === 'Enter') event.currentTarget.blur(); else if (event.key === 'Escape') { setNameDraft(track.name); setRenaming(false); event.currentTarget.blur() } }} /><button className="channel-fx-button" title="이펙트 체인 열기" onClick={(event) => { event.stopPropagation(); openFx() }}>FX</button></div>
      <div className="channel-buttons">
        <button className={track.muted ? 'active mute' : ''} onClick={(event) => { event.stopPropagation(); focusSingle(); updateTrack(track.id, { muted: !track.muted }); engine.setTrackMute(track.id, !track.muted) }}>M</button>
        <button className={track.solo ? 'active solo' : ''} onClick={(event) => { event.stopPropagation(); focusSingle(); updateTrack(track.id, { solo: !track.solo }); engine.setTrackSolo(track.id, !track.solo) }}>S</button>
        <button className={track.armed ? 'active arm' : ''} onClick={(event) => { event.stopPropagation(); focusSingle(); updateTrack(track.id, { armed: !track.armed }) }}>●</button>
      </div>
      <div className="send-routes">
        {track.sends.map((send) => <div className="send-route" key={send.id}><span><select aria-label="센드 라우팅" value={send.targetBusId} onChange={(event) => { if (!event.target.value) removeSend(track.id, send.id); else updateSendRoute(track.id, send.id, event.target.value) }}><option value="">센드 제거</option>{buses.map((bus) => <option key={bus.id} value={bus.id}>{bus.name}</option>)}</select><button title="센드 제거" onClick={() => removeSend(track.id, send.id)}><X size={9} /></button></span><input title="센드 레벨" type="range" min="-60" max="6" step="0.1" value={send.gainDb} onChange={(event) => { const value = Number(event.target.value); updateSend(track.id, send.id, value); engine.setSendLevel(send.id, value) }} /><EditableNumber value={send.gainDb} min={-60} max={6} step={0.1} onChange={(value) => { updateSend(track.id, send.id, value); engine.setSendLevel(send.id, value) }} format={(value) => value <= -59.9 ? '-∞' : `${value.toFixed(1)} dB`} /></div>)}
        <select className="add-send-select" value="" onChange={(event) => { if (event.target.value) addSend(track.id, event.target.value) }}><option value="">+ 센드 추가</option>{buses.filter((bus) => !track.sends.some((send) => send.targetBusId === bus.id)).map((bus) => <option key={bus.id} value={bus.id}>{bus.name}</option>)}</select>
      </div>
      <label className="pan-control"><span>L</span><input type="range" min="-1" max="1" step="0.01" value={track.pan} onDoubleClick={() => updateTrack(track.id, { pan: 0 })} onChange={(event) => { const value = Number(event.target.value); updateTrack(track.id, { pan: value }); engine.setTrackPan(track.id, value) }} /><span>R</span></label>
      <div className="fader-zone">
        <div className="fader-scale"><span>+6</span><span>0</span><span>-12</span><span>-30</span><span>-∞</span></div>
        <input className="vertical-fader" style={faderStyle(track.volumeDb)} aria-valuetext={`${track.volumeDb.toFixed(1)} dB`} type="range" min="-60" max="12" step="0.1" value={track.volumeDb} onClick={(event) => event.stopPropagation()} onPointerDown={beginFader} onPointerUp={() => { faderStart.current = null }} onPointerCancel={() => { faderStart.current = null }} onDoubleClick={(event) => { event.stopPropagation(); applyGroupVolume(0, false) }} onChange={(event) => applyGroupVolume(Number(event.target.value))} />
        <LevelMeter trackId={track.id} />
      </div>
      <div className="db-readout-shell" onClick={(event) => event.stopPropagation()}><EditableNumber className="db-readout" value={track.volumeDb} min={-60} max={12} step={0.1} onChange={(value) => applyGroupVolume(value, false)} format={(value) => `${value <= -59.9 ? '-∞' : value.toFixed(1)} dB`} /></div>
      <select className="channel-output" title={track.outputBusId ? '그룹 버스로 출력' : '마스터로 출력'} value={track.outputBusId ?? ''} onClick={(event) => { event.stopPropagation(); focusSingle() }} onChange={(event) => updateTrack(track.id, { outputBusId: event.target.value || null })}><option value="">MAIN</option>{buses.map((bus) => <option key={bus.id} value={bus.id}>{bus.name}</option>)}</select>
      {menu && <MenuPanel items={contextItems} anchor={menu} onClose={() => setMenu(null)} className="context-menu" />}
    </article>
  )
}

const TrackStrip = memo(TrackStripView)

function BusStripView({ bus }: { bus: Bus }) {
  const update = useProjectStore((state) => state.updateBusVolume)
  const toggleMute = useProjectStore((state) => state.toggleBusMute)
  const selectRack = useProjectStore((state) => state.setRackTarget)
  const selected = useProjectStore((state) => state.rackTarget.kind === 'bus' && state.rackTarget.id === bus.id)
  const engine = useEngine()
  // The graph only knows a strip gain, so mute is applied as an effective gain.
  const applyGain = (value: number, muted = bus.muted) => { update(bus.id, value); engine.setBusVolume(bus.id, muted ? MUTE_GAIN_DB : value) }
  return (
    <article className={`channel-strip bus-strip ${selected ? 'selected' : ''}`} onClick={() => { useProjectStore.getState().selectTrack(null); useProjectStore.setState({ rackTarget: { kind: 'bus', id: bus.id } }) }}>
      <div className="channel-color bus-color" /><div className="channel-title"><span>↪</span><strong>{bus.name}</strong><button className="channel-fx-button" title="이펙트 체인 열기" onClick={(event) => { event.stopPropagation(); useProjectStore.getState().selectTrack(null); selectRack({ kind: 'bus', id: bus.id }) }}>FX</button></div>
      <div className="bus-effects">{bus.effects.length ? bus.effects.map((effect) => <span key={effect.id}>{deviceName(effect.type, effect.plugin)}</span>) : <span className="empty">No effects</span>}</div>
      {/* The engine snapshot carries per-track and master levels only, so a return
          bus shows an inert meter rather than borrowing another strip's level. */}
      <div className="fader-zone"><input className="vertical-fader" style={faderStyle(bus.volumeDb)} aria-valuetext={`${bus.volumeDb.toFixed(1)} dB`} type="range" min="-60" max="12" step="0.1" value={bus.volumeDb} onDoubleClick={() => applyGain(0)} onChange={(event) => applyGain(Number(event.target.value))} /><div className="static-meter" title="리턴 버스 레벨 미터는 네이티브 엔진 지원이 필요합니다" /></div>
      <EditableNumber className="db-readout" value={bus.volumeDb} min={-60} max={12} step={0.1} onChange={(value) => applyGain(value)} format={(value) => `${value.toFixed(1)} dB`} />
      <div className="channel-buttons">
        <button className={bus.muted ? 'active mute' : ''} title="리턴 버스 뮤트" onClick={(event) => { event.stopPropagation(); toggleMute(bus.id); engine.setBusVolume(bus.id, bus.muted ? bus.volumeDb : MUTE_GAIN_DB) }}>M</button>
      </div>
    </article>
  )
}

const BusStrip = memo(BusStripView)

function MasterStrip() {
  const master = useProjectStore((state) => state.project.master)
  const selected = useProjectStore((state) => state.rackTarget.kind === 'master')
  const update = useProjectStore((state) => state.updateMasterVolume)
  const toggleMute = useProjectStore((state) => state.toggleMasterMute)
  const toggleDim = useProjectStore((state) => state.toggleMasterDim)
  const selectRack = useProjectStore((state) => state.setRackTarget)
  const engine = useEngine()
  const applyGain = (value: number) => { update(value); engine.setMasterVolume(effectiveMasterGainDb({ ...master, volumeDb: value })) }
  return (
    <article className={`channel-strip master-strip ${selected ? 'selected' : ''}`} onClick={() => { useProjectStore.getState().selectTrack(null); useProjectStore.setState({ rackTarget: { kind: 'master', id: 'master' } }) }}>
      <div className="channel-color master-color" /><div className="channel-title"><span>∑</span><strong>MASTER</strong><button className="channel-fx-button" title="마스터 이펙트 체인 열기" onClick={(event) => { event.stopPropagation(); useProjectStore.getState().selectTrack(null); selectRack({ kind: 'master', id: 'master' }) }}>FX</button></div>
      <div className="master-label">MAIN OUT<br /><small>1 / 2 · FX {master.effects.length}{master.dim ? ' · DIM -20 dB' : ''}</small></div>
      <div className="fader-zone"><input className="vertical-fader" style={faderStyle(master.volumeDb)} aria-valuetext={`${master.volumeDb.toFixed(1)} dB`} type="range" min="-60" max="12" step="0.1" value={master.volumeDb} onDoubleClick={() => applyGain(0)} onChange={(event) => applyGain(Number(event.target.value))} /><LevelMeter /></div>
      <EditableNumber className="db-readout" value={master.volumeDb} min={-60} max={12} step={0.1} onChange={applyGain} format={(gain) => `${gain.toFixed(1)} dB`} />
      <div className="channel-buttons">
        <button className={master.dim ? 'active dim' : ''} title="모니터 -20 dB 감쇠" onClick={(event) => { event.stopPropagation(); toggleDim(); engine.setMasterVolume(effectiveMasterGainDb({ ...master, dim: !master.dim })) }}>DIM</button>
        <button className={master.muted ? 'active mute' : ''} title="마스터 뮤트" onClick={(event) => { event.stopPropagation(); toggleMute(); engine.setMasterVolume(effectiveMasterGainDb({ ...master, muted: !master.muted })) }}>M</button>
      </div>
    </article>
  )
}

function DeviceRack() {
  const target = useProjectStore((state) => state.rackTarget)
  const tracks = useProjectStore((state) => state.project.tracks)
  const buses = useProjectStore((state) => state.project.buses)
  const master = useProjectStore((state) => state.project.master)
  const addEffect = useProjectStore((state) => state.addTargetEffect)
  const removeEffect = useProjectStore((state) => state.removeTargetEffect)
  const toggleBypass = useProjectStore((state) => state.toggleTargetEffect)
  const reorder = useProjectStore((state) => state.reorderTargetEffect)
  const duplicateEffect = useProjectStore((state) => state.duplicateTargetEffect)
  const focusedEffectId = useProjectStore((state) => state.focusedEffectId)
  const clearFocusedEffect = useProjectStore((state) => state.clearFocusedEffect)
  const [menu, setMenu] = useState<{ x: number; y: number; effect: EffectInstance } | null>(null)
  const [chainDropActive, setChainDropActive] = useState(false)
  const chainRef = useRef<HTMLDivElement>(null)
  useEffect(() => subscribeBrowserDrag((state) => {
    if (state.payload.kind !== 'effect') return
    const inside = state.type !== 'cancel' && !!chainRef.current?.contains(document.elementFromPoint(state.x, state.y))
    setChainDropActive(inside)
    if (state.type === 'drop' && inside) addEffect(target, state.payload.type, state.payload.plugin)
  }), [addEffect, target])
  useEffect(() => {
    if (!focusedEffectId) return
    const frame = window.requestAnimationFrame(() => {
      const card = chainRef.current?.querySelector<HTMLElement>(`[data-effect-id="${focusedEffectId}"]`)
      card?.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'center' })
      card?.focus({ preventScroll: true })
    })
    const clear = window.setTimeout(clearFocusedEffect, 900)
    return () => { window.cancelAnimationFrame(frame); window.clearTimeout(clear) }
  }, [clearFocusedEffect, focusedEffectId])
  const effects = target.kind === 'track' ? tracks.find((item) => item.id === target.id)?.effects : target.kind === 'bus' ? buses.find((item) => item.id === target.id)?.effects : master.effects
  const targetTrack = target.kind === 'track' ? tracks.find((item) => item.id === target.id) : undefined
  if (!effects) return <div className="rack-empty">믹서 채널을 선택하면 디바이스 체인이 표시됩니다.</div>
  const name = target.kind === 'track' ? tracks.find((item) => item.id === target.id)?.name ?? 'Track' : target.kind === 'bus' ? buses.find((item) => item.id === target.id)?.name ?? 'Bus' : 'MASTER FX'
  const color = target.kind === 'track' ? tracks.find((item) => item.id === target.id)?.color ?? '#55a7ff' : target.kind === 'bus' ? '#c7954c' : '#5ac8e8'
  const contextItems: MenuItem[] = menu ? [
    { kind: 'item', label: '바이패스', icon: <Power size={13} />, run: () => toggleBypass(target, menu.effect.id) },
    { kind: 'item', label: '복제', icon: <Copy size={13} />, run: () => duplicateEffect(target, menu.effect.id) },
    { kind: 'separator' },
    { kind: 'item', label: '삭제', icon: <Trash2 size={13} />, danger: true, run: () => target.kind === 'track' ? useProjectStore.getState().removeEffect(target.id, menu.effect.id) : removeEffect(target, menu.effect.id) },
  ] : []
  return (
    <div className="device-rack" onClick={() => setMenu(null)}>
      <div className={`rack-track ${target.kind === 'master' ? 'master-rack-target' : ''}`}>
        <span style={{ background: color }} />
        <strong>{name}</strong>
        <small>{target.kind === 'master' ? 'Master insert chain' : target.kind === 'bus' ? 'Return bus effects' : 'Audio effects'}</small>
        {targetTrack && <div className="rack-track-sends">
          <b>SENDS</b>
          {targetTrack.sends.length ? targetTrack.sends.map((send) => {
            const busName = buses.find((bus) => bus.id === send.targetBusId)?.name ?? 'Unassigned'
            const level = Math.max(0, Math.min(100, (send.gainDb + 60) / 66 * 100))
            return <div key={send.id} title={`${busName} · ${send.gainDb <= -59.9 ? '−∞' : `${send.gainDb.toFixed(1)} dB`}`}><span>{busName}</span><i><em style={{ width: `${level}%` }} /></i><small>{send.gainDb <= -59.9 ? '−∞' : send.gainDb.toFixed(1)}</small></div>
          }) : <em className="rack-no-sends">No sends</em>}
        </div>}
      </div>
      <div ref={chainRef} className={`device-chain ${chainDropActive ? 'rack-drop-active' : ''}`} onWheel={(event) => { if (!event.deltaX && !event.deltaY) return; event.preventDefault(); event.currentTarget.scrollLeft += Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX }}>
        {targetTrack?.kind === 'instrument' && targetTrack.instrument && <InstrumentCard track={targetTrack} />}
        {effects.map((effect, index) => <DeviceCard key={effect.id} effect={effect} target={target} index={index} onReorder={(from, to) => reorder(target, from, to)} onContextMenu={(event) => { event.preventDefault(); setMenu({ x: event.clientX, y: event.clientY, effect }) }} />)}
        <AddDevice onAdd={(type, plugin) => addEffect(target, type, plugin)} />
      </div>
      {menu && <MenuPanel items={contextItems} anchor={{ x: menu.x, y: menu.y }} onClose={() => setMenu(null)} className="context-menu" />}
    </div>
  )
}

function DeviceCard({ effect, target, index, onReorder, onContextMenu }: { effect: EffectInstance; target: RackTarget; index: number; onReorder(from: number, to: number): void; onContextMenu(event: React.MouseEvent): void }) {
  const [collapsed, setCollapsed] = useState(false)
  const [bypassMenu, setBypassMenu] = useState<{ x: number; y: number } | null>(null)
  const update = useProjectStore((state) => state.updateTargetEffect)
  const toggle = useProjectStore((state) => state.toggleTargetEffect)
  const remove = useProjectStore((state) => state.removeTargetEffect)
  const showToast = useProjectStore((state) => state.showToast)
  const focused = useProjectStore((state) => state.focusedEffectId === effect.id)
  const engine = useEngine()
  const setParam = (param: string, value: number) => { update(target, effect.id, { [param]: value }); engine.setEffectParam(effect.id, param, value) }
  const toggleEffect = () => {
    const bypassed = !effect.bypassed
    toggle(target, effect.id)
    engine.setEffectBypass(effect.id, bypassed)
  }
  const automationItems = useCallback((parameterId: string | undefined, label: string, value: number): MenuItem[] => {
    if (target.kind !== 'track') return [{ kind: 'item', label: '오토메이션은 트랙 인서트에서만 지원됩니다.', disabled: true, run: () => showToast('오토메이션 레인은 트랙 이펙트에서 추가할 수 있습니다.') }]
    const track = useProjectStore.getState().project.tracks.find((candidate) => candidate.id === target.id)
    return track ? parameterAutomationMenu(track, 'effect', effect.id, parameterId, label, value) : []
  }, [effect.id, showToast, target])
  const openEditor = () => {
    if (!effect.plugin) return
    void engine.openPluginEditor('effect', effect.id).catch((error) => showToast(`${effect.plugin!.name} 편집기를 열 수 없습니다: ${describeEngineError(error)}`))
  }
  const savePreset = () => {
    void (async () => {
      const state = effect.plugin ? await engine.savePluginState('effect', effect.id) : undefined
      const path = await saveDevicePreset({ format: 'ministudio-device-preset', version: 1, name: deviceName(effect.type, effect.plugin), deviceType: effect.type, pluginUid: effect.plugin?.uid, params: { ...effect.params }, state })
      if (path) showToast(`${deviceName(effect.type, effect.plugin)} 프리셋을 저장했습니다.`)
    })().catch((error) => showToast(`프리셋 저장 실패: ${String(error)}`))
  }
  const loadPreset = () => {
    void (async () => {
      const preset = await openDevicePreset()
      if (!preset) return
      if (preset.deviceType !== effect.type || (effect.plugin && preset.pluginUid !== effect.plugin.uid)) throw new Error('현재 디바이스와 다른 종류의 프리셋입니다.')
      if (effect.plugin && preset.state) await engine.loadPluginState('effect', effect.id, preset.state)
      update(target, effect.id, preset.params)
      for (const [parameter, value] of Object.entries(preset.params)) engine.setEffectParam(effect.id, parameter, value)
      showToast(`${deviceName(effect.type, effect.plugin)} 프리셋을 불러왔습니다.`)
    })().catch((error) => showToast(`프리셋 불러오기 실패: ${String(error)}`))
  }
  const hasSidechain = effect.type === 'builtin:compressor' || (effect.type === 'builtin:vocoder' && Math.round(effect.params.source ?? 3) === 0) || effect.plugin?.supportsSidechain || (effect.plugin?.audioInputBuses ?? 0) > 1
  if (collapsed) return <div className={`device-collapsed ${effect.bypassed ? 'bypassed' : ''}`} data-effect-index={index}><button className="collapsed-grip" onPointerDown={(event) => beginPointerReorder(event, { itemSelector: '[data-effect-index]', indexAttribute: 'data-effect-index', axis: 'horizontal', scrollSelector: '.device-chain', onCommit: onReorder })} title="드래그하여 체인 순서 변경"><GripVertical size={12} /></button><button className="collapsed-open" onClick={() => setCollapsed(false)} title="펼치기"><span>{deviceName(effect.type, effect.plugin)}</span></button></div>
  return (
    // Only the grip starts reordering. Window-level pointer tracking keeps the
    // gesture alive outside this card and avoids Tauri's native file-drag path.
    <ParameterAutomationProvider items={automationItems}>
    <article className={`device-card ${effect.type === 'builtin:multiband-compressor' ? 'multiband-card' : ''} ${effect.type === 'builtin:eq8' ? 'eq8-card' : ''} ${effect.type === 'builtin:mastering-limiter' ? 'limiter-card' : ''} ${effect.type === 'builtin:vocoder' ? 'vocoder-card' : ''} ${effect.type === 'builtin:clipper' ? 'clipper-card' : ''} ${effect.type === 'builtin:upward-compressor' ? 'upward-card' : ''} ${effect.type === 'builtin:roboter' ? 'roboter-card' : ''} ${effect.type === 'builtin:resonator' ? 'colorizer-card' : ''} ${effect.bypassed ? 'bypassed' : ''} ${focused ? 'effect-focused' : ''}`} data-effect-index={index} data-effect-id={effect.id} tabIndex={-1} onContextMenu={onContextMenu}>
      <header className="device-card-header"><div className="device-header-main"><span className="device-grip" onPointerDown={(event) => beginPointerReorder(event, { itemSelector: '[data-effect-index]', indexAttribute: 'data-effect-index', axis: 'horizontal', scrollSelector: '.device-chain', onCommit: onReorder })} title="드래그하여 체인 순서 변경"><GripVertical size={13} /></span><button className={effect.bypassed ? '' : 'powered'} title={effect.bypassed ? '전원 켜기' : '전원 끄기'} onClick={toggleEffect}><CirclePower size={13} /></button><strong title={effect.plugin ? '더블클릭하여 플러그인 창 열기' : undefined} onDoubleClick={(event) => { event.stopPropagation(); if (effect.plugin) openEditor() }}>{deviceName(effect.type, effect.plugin)}</strong><span>{effect.plugin?.format.toUpperCase() ?? effect.type.replace('builtin:', '').toUpperCase()}</span><div className="device-header-actions"><button onClick={() => setCollapsed(true)} title="접기"><Minus size={12} /></button><button className="device-close" onClick={() => remove(target, effect.id)} title="이펙트 제거"><X size={13} /></button></div></div><div className="device-header-sub"><button className={`device-bypass ${effect.bypassed ? 'active' : ''}`} onClick={toggleEffect} onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); setBypassMenu({ x: event.clientX, y: event.clientY }) }}>BYPASS {effect.bypassed ? 'ON' : 'OFF'}</button><button className="device-preset-button" title="프리셋 저장" onClick={savePreset}><Save size={11} /><span>SAVE</span></button><button className="device-preset-button" title="프리셋 불러오기" onClick={loadPreset}><FolderOpen size={11} /><span>LOAD</span></button>{effect.plugin && target.kind === 'track' && <DeviceAutomationModes trackId={target.id} targetKind="effect" targetId={effect.id} />}{hasSidechain && <SidechainControl effect={effect} target={target} />}{effect.plugin && effect.plugin.hasEditor !== false && <button className="device-editor-button" title="플러그인 창 열기" onClick={openEditor}><Piano size={12} /><span>EDIT</span></button>}</div></header>
      <div className="device-body">
        {(effect.type === 'builtin:eq' || effect.type === 'builtin:eq8') && <EqPanel effectId={effect.id} params={effect.params} bypassed={effect.bypassed} bandCount={effect.type === 'builtin:eq8' ? 8 : 4} setParam={setParam} />}
        {effect.type === 'builtin:compressor' && <><div className="parameter-row"><Knob value={effect.params.threshold ?? -18} min={-60} max={0} step={0.1} defaultValue={-18} label="THRESH" format={dbFormat} onChange={(value) => setParam('threshold', value)} /><Knob value={effect.params.ratio ?? 3} min={1} max={20} step={0.1} defaultValue={3} label="RATIO" format={(v) => `${v.toFixed(1)}:1`} onChange={(value) => setParam('ratio', value)} /><Knob value={(effect.params.attack ?? .01) * 1000} min={1} max={500} step={1} defaultValue={10} label="ATTACK" format={msFormat} onChange={(value) => setParam('attack', value / 1000)} /><Knob value={(effect.params.release ?? .2) * 1000} min={10} max={1000} step={1} defaultValue={200} label="RELEASE" format={msFormat} onChange={(value) => setParam('release', value / 1000)} /></div><div className="parameter-row"><Knob value={effect.params.knee ?? 12} min={0} max={24} step={0.5} defaultValue={12} label="KNEE" format={dbFormat} onChange={(value) => setParam('knee', value)} /><Knob value={effect.params.makeupDb ?? 0} min={-12} max={24} step={0.1} defaultValue={0} label="MAKEUP" format={dbFormat} onChange={(value) => setParam('makeupDb', value)} /></div></>}
        {effect.type === 'builtin:multiband-compressor' && <MultibandCompressorPanel effectId={effect.id} params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:utility' && <UtilityPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:distortion' && <DistortionPanel effectId={effect.id} params={effect.params} bypassed={effect.bypassed} setParam={setParam} />}
        {effect.type === 'builtin:disperser' && <DisperserPanel effectId={effect.id} params={effect.params} bypassed={effect.bypassed} setParam={setParam} />}
        {effect.type === 'builtin:mastering-limiter' && <LimiterPanel effectId={effect.id} params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:vocoder' && <VocoderPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:lfo-tremolo' && <TremoloPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:clipper' && <ClipperPanel effectId={effect.id} params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:upward-compressor' && <UpwardCompressorPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:transient-shaper' && <TransientShaperPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:roboter' && <RoboterPanel effectId={effect.id} params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:resonator' && <ColorizerPanel effectId={effect.id} params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:formant-shifter' && <FormantShifterPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:delay' && <><div className="delay-display"><i /><i /><i /><i /><i /></div><div className="parameter-row"><Knob value={effect.params.time ?? .25} min={.01} max={2} step={.01} defaultValue={.25} label="TIME" format={(v) => `${v.toFixed(2)} s`} onChange={(value) => setParam('time', value)} /><Knob value={effect.params.feedback ?? .3} min={0} max={.95} step={.01} defaultValue={.3} label="FEEDBACK" format={percentFormat} onChange={(value) => setParam('feedback', value)} /><Knob value={effect.params.damping ?? .35} min={.01} max={1} step={.01} defaultValue={.35} label="DAMPING" format={percentFormat} onChange={(value) => setParam('damping', value)} /><Knob value={effect.params.mix ?? .25} min={0} max={1} step={.01} defaultValue={.25} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div><ToggleRow label="PING PONG" on={(effect.params.pingPong ?? 0) >= 0.5} onToggle={(on) => setParam('pingPong', on ? 1 : 0)} /></>}
        {effect.type === 'builtin:reverb' && <><div className="reverb-display"><span /><span /><span /><span /></div><div className="parameter-row"><Knob value={effect.params.decaySec ?? 2.4} min={.1} max={20} step={.1} defaultValue={2.4} label="DECAY" format={(v) => `${v.toFixed(1)} s`} onChange={(value) => setParam('decaySec', value)} /><Knob value={effect.params.damping ?? .4} min={0} max={1} step={.01} defaultValue={.4} label="DAMPING" format={percentFormat} onChange={(value) => setParam('damping', value)} /><Knob value={effect.params.width ?? .8} min={0} max={1} step={.01} defaultValue={.8} label="WIDTH" format={percentFormat} onChange={(value) => setParam('width', value)} /><Knob value={effect.params.diffusion ?? .7} min={0} max={.92} step={.01} defaultValue={.7} label="DIFFUSE" format={percentFormat} onChange={(value) => setParam('diffusion', value)} /><Knob value={effect.params.mix ?? .25} min={0} max={1} step={.01} defaultValue={.25} label="DRY / WET" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div></>}
        {effect.type === 'builtin:waveshaper' && <><ShaperDisplay params={effect.params} bypassed={effect.bypassed} onCurve={(curve) => setParam('curve', curve)} /><div className="parameter-row shaper-controls"><Knob value={effect.params.driveDb ?? 6} min={0} max={36} step={.1} defaultValue={6} label="DRIVE" format={dbFormat} onChange={(value) => setParam('driveDb', value)} /><Knob value={effect.params.mix ?? 1} min={0} max={1} step={.01} defaultValue={1} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div><div className="shaper-quality">4× OVERSAMPLING <span>·</span> DC FILTER</div></>}
        {effect.plugin && <ExternalPluginParameters plugin={effect.plugin} values={effect.params} setParam={setParam} />}
      </div>
      {bypassMenu && <MenuPanel items={automationItems('__bypass', '바이패스', effect.bypassed ? 1 : 0)} anchor={bypassMenu} onClose={() => setBypassMenu(null)} className="parameter-context-menu" />}
    </article>
    </ParameterAutomationProvider>
  )
}

function ToggleRow({ label, on, onToggle }: { label: string; on: boolean; onToggle(on: boolean): void }) {
  return <div className="toggle-row"><button className={on ? 'active' : ''} role="switch" aria-checked={on} onClick={() => onToggle(!on)}><i />{label}</button></div>
}

function ExternalPluginParameters({ plugin, values, setParam }: { plugin: ExternalPluginRef; values: Record<string, number>; setParam(id: string, value: number): void }) {
  const parameters = plugin.parameters ?? []
  const visible = rackParameters(parameters)
  if (!visible.length) return <PluginIdentitySurface plugin={plugin} />
  return <div className="external-instrument-editor"><div className="external-instrument-heading"><strong>{plugin.name}</strong><span>{parameters.length < 12 ? 'ALL PARAMETERS' : `${visible.length} MACROS`}</span></div><PluginParameterGrid parameters={visible} values={values} setParam={setParam} /></div>
}

function PluginParameterGrid({ parameters, values, setParam }: { parameters: NonNullable<ExternalPluginRef['parameters']>; values: Record<string, number>; setParam(id: string, value: number): void }) {
  return <div className="external-parameter-grid macro-grid">{parameters.map((parameter) => {
    const min = Number.isFinite(parameter.min) ? parameter.min : 0
    const max = Number.isFinite(parameter.max) && parameter.max > min ? parameter.max : 1
    const value = values[parameter.id] ?? parameter.defaultValue
    const step = Math.max(.0001, (max - min) / 1000)
    const binary = min === 0 && max === 1 && /(?:on|off|enable|bypass|mute|solo|sync)$/i.test(parameter.name.trim())
    return binary
      ? <AutomationButton key={parameter.id} parameterId={`param:${parameter.id}`} label={parameter.name} value={value} className={`external-param-toggle ${value >= .5 ? 'active' : ''}`} title={`${parameter.module} · ${parameter.id}`} onClick={() => setParam(parameter.id, value >= .5 ? 0 : 1)}><i /><span>{parameter.name}</span></AutomationButton>
      : <Knob key={parameter.id} parameterId={`param:${parameter.id}`} value={value} min={min} max={max} step={step} defaultValue={parameter.defaultValue} label={parameter.name} format={(next) => next.toFixed(Math.abs(next) < 10 ? 3 : 1)} onChange={(next) => setParam(parameter.id, next)} />
  })}</div>
}

function PluginIdentitySurface({ plugin }: { plugin: ExternalPluginRef }) {
  return <div className="plugin-identity-surface"><div className="plugin-identity-mark"><i /><i /><i /><i /><span>{plugin.format.toUpperCase()}</span></div><strong>{plugin.name}</strong><small>{plugin.vendor || 'EXTERNAL DEVICE'}</small></div>
}

function TransientShaperPanel({ params, setParam }: { params: Record<string, number>; setParam(param: string, value: number): void }) {
  const attack = params.attack ?? 0
  const sustain = params.sustain ?? 0
  const threshold = params.thresholdDb ?? -36
  const speed = params.speed ?? .5
  const updatePad = (event: React.PointerEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect()
    setParam('thresholdDb', Math.max(-72, Math.min(0, -72 + (event.clientX - bounds.left) / Math.max(1, bounds.width) * 72)))
    setParam('speed', Math.max(0, Math.min(1, 1 - (event.clientY - bounds.top) / Math.max(1, bounds.height))))
  }
  const down = (event: React.PointerEvent<HTMLDivElement>) => { event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); updatePad(event) }
  const up = (event: React.PointerEvent<HTMLDivElement>) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId) }
  const path = `M 4 58 C 24 58, 33 ${58 - attack * 22}, 48 ${58 - attack * 30} C 60 ${58 + attack * 7}, 76 ${58 - sustain * 16}, 112 ${58 - sustain * 16}`
  return <div className="transient-panel">
    <div className="transient-display"><svg viewBox="0 0 116 76" preserveAspectRatio="none"><line x1="4" y1="58" x2="112" y2="58" /><path d={path} /></svg><span>ENVELOPE CONTOUR</span></div>
    <div className="transient-main-controls">
      <Knob parameterId="attack" value={attack} min={-1} max={1} step={.01} defaultValue={0} label="ATTACK" format={(value) => `${value >= 0 ? '+' : ''}${Math.round(value * 100)}%`} onChange={(value) => setParam('attack', value)} />
      <Knob parameterId="sustain" value={sustain} min={-1} max={1} step={.01} defaultValue={0} label="SUSTAIN" format={(value) => `${value >= 0 ? '+' : ''}${Math.round(value * 100)}%`} onChange={(value) => setParam('sustain', value)} />
    </div>
    <div className="transient-pad-wrap"><b>DETECTOR</b><div className="transient-pad" role="application" aria-label="Threshold and speed XY control" onPointerDown={down} onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) updatePad(event) }} onPointerUp={up} onPointerCancel={up}><i style={{ left: `${(threshold + 72) / 72 * 100}%`, top: `${(1 - speed) * 100}%` }} /></div><div><span>THRESH {threshold.toFixed(1)} dB</span><span>SPEED {Math.round(speed * 100)}%</span></div></div>
    <AutomationButton parameterId="clip" label="Clip" value={params.clip ?? 0} className={`transient-clip ${(params.clip ?? 0) >= .5 ? 'active' : ''}`} onClick={() => setParam('clip', (params.clip ?? 0) >= .5 ? 0 : 1)}><CirclePower size={13} /> CLIP</AutomationButton>
  </div>
}

const LIMITER_MODES = [
  { name: 'CLEAN', detail: 'true-peak lookahead' },
  { name: 'TRANSPARENT', detail: 'long adaptive release' },
  { name: 'PUNCH', detail: 'fast recovery' },
  { name: 'LOUD', detail: 'soft-clip + limiting' },
] as const

function LimiterPanel({ effectId, params, setParam }: { effectId: string; params: Record<string, number>; setParam(param: string, value: number): void }) {
  const engine = useEngine()
  const [meters, setMeters] = useState(() => engine.getLimiterMetrics(effectId))
  useEffect(() => subscribeMeterFrame(() => setMeters({ ...engine.getLimiterMetrics(effectId) })), [effectId, engine])
  const mode = Math.max(0, Math.min(3, Math.round(params.algorithm ?? 0)))
  return <div className="limiter-panel">
    <div className="limiter-mode-tabs">{LIMITER_MODES.map((item, index) => <button key={item.name} className={mode === index ? 'active' : ''} onClick={() => setParam('algorithm', index)}><strong>{item.name}</strong><small>{item.detail}</small></button>)}</div>
    <div className="limiter-main">
      <div className="limiter-controls"><Knob value={params.inputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="INPUT" format={dbFormat} onChange={(value) => setParam('inputDb', value)} /><Knob value={params.outputDb ?? -1} min={-24} max={0} step={.1} defaultValue={-1} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} /><Knob value={params.releaseMs ?? 120} min={5} max={2000} step={1} scale="log" defaultValue={120} label="RELEASE" format={msFormat} onChange={(value) => setParam('releaseMs', value)} /><Knob value={params.stereoLink ?? 1} min={0} max={1} step={.01} defaultValue={1} label="STEREO LINK" format={percentFormat} onChange={(value) => setParam('stereoLink', value)} /></div>
      <div className="limiter-meters"><LimiterBar label="IN" value={meters.inputPeakDb} min={-60} max={0} /><LimiterBar label="GR" value={-meters.gainReductionDb} min={-24} max={0} reduction /><LimiterBar label="OUT" value={meters.outputPeakDb} min={-60} max={0} /><div className="limiter-readouts"><span>TRUE PEAK <b>{meterFormat(meters.truePeakDb, 'dBTP')}</b></span><span>MOMENTARY <b>{meterFormat(meters.momentaryLufs, 'LUFS')}</b></span><span>SHORT <b>{meterFormat(meters.shortTermLufs, 'LUFS')}</b></span><span>INTEGRATED <b>{meterFormat(meters.integratedLufs, 'LUFS')}</b></span></div></div>
    </div>
    <ToggleRow label="TRUE PEAK · 4×" on={(params.truePeak ?? 1) >= .5} onToggle={(on) => setParam('truePeak', on ? 1 : 0)} />
  </div>
}

function LimiterBar({ label, value, min, max, reduction = false }: { label: string; value: number; min: number; max: number; reduction?: boolean }) {
  const normalized = reduction ? Math.max(0, Math.min(1, -value / -min)) : Math.max(0, Math.min(1, (value - min) / (max - min)))
  return <div className={`limiter-bar ${reduction ? 'reduction' : ''}`}><span>{label}</span><i><b style={{ transform: `scaleY(${normalized})` }} /></i><output>{value.toFixed(1)}</output></div>
}

function VocoderPanel({ params, setParam }: { params: Record<string, number>; setParam(param: string, value: number): void }) {
  const source = Math.max(0, Math.min(4, Math.round(params.source ?? 3)))
  return <div className="vocoder-panel">
    <div className="vocoder-display"><span>MODULATOR</span><strong>{['SIDECHAIN', 'SINE', 'SQUARE', 'SAW', 'NOISE'][source]}</strong><div>{Array.from({ length: 16 }, (_, index) => <i key={index} style={{ height: `${24 + ((index * 37) % 62)}%` }} />)}</div></div>
    <div className="select-row"><label>SOURCE<select value={source} onChange={(event) => setParam('source', Number(event.target.value))}><option value="0">SIDECHAIN</option><option value="1">SINE</option><option value="2">SQUARE</option><option value="3">SAW</option><option value="4">NOISE</option></select></label><label>BANDS<select value={params.bands ?? 16} onChange={(event) => setParam('bands', Number(event.target.value))}><option value="8">8</option><option value="12">12</option><option value="16">16</option><option value="24">24</option></select></label></div>
    <div className="parameter-row vocoder-controls"><Knob value={params.pitchHz ?? 110} min={20} max={2000} step={1} scale="log" defaultValue={110} label="PITCH" format={hzFormat} onChange={(value) => setParam('pitchHz', value)} /><Knob value={params.attackMs ?? 5} min={.1} max={200} step={.1} scale="log" defaultValue={5} label="ATTACK" format={msFormat} onChange={(value) => setParam('attackMs', value)} /><Knob value={params.releaseMs ?? 90} min={5} max={2000} step={1} scale="log" defaultValue={90} label="RELEASE" format={msFormat} onChange={(value) => setParam('releaseMs', value)} /><Knob value={params.formantShift ?? 0} min={-24} max={24} step={1} defaultValue={0} label="FORMANT" format={(value) => `${value > 0 ? '+' : ''}${Math.round(value)} st`} onChange={(value) => setParam('formantShift', value)} /><Knob value={params.bandwidth ?? 1} min={.5} max={2} step={.01} defaultValue={1} label="BANDWIDTH" format={(value) => `${value.toFixed(2)}×`} onChange={(value) => setParam('bandwidth', value)} /><Knob value={params.mix ?? 1} min={0} max={1} step={.01} defaultValue={1} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /><Knob value={params.outputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} /></div>
  </div>
}

function TremoloPanel({ params, setParam }: { params: Record<string, number>; setParam(param: string, value: number): void }) {
  const waveform = Math.max(0, Math.min(3, Math.round(params.waveform ?? 0)))
  const points = Array.from({ length: 65 }, (_, index) => { const phase = index / 64; return `${phase * 220},${48 - lfoPreview(waveform, phase) * 34}` }).join(' ')
  return <div className="tremolo-panel"><svg className="tremolo-display" viewBox="0 0 220 96" preserveAspectRatio="none"><line x1="0" y1="48" x2="220" y2="48" /><polyline points={points} /></svg><div className="select-row"><label>WAVE<select value={waveform} onChange={(event) => setParam('waveform', Number(event.target.value))}><option value="0">SINE</option><option value="1">TRIANGLE</option><option value="2">SQUARE</option><option value="3">SAW</option></select></label></div><div className="parameter-row"><Knob value={params.rateHz ?? 4} min={.01} max={30} step={.01} scale="log" defaultValue={4} label="RATE" format={(value) => `${value.toFixed(2)} Hz`} onChange={(value) => setParam('rateHz', value)} /><Knob value={params.volumeDepth ?? .5} min={0} max={1} step={.01} defaultValue={.5} label="VOLUME" format={percentFormat} onChange={(value) => setParam('volumeDepth', value)} /><Knob value={params.panDepth ?? 0} min={0} max={1} step={.01} defaultValue={0} label="PAN" format={percentFormat} onChange={(value) => setParam('panDepth', value)} /><Knob value={(params.stereoPhase ?? .25) * 360} min={0} max={360} step={1} defaultValue={90} label="PHASE" format={(value) => `${Math.round(value)}°`} onChange={(value) => setParam('stereoPhase', value / 360)} /></div></div>
}

function ClipperPanel({ effectId, params, setParam }: { effectId: string; params: Record<string, number>; setParam(param: string, value: number): void }) {
  return <div className="clipper-panel">
    <ClipperFlowDisplay effectId={effectId} />
    <div className="clipper-status"><span><i /> REALTIME PEAK FLOW</span><b>4× OVERSAMPLING</b></div>
    <div className="parameter-row clipper-controls"><Knob value={params.inputDb ?? 6} min={-24} max={36} step={.1} defaultValue={6} label="IN GAIN" format={dbFormat} onChange={(value) => setParam('inputDb', value)} /><Knob value={params.knee ?? .25} min={0} max={1} step={.01} defaultValue={.25} label="KNEE" format={percentFormat} onChange={(value) => setParam('knee', value)} /><Knob value={params.outputDb ?? -1} min={-24} max={0} step={.1} defaultValue={-1} label="OUT GAIN" format={dbFormat} onChange={(value) => setParam('outputDb', value)} /></div>
  </div>
}

function ClipperFlowDisplay({ effectId }: { effectId: string }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const engine = useEngine()
  const draw = useCallback(() => {
    const canvas = ref.current; const context = canvas?.getContext('2d'); if (!canvas || !context) return
    const { width, height } = canvas; const center = height * .5; const limit = height * .31; const history = engine.getEffectSpectrum(effectId)
    const background = context.createLinearGradient(0, 0, 0, height); background.addColorStop(0, '#141c25'); background.addColorStop(.5, '#090f14'); background.addColorStop(1, '#141c25'); context.fillStyle = background; context.fillRect(0, 0, width, height)
    context.strokeStyle = '#26343e'; context.lineWidth = 1
    for (let x = 0; x <= width; x += width / 8) { context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.stroke() }
    context.fillStyle = '#f0675b13'; context.fillRect(0, 0, width, center - limit); context.fillRect(0, center + limit, width, center - limit)
    const xAt = (index: number) => index / Math.max(1, history.length - 1) * width
    const amplitudeAt = (value: number) => Math.min(center - 5, Math.max(1.5, value * limit))
    context.beginPath(); context.moveTo(0, center)
    history.forEach((value, index) => context.lineTo(xAt(index), center - amplitudeAt(value)))
    for (let index = history.length - 1; index >= 0; index -= 1) context.lineTo(xAt(index), center + amplitudeAt(history[index] ?? 0))
    context.closePath(); const fill = context.createLinearGradient(0, 0, 0, height); fill.addColorStop(0, '#ff725d99'); fill.addColorStop(.22, '#55b9ffcc'); fill.addColorStop(.5, '#1b73b866'); fill.addColorStop(.78, '#55b9ffcc'); fill.addColorStop(1, '#ff725d99'); context.fillStyle = fill; context.fill()
    context.beginPath(); history.forEach((value, index) => { const x = xAt(index); const y = center - amplitudeAt(value); if (index === 0) context.moveTo(x, y); else context.lineTo(x, y) }); context.strokeStyle = '#77c9ff'; context.lineWidth = 1.5; context.shadowColor = '#44aef7'; context.shadowBlur = 5; context.stroke(); context.shadowBlur = 0
    context.strokeStyle = '#d9edf7'; context.lineWidth = 1; context.setLineDash([5, 4]); for (const y of [center - limit, center + limit]) { context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke() } context.setLineDash([])
    context.fillStyle = '#8da2af'; context.font = '7px monospace'; context.textAlign = 'right'; context.fillText('0 dBFS', width - 6, center - limit - 4)
  }, [effectId, engine])
  useEffect(() => { draw(); return subscribeAnalyzerFrame(draw) }, [draw])
  return <canvas ref={ref} className="clipper-flow" width="340" height="116" />
}

function UpwardCompressorPanel({ params, setParam }: { params: Record<string, number>; setParam(param: string, value: number): void }) {
  const threshold = params.threshold ?? -32; const ratio = params.ratio ?? 3; const range = params.rangeDb ?? 12
  const points = Array.from({ length: 73 }, (_, index) => { const input = -72 + index; const boost = input < threshold ? Math.min(range, (threshold - input) * (1 - 1 / ratio)) : 0; return `${index / 72 * 286},${94 - (input + boost + 72) / 72 * 84}` }).join(' ')
  const thresholdX = (threshold + 72) / 72 * 286
  return <div className="upward-panel">
    <div className="upward-graph"><svg viewBox="0 0 286 104" preserveAspectRatio="none"><defs><linearGradient id="upwardArea" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#6de6c0" stopOpacity=".42" /><stop offset="1" stopColor="#328c7d" stopOpacity=".03" /></linearGradient></defs><path className="unity" d="M0 94 L286 10" /><path className="area" d={`M0 94 L${points.replaceAll(' ', ' L')} L286 94 Z`} /><polyline points={points} /><line className="threshold" x1={thresholdX} x2={thresholdX} y1="7" y2="97" /><circle cx={thresholdX} cy={94 - (threshold + 72) / 72 * 84} r="3.5" /></svg><span>UPWARD RANGE</span><b>{dbFormat(range)}</b><small>{dbFormat(threshold)} THRESHOLD</small></div>
    <div className="upward-controls parameter-row"><Knob value={threshold} min={-72} max={-6} step={.1} defaultValue={-32} label="THRESH" format={dbFormat} onChange={(value) => setParam('threshold', value)} /><Knob value={ratio} min={1} max={20} step={.1} defaultValue={3} label="RATIO" format={(value) => `${value.toFixed(1)}:1`} onChange={(value) => setParam('ratio', value)} /><Knob value={params.attackMs ?? 35} min={.1} max={500} step={.1} scale="log" defaultValue={35} label="ATTACK" format={msFormat} onChange={(value) => setParam('attackMs', value)} /><Knob value={params.releaseMs ?? 240} min={5} max={2000} step={1} scale="log" defaultValue={240} label="RELEASE" format={msFormat} onChange={(value) => setParam('releaseMs', value)} /><Knob value={range} min={0} max={36} step={.1} defaultValue={12} label="RANGE" format={dbFormat} onChange={(value) => setParam('rangeDb', value)} /><Knob value={params.stereoLink ?? 1} min={0} max={1} step={.01} defaultValue={1} label="LINK" format={percentFormat} onChange={(value) => setParam('stereoLink', value)} /><Knob value={params.mix ?? 1} min={0} max={1} step={.01} defaultValue={1} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /><Knob value={params.outputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} /></div>
  </div>
}

const semitoneFormat = (value: number) => `${value > 0 ? '+' : ''}${value.toFixed(1)} st`

const FORMANT_MODE_LABELS = ['AUTO', 'MONO · PSOLA-STYLE', 'POLY · PHASE VOCODER']

function FormantShifterPanel({ params, setParam }: { params: Record<string, number>; setParam(param: string, value: number): void }) {
  const pitch = params.pitchSemitones ?? 0
  const link = (params.formantLink ?? 1) >= 0.5
  const formant = link ? pitch : (params.formantSemitones ?? 0)
  const mode = Math.round(params.mode ?? 0)
  return <div className="formant-panel">
    <div className="formant-graph"><svg viewBox="0 0 286 104" preserveAspectRatio="none">
      <line className="unity" x1="143" x2="143" y1="6" y2="98" />
      <line className="unity" y1="52" x1="0" y2="52" x2="286" />
      <circle className="formant-dry" cx="143" cy="52" r="4" />
      <line className="formant-vector" x1="143" y1="52" x2={143 + pitch * 4.6} y2={52 - formant * 3.6} />
      <circle className="formant-dot" cx={143 + pitch * 4.6} cy={52 - formant * 3.6} r="5" />
    </svg><span>{FORMANT_MODE_LABELS[mode]}</span><b>{semitoneFormat(pitch)}</b><small>{link ? 'FORMANT LINKED TO PITCH' : `${semitoneFormat(formant)} FORMANT`}</small></div>
    <div className="formant-controls">
      <div className="formant-mode-row">
        <select value={mode} title="AUTO: 주기성을 감지해 단선율엔 가벼운 Mono, 화음·믹스엔 Poly를 자동 선택" onChange={(event) => setParam('mode', Number(event.target.value))}>
          <option value={0}>AUTO</option>
          <option value={1}>MONO</option>
          <option value={2}>POLY</option>
        </select>
        <ToggleRow label="LINK" on={link} onToggle={(value) => setParam('formantLink', value ? 1 : 0)} />
      </div>
      <div className="parameter-row">
        <Knob value={pitch} min={-24} max={24} step={.1} defaultValue={0} label="PITCH" format={semitoneFormat} onChange={(value) => setParam('pitchSemitones', value)} />
        <div className={link ? 'formant-knob-linked' : ''}><Knob value={formant} min={-24} max={24} step={.1} defaultValue={0} label="FORMANT" format={semitoneFormat} onChange={(value) => setParam('formantSemitones', value)} /></div>
        <Knob value={params.mix ?? 1} min={0} max={1} step={.01} defaultValue={1} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} />
        <Knob value={params.outputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} />
      </div>
    </div>
  </div>
}

function RoboterPanel({ effectId, params, setParam }: { effectId: string; params: Record<string, number>; setParam(param: string, value: number): void }) {
  const engine = useEngine()
  const [metrics, setMetrics] = useState(() => [...engine.getEffectSpectrum(effectId)])
  useEffect(() => subscribeMeterFrame(() => setMetrics([...engine.getEffectSpectrum(effectId)])), [effectId, engine])
  const amount = Math.max(0, Math.min(1, params.amount ?? .72)); const number = Math.max(0, Math.min(5, Math.round(params.number ?? 0)))
  const pitch = Math.round(metrics[0] ?? -1); const pitchClass = pitch >= 0 ? ((pitch % 12) + 12) % 12 : -1
  const root = Math.max(0, Math.min(11, Math.round(metrics[1] ?? 0))); const mode = Math.round(metrics[2] ?? 2); const confidence = Math.max(0, Math.min(1, metrics[3] ?? 0))
  const noteNames = ['C', 'C♯', 'D', 'E♭', 'E', 'F', 'F♯', 'G', 'A♭', 'A', 'B♭', 'B']
  const keyName = mode === 2 ? 'Chromatic' : `${noteNames[root]} ${mode === 1 ? 'minor' : 'major'}`
  const activeRoles = ROBOTER_ACTIVE_ROLES[number] ?? []
  const intervals = activeRoles.map((role) => Math.round(metrics[7 + role] ?? ROBOTER_FALLBACK_INTERVALS[role] ?? 0)).map((interval) => `${interval > 0 ? '+' : ''}${interval} st`)
  return <div className="roboter-panel"><div className="roboter-display"><div className="robot-note-ring">{noteNames.map((note, index) => <i key={note} className={index === pitchClass ? 'active' : index === root && mode !== 2 ? 'tonic' : ''} style={{ '--note-index': index } as React.CSSProperties}>{note}</i>)}<span className="robot-core"><b>{pitchClass >= 0 ? noteNames[pitchClass] : 'R'}</b><small>{pitchClass >= 0 ? 'TRACKING' : 'LISTENING'}</small></span></div></div><div className="roboter-controls"><Knob value={amount} min={0} max={1} step={.01} defaultValue={.72} label="AMOUNT" format={percentFormat} onChange={(value) => setParam('amount', value)} /><Knob value={number} min={0} max={5} step={1} defaultValue={0} label="NUMBER" format={(value) => `${Math.round(value)}`} onChange={(value) => setParam('number', Math.round(value))} /><div className="roboter-key"><span>DETECTED KEY</span><strong>{keyName}</strong><small>{mode === 2 ? 'fixed-interval fallback' : `${Math.round(confidence * 100)}% confidence`}</small></div></div><footer><span>{intervals.length ? intervals.join('  ·  ') : 'LEAD ONLY'}</span><b>{number ? `${number} HARMON${number === 1 ? 'Y' : 'IES'}` : 'MONO LEAD'}</b></footer></div>
}

const COLORIZER_NOTES = ['C', 'C♯', 'D', 'E♭', 'E', 'F', 'F♯', 'G', 'A♭', 'A', 'B♭', 'B'] as const
const COLORIZER_SCALES = [
  ['Major', [0, 2, 4, 5, 7, 9, 11]], ['Natural Minor', [0, 2, 3, 5, 7, 8, 10]], ['Harmonic Minor', [0, 2, 3, 5, 7, 8, 11]], ['Melodic Minor', [0, 2, 3, 5, 7, 9, 11]],
  ['Dorian', [0, 2, 3, 5, 7, 9, 10]], ['Phrygian', [0, 1, 3, 5, 7, 8, 10]], ['Lydian', [0, 2, 4, 6, 7, 9, 11]], ['Mixolydian', [0, 2, 4, 5, 7, 9, 10]], ['Locrian', [0, 1, 3, 5, 6, 8, 10]],
  ['Major Pentatonic', [0, 2, 4, 7, 9]], ['Minor Pentatonic', [0, 3, 5, 7, 10]], ['Blues', [0, 3, 5, 6, 7, 10]], ['Whole Tone', [0, 2, 4, 6, 8, 10]], ['Chromatic', [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]],
] as const
const COLORIZER_CUSTOM_SCALE = COLORIZER_SCALES.length

function ColorizerPanel({ effectId, params, setParam }: { effectId: string; params: Record<string, number>; setParam(param: string, value: number): void }) {
  const engine = useEngine(); const canvasRef = useRef<HTMLCanvasElement>(null)
  const midi = (params.midi ?? 0) >= .5; const key = Math.max(0, Math.min(11, Math.round(params.key ?? 0))); const scale = Math.max(0, Math.min(COLORIZER_CUSTOM_SCALE, Math.round(params.scale ?? 0)))
  const applyPreset = (nextKey: number, nextScale: number) => {
    setParam('key', nextKey); setParam('scale', nextScale)
    const preset = COLORIZER_SCALES[nextScale]
    if (!preset) return
    const enabled = new Set<number>(preset[1].map((interval) => (interval + nextKey) % 12))
    for (let pitch = 0; pitch < 12; pitch += 1) setParam(`pitch${pitch}`, enabled.has(pitch) ? 1 : 0)
  }
  useEffect(() => {
    const draw = () => drawColorizerSpectrum(canvasRef.current, engine.getEffectSpectrum(effectId))
    draw(); return subscribeAnalyzerFrame(draw)
  }, [effectId, engine])
  return <div className="colorizer-panel">
    <div className="colorizer-spectrum"><canvas ref={canvasRef} /><span>40</span><span>120</span><span>500</span><span>2k</span><span>12k</span><b>HARMONIC MASK</b></div>
    <div className="colorizer-source">
      <div className="colorizer-selects"><label>KEY<select value={key} disabled={midi} onChange={(event) => applyPreset(Number(event.target.value), scale)}>{COLORIZER_NOTES.map((note, index) => <option key={note} value={index}>{note}</option>)}</select></label><label>SCALE<select value={scale} disabled={midi} onChange={(event) => applyPreset(key, Number(event.target.value))}>{COLORIZER_SCALES.map(([name], index) => <option key={name} value={index}>{name}</option>)}<option value={COLORIZER_CUSTOM_SCALE}>Custom</option></select></label><button className={midi ? 'active' : ''} onClick={() => setParam('midi', midi ? 0 : 1)}>MIDI</button></div>
      <PitchClassKeyboard disabled={midi} active={COLORIZER_NOTES.map((_, pitch) => (params[`pitch${pitch}`] ?? 0) >= .5)} onToggle={(pitch) => { setParam(`pitch${pitch}`, (params[`pitch${pitch}`] ?? 0) >= .5 ? 0 : 1); setParam('scale', COLORIZER_CUSTOM_SCALE) }} />
      {midi && <div className="colorizer-midi-note">MIDI ROUTING NOT AVAILABLE YET · WAITING FOR NOTES</div>}
    </div>
    <div className="colorizer-controls parameter-row"><Knob value={params.resonance ?? .62} min={0} max={1} step={.01} defaultValue={.62} label="RESONANCE" format={percentFormat} onChange={(value) => setParam('resonance', value)} /><Knob value={params.decay ?? .45} min={0} max={1} step={.01} defaultValue={.45} label="DECAY" format={percentFormat} onChange={(value) => setParam('decay', value)} /><Knob value={params.depth ?? .82} min={0} max={1} step={.01} defaultValue={.82} label="DEPTH" format={percentFormat} onChange={(value) => setParam('depth', value)} /><Knob value={params.mix ?? .72} min={0} max={1} step={.01} defaultValue={.72} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div>
  </div>
}

function PitchClassKeyboard({ active, disabled, onToggle }: { active: boolean[]; disabled?: boolean; onToggle(pitch: number): void }) {
  const whites = [0, 2, 4, 5, 7, 9, 11]; const blacks = [[1, 10.7], [3, 25], [6, 53.6], [8, 67.9], [10, 82.1]] as const
  return <div className={`pitch-class-keyboard ${disabled ? 'disabled' : ''}`}>{whites.map((pitch, index) => <button key={pitch} className={`white ${active[pitch] ? 'active' : ''}`} style={{ left: `${index * (100 / 7)}%`, width: `${100 / 7}%` }} disabled={disabled} aria-pressed={active[pitch]} title={COLORIZER_NOTES[pitch]} onClick={() => onToggle(pitch)}><span>{COLORIZER_NOTES[pitch]}</span></button>)}{blacks.map(([pitch, left]) => <button key={pitch} className={`black ${active[pitch] ? 'active' : ''}`} style={{ left: `${left}%` }} disabled={disabled} aria-pressed={active[pitch]} title={COLORIZER_NOTES[pitch]} onClick={() => onToggle(pitch)}><span>{COLORIZER_NOTES[pitch]}</span></button>)}</div>
}

function drawColorizerSpectrum(canvas: HTMLCanvasElement | null, values: readonly number[]): void {
  if (!canvas) return
  const ratio = window.devicePixelRatio || 1; const width = Math.max(1, Math.round(canvas.clientWidth * ratio)); const height = Math.max(1, Math.round(canvas.clientHeight * ratio))
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height }
  const context = canvas.getContext('2d'); if (!context) return
  context.clearRect(0, 0, width, height); context.lineWidth = ratio
  context.strokeStyle = '#29404b'; context.globalAlpha = .75
  for (let row = 1; row < 4; row += 1) { const y = height * row / 4; context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke() }
  for (let column = 1; column < 6; column += 1) { const x = width * column / 6; context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.stroke() }
  context.globalAlpha = 1; const count = 24
  context.beginPath(); context.moveTo(0, height)
  for (let index = 0; index < count; index += 1) { const x = index / (count - 1) * width; const y = height * (1 - Math.min(1, Math.max(0, values[index] ?? 0))); context.lineTo(x, y) }
  context.lineTo(width, height); context.closePath(); const fill = context.createLinearGradient(0, 0, 0, height); fill.addColorStop(0, '#76e3ff80'); fill.addColorStop(1, '#327f9b08'); context.fillStyle = fill; context.fill()
  context.beginPath()
  for (let index = 0; index < count; index += 1) { const x = index / (count - 1) * width; const y = height * (1 - Math.min(1, Math.max(0, values[count + index] ?? 0)) * .88); if (index) context.lineTo(x, y); else context.moveTo(x, y) }
  context.strokeStyle = '#ffd36a'; context.lineWidth = 1.6 * ratio; context.shadowColor = '#ffbd3b'; context.shadowBlur = 5 * ratio; context.stroke(); context.shadowBlur = 0
}

const ROBOTER_ACTIVE_ROLES = [[], [0], [1, 2], [0, 1, 2], [1, 2, 3, 4], [0, 1, 2, 3, 4]]
const ROBOTER_FALLBACK_INTERVALS = [-12, -4, 4, -9, 7]

function lfoPreview(waveform: number, phase: number): number { if (waveform === 1) return 1 - 4 * Math.abs(phase - .5); if (waveform === 2) return phase < .5 ? 1 : -1; if (waveform === 3) return phase * 2 - 1; return Math.sin(Math.PI * 2 * phase) }
function meterFormat(value: number, unit: string): string { return `${value <= -119 ? '−∞' : value.toFixed(1)} ${unit}` }

function SidechainControl({ effect, target }: { effect: EffectInstance; target: RackTarget }) {
  const [open, setOpen] = useState(false)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const tracks = useProjectStore((state) => state.project.tracks)
  const buses = useProjectStore((state) => state.project.buses)
  const setSidechain = useProjectStore((state) => state.setTargetEffectSidechain)
  const getAnchorElement = useCallback(() => buttonRef.current, [])
  const sidechain = effect.sidechain ?? { enabled: false, sourceTrackId: null }
  return <div className="sidechain-control"><button ref={buttonRef} className={sidechain.enabled ? 'active' : ''} onClick={() => setOpen(!open)}>SIDECHAIN {sidechain.enabled ? 'ON' : 'OFF'}</button>{open && <FloatingPanel getAnchorElement={getAnchorElement} onClose={() => setOpen(false)} className="sidechain-picker"><label><input type="checkbox" checked={sidechain.enabled} onChange={(event) => setSidechain(target, effect.id, event.target.checked, sidechain.sourceTrackId)} /> 사이드 체인</label><strong>INPUT CHANNEL</strong><select value={sidechain.sourceTrackId ?? ''} onChange={(event) => setSidechain(target, effect.id, sidechain.enabled, event.target.value || null)}><option value="">채널 선택</option><optgroup label="TRACKS">{tracks.filter((track) => target.kind !== 'track' || track.id !== target.id).map((track) => <option key={track.id} value={track.id}>{track.name}</option>)}</optgroup><optgroup label="RETURNS">{buses.filter((bus) => target.kind !== 'bus' || bus.id !== target.id).map((bus) => <option key={bus.id} value={`bus:${bus.id}`}>{bus.name}</option>)}</optgroup></select><small>보조 입력 버스로 전달되며 순환 라우팅은 자동 차단됩니다.</small></FloatingPanel>}</div>
}

function UtilityPanel({ params, setParam }: { params: Record<string, number>; setParam(param: string, value: number): void }) {
  return <div className="utility-panel">
    <div className="utility-input">
      <strong>INPUT</strong>
      <div><button className={(params.invertLeft ?? 0) >= .5 ? 'active' : ''} onClick={() => setParam('invertLeft', (params.invertLeft ?? 0) >= .5 ? 0 : 1)}>Ø L</button><button className={(params.invertRight ?? 0) >= .5 ? 'active' : ''} onClick={() => setParam('invertRight', (params.invertRight ?? 0) >= .5 ? 0 : 1)}>Ø R</button></div>
      <select value={Math.round(params.inputMode ?? 0)} onChange={(event) => setParam('inputMode', Number(event.target.value))}><option value="0">Stereo</option><option value="1">Left</option><option value="2">Right</option><option value="3">Swap L/R</option></select>
      <Knob value={(params.width ?? 1) * 100} min={0} max={200} step={1} defaultValue={100} label="WIDTH" format={(value) => `${Math.round(value)}%`} onChange={(value) => setParam('width', value / 100)} />
      <button className={(params.mono ?? 0) >= .5 ? 'active wide' : 'wide'} onClick={() => setParam('mono', (params.mono ?? 0) >= .5 ? 0 : 1)}>MONO</button>
      <button className={(params.bassMono ?? 0) >= .5 ? 'active wide' : 'wide'} onClick={() => setParam('bassMono', (params.bassMono ?? 0) >= .5 ? 0 : 1)}>BASS MONO</button>
      <EditableNumber value={params.bassFreq ?? 120} min={40} max={500} step={1} onChange={(value) => setParam('bassFreq', value)} format={hzFormat} />
    </div>
    <div className="utility-output">
      <strong>OUTPUT</strong>
      <Knob value={params.gainDb ?? 0} min={-60} max={24} step={.1} defaultValue={0} label="GAIN" format={dbFormat} onChange={(value) => setParam('gainDb', value)} />
      <Knob value={params.balance ?? 0} min={-1} max={1} step={.01} defaultValue={0} label="BALANCE" format={(value) => value === 0 ? 'C' : value < 0 ? `${Math.round(-value * 100)}L` : `${Math.round(value * 100)}R`} onChange={(value) => setParam('balance', value)} />
      <div className="utility-buttons"><button className={(params.mute ?? 0) >= .5 ? 'active' : ''} onClick={() => setParam('mute', (params.mute ?? 0) >= .5 ? 0 : 1)}>MUTE</button><button className={(params.dcBlock ?? 0) >= .5 ? 'active' : ''} onClick={() => setParam('dcBlock', (params.dcBlock ?? 0) >= .5 ? 0 : 1)}>DC</button></div>
    </div>
  </div>
}

type DistortionBandName = 'low' | 'mid' | 'high'
const DISTORTION_MODES = ['Off', 'Tube', 'Tape', 'Saturation', 'Exciter'] as const
const DISTORTION_BANDS: Array<{ id: DistortionBandName; color: string }> = [
  { id: 'low', color: '#f0ca62' }, { id: 'mid', color: '#62d4f4' }, { id: 'high', color: '#ed916e' },
]

function DistortionPanel({ effectId, params, bypassed, setParam }: { effectId: string; params: Record<string, number>; bypassed: boolean; setParam(param: string, value: number): void }) {
  const [selected, setSelected] = useState<DistortionBandName>('mid')
  const color = DISTORTION_BANDS.find((band) => band.id === selected)!.color
  const enabled = (params[`${selected}Enabled`] ?? 1) >= .5
  return <div className="distortion-panel" style={{ '--distortion-color': color } as React.CSSProperties}>
    <DistortionDisplay effectId={effectId} params={params} bypassed={bypassed} selected={selected} onSelect={setSelected} onParam={setParam} />
    <div className="distortion-controls">
      <button className={enabled ? 'eq-enable active' : 'eq-enable'} onClick={() => setParam(`${selected}Enabled`, enabled ? 0 : 1)}>{enabled ? 'ON' : 'OFF'}</button>
      <label>MODEL<select value={Math.round(params[`${selected}Mode`] ?? 0)} onChange={(event) => setParam(`${selected}Mode`, Number(event.target.value))}>{DISTORTION_MODES.map((mode, index) => <option key={mode} value={index}>{mode}</option>)}</select></label>
      <Knob parameterId={`${selected}GainDb`} value={params[`${selected}GainDb`] ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="GAIN" format={dbFormat} onChange={(value) => setParam(`${selected}GainDb`, value)} />
      <Knob parameterId={`${selected}DriveDb`} value={params[`${selected}DriveDb`] ?? 6} min={0} max={36} step={.1} defaultValue={6} label="DRIVE" format={dbFormat} onChange={(value) => setParam(`${selected}DriveDb`, value)} />
      <Knob parameterId="splitLow" value={params.splitLow ?? 180} min={20} max={Math.max(21, (params.splitHigh ?? 4500) / 1.05)} step={1} scale="log" defaultValue={180} label="LOW FREQ" format={hzFormat} onChange={(value) => setParam('splitLow', value)} />
      <Knob parameterId="splitHigh" value={params.splitHigh ?? 4500} min={Math.min(19_999, (params.splitLow ?? 180) * 1.05)} max={20_000} step={10} scale="log" defaultValue={4500} label="HIGH FREQ" format={hzFormat} onChange={(value) => setParam('splitHigh', value)} />
      <Knob parameterId={`${selected}Mix`} value={params[`${selected}Mix`] ?? .75} min={0} max={1} step={.01} defaultValue={.75} label="MIX" format={percentFormat} onChange={(value) => setParam(`${selected}Mix`, value)} />
      <label>OVERSAMPLE<select value={params.oversample ?? 4} onChange={(event) => setParam('oversample', Number(event.target.value))}><option value="1">1×</option><option value="2">2×</option><option value="4">4×</option></select></label>
    </div>
    <div className="distortion-tabs">{DISTORTION_BANDS.map((band) => { const on = (params[`${band.id}Enabled`] ?? 1) >= .5; return <button key={band.id} className={`${selected === band.id ? 'active' : ''} ${on ? '' : 'disabled'}`} style={{ '--band-color': band.color } as React.CSSProperties} onClick={() => setSelected(band.id)}><b>{band.id.toUpperCase()}</b><span>{on ? DISTORTION_MODES[Math.round(params[`${band.id}Mode`] ?? 0)] : 'BYPASS'}</span></button> })}</div>
  </div>
}

function DistortionDisplay({ effectId, params, bypassed, selected, onSelect, onParam }: { effectId: string; params: Record<string, number>; bypassed: boolean; selected: DistortionBandName; onSelect(band: DistortionBandName): void; onParam(param: string, value: number): void }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const dragging = useRef<'splitLow' | 'splitHigh' | DistortionBandName | null>(null)
  const engine = useEngine()
  const low = params.splitLow ?? 180
  const high = params.splitHigh ?? 4500
  const gain = useCallback((band: DistortionBandName) => params[`${band}GainDb`] ?? 0, [params])
  const draw = useCallback(() => {
    const canvas = canvasRef.current; const context = canvas?.getContext('2d'); if (!canvas || !context) return
    const { width, height } = canvas; const xFor = (frequency: number) => frequencyToX(frequency, width); const yFor = (db: number) => height / 2 - Math.max(-24, Math.min(24, db)) / 24 * height * .4
    const lowX = xFor(low); const highX = xFor(high)
    const background = context.createLinearGradient(0, 0, 0, height); background.addColorStop(0, '#16222a'); background.addColorStop(1, '#0b1116'); context.fillStyle = background; context.fillRect(0, 0, width, height)
    drawFrequencyGrid(context, width, height, '#283640')
    context.strokeStyle = '#283640'; context.lineWidth = 1
    for (const db of [-12, 0, 12]) { const y = yFor(db); context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke() }
    const spectrum = engine.getDistortionSpectrum(effectId); context.beginPath()
    for (let index = 0; index < spectrum.length; index += 1) { const x = xFor(spectrumFrequencyAtIndex(index, spectrum.length)); const db = 20 * Math.log10(Math.max(1e-5, spectrum[index] ?? 0)); const y = height - Math.max(0, Math.min(1, (db + 72) / 72)) * height * .88; if (index === 0) context.moveTo(x, y); else context.lineTo(x, y) }
    context.lineTo(width, height); context.lineTo(0, height); context.closePath(); const fill = context.createLinearGradient(0, 0, 0, height); fill.addColorStop(0, bypassed ? '#7a87902a' : '#5ad3f75b'); fill.addColorStop(1, '#18313b08'); context.fillStyle = fill; context.fill()
    const ranges: Array<{ band: DistortionBandName; from: number; to: number; color: string }> = [{ band: 'low', from: 0, to: lowX, color: '#f0ca62' }, { band: 'mid', from: lowX, to: highX, color: '#62d4f4' }, { band: 'high', from: highX, to: width, color: '#ed916e' }]
    for (const range of ranges) { const y = yFor(gain(range.band)); context.beginPath(); context.moveTo(range.from, y); context.lineTo(range.to, y); context.strokeStyle = bypassed ? '#68747c' : range.color; context.lineWidth = selected === range.band ? 3 : 2; context.stroke(); context.fillStyle = selected === range.band ? `${range.color}18` : `${range.color}0b`; context.fillRect(range.from, 0, range.to - range.from, height); context.fillStyle = range.color; context.font = 'bold 8px sans-serif'; context.textAlign = 'center'; context.fillText(range.band.toUpperCase(), (range.from + range.to) / 2, 12) }
    for (const [frequency, color] of [[low, '#f0ca62'], [high, '#ed916e']] as Array<[number, string]>) { const x = xFor(frequency); context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.strokeStyle = color; context.lineWidth = 2; context.stroke(); context.fillStyle = color; context.fillRect(x - 3, height / 2 - 10, 6, 20) }
    context.fillStyle = '#8b9aa4'; context.font = '7px monospace'; context.textAlign = 'left'; context.fillText(low <= 20.5 ? 'LOW OFF' : hzFormat(low), 5, height - 15); context.textAlign = 'right'; context.fillText(high >= 19_950 ? 'HIGH OFF' : hzFormat(high), width - 5, height - 15)
  }, [bypassed, effectId, engine, gain, high, low, selected])
  useEffect(() => { draw(); return subscribeAnalyzerFrame(draw) }, [draw])
  const point = (event: React.PointerEvent<HTMLCanvasElement>) => { const bounds = event.currentTarget.getBoundingClientRect(); return { x: (event.clientX - bounds.left) * event.currentTarget.width / bounds.width, y: (event.clientY - bounds.top) * event.currentTarget.height / bounds.height } }
  const update = (event: React.PointerEvent<HTMLCanvasElement>) => { const target = dragging.current; if (!target) return; const p = point(event); const { width, height } = event.currentTarget; if (target === 'splitLow' || target === 'splitHigh') { let frequency = parameterFrequencyAtX(p.x, width); if (target === 'splitLow') { if (p.x < 5) frequency = 20; frequency = Math.min(frequency, high / 1.05) } else { if (p.x > width - 5) frequency = 20_000; frequency = Math.max(frequency, low * 1.05) } onParam(target, Math.round(frequency)); return } const value = (height / 2 - p.y) / (height * .4) * 24; onParam(`${target}GainDb`, Math.round(Math.max(-24, Math.min(24, value)) * 10) / 10) }
  const down = (event: React.PointerEvent<HTMLCanvasElement>) => { const p = point(event); const width = event.currentTarget.width; const lowX = frequencyToX(low, width); const highX = frequencyToX(high, width); if (Math.abs(p.x - lowX) <= 9) dragging.current = 'splitLow'; else if (Math.abs(p.x - highX) <= 9) dragging.current = 'splitHigh'; else { const band: DistortionBandName = p.x < lowX ? 'low' : p.x < highX ? 'mid' : 'high'; onSelect(band); dragging.current = band } event.currentTarget.setPointerCapture(event.pointerId); update(event) }
  const up = (event: React.PointerEvent<HTMLCanvasElement>) => { dragging.current = null; if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId) }
  return <canvas ref={canvasRef} className="distortion-display" width="500" height="112" onPointerDown={down} onPointerMove={update} onPointerUp={up} onPointerCancel={up} title="Drag crossovers horizontally and band gain lines vertically" />
}

type MultibandBand = 'low' | 'mid' | 'high'

function MultibandCompressorPanel({ effectId, params, setParam }: { effectId: string; params: Record<string, number>; setParam(param: string, value: number): void }) {
  const [selected, setSelected] = useState<MultibandBand>('mid')
  const bands: Array<{ id: MultibandBand; name: string; range: string; color: string }> = [
    { id: 'high', name: 'HIGH', range: `${hzFormat(params.splitHigh ?? 2500)} +`, color: '#ef9564' },
    { id: 'mid', name: 'MID', range: `${hzFormat(params.splitLow ?? 150)} – ${hzFormat(params.splitHigh ?? 2500)}`, color: '#68d6f7' },
    { id: 'low', name: 'LOW', range: `20 – ${hzFormat(params.splitLow ?? 150)}`, color: '#f1c75b' },
  ]
  const prefix = selected
  const threshold = params[`${prefix}Threshold`] ?? (selected === 'low' ? -24 : selected === 'mid' ? -20 : -18)
  const ratio = params[`${prefix}Ratio`] ?? (selected === 'low' ? 3 : selected === 'mid' ? 2.5 : 2)
  const attack = params[`${prefix}Attack`] ?? (selected === 'low' ? .03 : selected === 'mid' ? .015 : .006)
  const release = params[`${prefix}Release`] ?? (selected === 'low' ? .25 : selected === 'mid' ? .18 : .12)
  const makeup = params[`${prefix}MakeupDb`] ?? 0
  return <div className="multiband-panel">
    <div className="multiband-top">
      <div className="multiband-splits">
        <Knob value={params.splitLow ?? 150} min={20} max={150} step={1} scale="log" defaultValue={150} label="LOW SPLIT" format={hzFormat} onChange={(value) => setParam('splitLow', value)} />
        <Knob value={params.splitHigh ?? 2500} min={300} max={16_000} step={10} scale="log" defaultValue={2500} label="HIGH SPLIT" format={hzFormat} onChange={(value) => setParam('splitHigh', value)} />
      </div>
      <div className="multiband-display" aria-label="Multiband compressor bands">
        {bands.map((band) => {
          const bandThreshold = params[`${band.id}Threshold`] ?? -20
          const bandRatio = params[`${band.id}Ratio`] ?? 2
          return <button key={band.id} className={selected === band.id ? 'active' : ''} onClick={() => setSelected(band.id)} style={{ '--band-color': band.color } as React.CSSProperties}>
            <span><strong>{band.name}</strong><small>{band.range}</small></span>
            <MultibandBandMeter effectId={effectId} band={band.id} threshold={bandThreshold} onActivate={() => setSelected(band.id)} onThreshold={(value) => setParam(`${band.id}Threshold`, value)} />
            <output>{bandThreshold.toFixed(1)} dB&nbsp; · &nbsp;{bandRatio.toFixed(1)}:1</output>
          </button>
        })}
      </div>
      <div className="multiband-output">
        <Knob value={params.outputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} />
        <Knob value={params.mix ?? 1} min={0} max={1} step={.01} defaultValue={1} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} />
      </div>
    </div>
    <div className="multiband-band-controls">
      <strong style={{ '--band-color': bands.find((band) => band.id === selected)?.color } as React.CSSProperties}>{selected.toUpperCase()}</strong>
      <Knob parameterId={`${prefix}Threshold`} value={threshold} min={-60} max={0} step={.1} defaultValue={-20} label="THRESH" format={dbFormat} onChange={(value) => setParam(`${prefix}Threshold`, value)} />
      <Knob parameterId={`${prefix}Ratio`} value={ratio} min={1} max={20} step={.1} defaultValue={2} label="RATIO" format={(value) => `${value.toFixed(1)}:1`} onChange={(value) => setParam(`${prefix}Ratio`, value)} />
      <Knob parameterId={`${prefix}Attack`} value={attack * 1000} min={1} max={500} step={1} defaultValue={15} label="ATTACK" format={msFormat} onChange={(value) => setParam(`${prefix}Attack`, value / 1000)} />
      <Knob parameterId={`${prefix}Release`} value={release * 1000} min={10} max={1000} step={1} defaultValue={180} label="RELEASE" format={msFormat} onChange={(value) => setParam(`${prefix}Release`, value / 1000)} />
      <Knob parameterId={`${prefix}MakeupDb`} value={makeup} min={-12} max={24} step={.1} defaultValue={0} label="MAKEUP" format={dbFormat} onChange={(value) => setParam(`${prefix}MakeupDb`, value)} />
      <Knob value={params.knee ?? 8} min={0} max={24} step={.5} defaultValue={8} label="KNEE" format={dbFormat} onChange={(value) => setParam('knee', value)} />
    </div>
  </div>
}

function MultibandBandMeter({ effectId, band, threshold, onActivate, onThreshold }: { effectId: string; band: MultibandBand; threshold: number; onActivate(): void; onThreshold(value: number): void }) {
  const engine = useEngine()
  const leftRef = useRef<HTMLElement>(null)
  const rightRef = useRef<HTMLElement>(null)

  useEffect(() => subscribeMeterFrame(() => {
    const levels = engine.getMultibandLevels(effectId)[band]
    leftRef.current?.style.setProperty('transform', `scaleX(${levelMeterScale(levels.left)})`)
    rightRef.current?.style.setProperty('transform', `scaleX(${levelMeterScale(levels.right)})`)
  }), [band, effectId, engine])

  const updateThreshold = (event: React.PointerEvent<HTMLElement>) => {
    const bounds = event.currentTarget.parentElement?.getBoundingClientRect()
    if (!bounds) return
    const normalized = Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width))
    onThreshold(Math.round((-60 + normalized * 60) * 10) / 10)
  }
  const beginThresholdDrag = (event: React.PointerEvent<HTMLElement>) => {
    event.stopPropagation()
    onActivate()
    event.currentTarget.setPointerCapture(event.pointerId)
    updateThreshold(event)
  }
  const endThresholdDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  const thresholdPosition = Math.max(0, Math.min(100, (threshold + 60) / 60 * 100))
  return <div className="multiband-meter" title="L/R band level · drag the marker to change threshold">
    <span>L<i ref={leftRef} /></span>
    <span>R<i ref={rightRef} /></span>
    <em role="slider" aria-label={`${band} threshold`} aria-valuemin={-60} aria-valuemax={0} aria-valuenow={threshold} tabIndex={0} style={{ left: `${thresholdPosition}%` }} onPointerDown={beginThresholdDrag} onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) updateThreshold(event) }} onPointerUp={endThresholdDrag} onPointerCancel={endThresholdDrag} />
  </div>
}

function levelMeterScale(level: number): number {
  if (level <= 0.000_001) return 0
  const db = 20 * Math.log10(level)
  return Math.max(0, Math.min(1, (db + 60) / 60))
}

function InstrumentCard({ track }: { track: Track }) {
  const instrument = track.instrument!
  const update = useProjectStore((state) => state.updateInstrument)
  const toggleBypass = useProjectStore((state) => state.toggleInstrumentBypass)
  const setPlugin = useProjectStore((state) => state.setTrackInstrumentPlugin)
  const replaceInstrument = useProjectStore((state) => state.replaceTrackInstrument)
  const engine = useEngine()
  const [voices, setVoices] = useState(0)
  const [pickerOpen, setPickerOpen] = useState(false)
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [loading, setLoading] = useState(false)
  const [dropActive, setDropActive] = useState(false)
  const [collapsed, setCollapsed] = useState(false)
  const pickerRef = useRef<HTMLButtonElement>(null)
  const cardRef = useRef<HTMLElement>(null)
  const getPickerAnchor = useCallback(() => pickerRef.current, [])
  useEffect(() => { const timer = window.setInterval(() => setVoices(engine.getActiveVoiceCount(track.id)), 250); return () => window.clearInterval(timer) }, [engine, track.id])
  useEffect(() => subscribeBrowserDrag((state) => {
    if (state.payload.kind !== 'instrument') return
    const inside = state.type !== 'cancel' && !!cardRef.current?.contains(document.elementFromPoint(state.x, state.y))
    setDropActive(inside)
    if (state.type === 'drop' && inside) {
      const plugin = state.payload.plugin
      void (plugin ? hydratePluginRef(engine, plugin, true) : Promise.resolve(undefined)).then((detailed) => {
        replaceInstrument(track.id, detailed)
        useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id })
        useProjectStore.getState().showToast(`${detailed?.name ?? 'DefaultSynth'}로 악기를 교체했습니다.`)
        if (detailed && detailed.hasEditor !== false) openPluginEditorWhenReady(engine, 'instrument', track.id, (error) => useProjectStore.getState().showToast(`${detailed.name} 편집기를 열 수 없습니다: ${String(error)}`))
      }).catch((error) => useProjectStore.getState().showToast(`${plugin?.name ?? '악기'} 파라미터를 불러오지 못했습니다: ${String(error)}`))
    }
  }), [engine, replaceInstrument, track.id])
  useEffect(() => {
    if (!pickerOpen || plugins.length || loading) return
    let cancelled = false
    setLoading(true)
    void scanPluginsOnce(engine).then((result) => { if (!cancelled) setPlugins(result.filter((plugin) => plugin.isInstrument)) }).finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [engine, loading, pickerOpen, plugins.length])
  const setParam = (id: string, value: number) => { update(track.id, { [id]: value }); engine.setInstrumentParam(track.id, id, value) }
  const automationItems = useCallback((parameterId: string | undefined, label: string, value: number) => parameterAutomationMenu(useProjectStore.getState().project.tracks.find((candidate) => candidate.id === track.id) ?? track, 'instrument', track.id, parameterId, label, value), [track])
  const openEditor = () => {
    if (!instrument.plugin) return
    void engine.openPluginEditor('instrument', track.id).catch((error) => useProjectStore.getState().showToast(`${instrument.plugin!.name} 편집기를 열 수 없습니다: ${describeEngineError(error)}`))
  }
  const savePreset = () => {
    void (async () => {
      const state = instrument.plugin ? await engine.savePluginState('instrument', track.id) : undefined
      const path = await saveDevicePreset({ format: 'ministudio-device-preset', version: 1, name: instrument.plugin?.name ?? 'DefaultSynth', deviceType: instrument.type, pluginUid: instrument.plugin?.uid, params: { ...instrument.params }, state })
      if (path) useProjectStore.getState().showToast(`${instrument.plugin?.name ?? 'DefaultSynth'} 프리셋을 저장했습니다.`)
    })().catch((error) => useProjectStore.getState().showToast(`프리셋 저장 실패: ${String(error)}`))
  }
  const loadPreset = () => {
    void (async () => {
      const preset = await openDevicePreset()
      if (!preset) return
      if (preset.deviceType !== instrument.type || (instrument.plugin && preset.pluginUid !== instrument.plugin.uid)) throw new Error('현재 악기와 다른 종류의 프리셋입니다.')
      if (instrument.plugin && preset.state) await engine.loadPluginState('instrument', track.id, preset.state)
      update(track.id, preset.params)
      for (const [parameter, value] of Object.entries(preset.params)) engine.setInstrumentParam(track.id, parameter, value)
      useProjectStore.getState().showToast(`${instrument.plugin?.name ?? 'DefaultSynth'} 프리셋을 불러왔습니다.`)
    })().catch((error) => useProjectStore.getState().showToast(`프리셋 불러오기 실패: ${String(error)}`))
  }
  if (collapsed) return <div className={`device-collapsed ${instrument.bypassed ? 'bypassed' : ''}`}><button className="collapsed-open" onClick={() => setCollapsed(false)} title="악기 펼치기"><span>{instrument.plugin?.name ?? 'Test Tone'}</span></button></div>
  return <ParameterAutomationProvider items={automationItems}><article
    ref={cardRef}
    className={`device-card instrument-card ${instrument.bypassed ? 'bypassed' : ''} ${dropActive ? 'instrument-drop-active' : ''}`}
  >
    <header className="device-card-header"><div className="device-header-main"><span className="device-grip">♪</span><button className={instrument.bypassed ? '' : 'powered'} title={instrument.bypassed ? '인스트루먼트 켜기' : '인스트루먼트 끄기'} onClick={() => toggleBypass(track.id)}><CirclePower size={13} /></button><strong title={instrument.plugin ? '더블클릭하여 악기 창 열기' : undefined} onDoubleClick={(event) => { event.stopPropagation(); if (instrument.plugin) openEditor() }}>{instrument.plugin?.name ?? 'Test Tone'}</strong><span>{instrument.plugin?.format.toUpperCase() ?? `${voices} / ${Math.round(instrument.params.polyphony ?? 16)} voices`}</span><div className="device-header-actions"><button ref={pickerRef} onClick={() => setPickerOpen((open) => !open)} title="인스트루먼트 선택">▾</button><button onClick={() => setCollapsed(true)} title="접기"><Minus size={12} /></button>{instrument.plugin && <button className="device-close" title="악기 제거" onClick={() => replaceInstrument(track.id)}><X size={13} /></button>}</div></div><div className="device-header-sub"><button className={`device-bypass ${instrument.bypassed ? 'active' : ''}`} onClick={() => toggleBypass(track.id)}>BYPASS {instrument.bypassed ? 'ON' : 'OFF'}</button><button className="device-preset-button" title="프리셋 저장" onClick={savePreset}><Save size={11} /><span>SAVE</span></button><button className="device-preset-button" title="프리셋 불러오기" onClick={loadPreset}><FolderOpen size={11} /><span>LOAD</span></button>{instrument.plugin && <DeviceAutomationModes trackId={track.id} targetKind="instrument" targetId={track.id} />}{instrument.plugin && instrument.plugin.hasEditor !== false && <button className="device-editor-button" title="악기 창 열기" onClick={openEditor}><Piano size={12} /><span>EDIT</span></button>}</div></header>
    <div className="device-body">
      {instrument.plugin && <ExternalInstrumentEditor track={track} setParam={setParam} />}
      {!instrument.plugin && <>
      <div className="select-row">
        <label>WAVEFORM <select value={instrument.params.waveform ?? 0} onChange={(event) => setParam('waveform', Number(event.target.value))}><option value="0">Sine</option><option value="1">Triangle</option><option value="2">Saw</option><option value="3">Square</option></select></label>
        <label>POLY <select value={Math.round(instrument.params.polyphony ?? 16)} onChange={(event) => setParam('polyphony', Number(event.target.value))}>{[1, 2, 4, 8, 16, 24, 32].map((count) => <option key={count} value={count}>{count}</option>)}</select></label>
      </div>
      <div className="parameter-row">
        <Knob value={instrument.params.attack ?? .01} min={.001} max={5} step={.001} defaultValue={.01} label="ATTACK" format={(value) => `${Math.round(value * 1000)} ms`} onChange={(value) => setParam('attack', value)} />
        <Knob value={instrument.params.decay ?? .15} min={.005} max={8} step={.005} defaultValue={.15} label="DECAY" format={(value) => `${Math.round(value * 1000)} ms`} onChange={(value) => setParam('decay', value)} />
        <Knob value={instrument.params.sustain ?? .7} min={0} max={1} step={.01} defaultValue={.7} label="SUSTAIN" format={percentFormat} onChange={(value) => setParam('sustain', value)} />
        <Knob value={instrument.params.release ?? .3} min={.005} max={12} step={.005} defaultValue={.3} label="RELEASE" format={(value) => `${value.toFixed(2)} s`} onChange={(value) => setParam('release', value)} />
        <Knob value={instrument.params.velocityCurve ?? 1} min={.25} max={4} step={.05} defaultValue={1} label="VEL CURVE" format={(value) => value.toFixed(2)} onChange={(value) => setParam('velocityCurve', value)} />
        <Knob value={instrument.params.gainDb ?? -12} min={-60} max={6} step={.1} defaultValue={-12} label="GAIN" format={dbFormat} onChange={(value) => setParam('gainDb', value)} />
      </div>
      </>}
    </div>
    {pickerOpen && <FloatingPanel getAnchorElement={getPickerAnchor} onClose={() => setPickerOpen(false)} className="device-picker"><strong>VST3 / CLAP INSTRUMENTS</strong>{loading && <small>플러그인을 검색하는 중…</small>}{plugins.map((plugin) => <button key={`${plugin.format}:${plugin.uid}:${plugin.path}`} onClick={() => { void hydratePlugin(engine, plugin).then((detailed) => { const ref = pluginRef(detailed); setPlugin(track.id, ref); openPluginEditorWhenReady(engine, 'instrument', track.id, (error) => useProjectStore.getState().showToast(`${ref.name} 편집기를 열 수 없습니다: ${String(error)}`)) }); setPickerOpen(false) }}><span>{plugin.name}</span><small>{plugin.format.toUpperCase()} · {plugin.vendor || plugin.category}</small></button>)}{!loading && plugins.length === 0 && <small>설치된 인스트루먼트를 찾지 못했습니다.</small>}</FloatingPanel>}
  </article></ParameterAutomationProvider>
}

function ExternalInstrumentEditor({ track, setParam }: { track: Track; setParam(id: string, value: number): void }) {
  const plugin = track.instrument!.plugin!
  const parameters = plugin.parameters ?? []
  const visible = rackParameters(parameters)
  if (!visible.length) return <PluginIdentitySurface plugin={plugin} />
  return <div className="external-instrument-editor">
    <div className="external-instrument-heading"><strong>{plugin.name}</strong><span>{parameters.length < 12 ? 'ALL PARAMETERS' : `${visible.length} MACROS`}</span></div>
    <PluginParameterGrid parameters={visible} values={track.instrument!.params} setParam={setParam} />
  </div>
}

function macroParameters(parameters: NonNullable<ExternalPluginRef['parameters']>) {
  const preferred = /\b(macro|quick control|perform|performance|assign|remote)\b/i
  return parameters.filter((parameter) => preferred.test(`${parameter.module} ${parameter.name}`)).slice(0, 32)
}

function rackParameters(parameters: NonNullable<ExternalPluginRef['parameters']>) {
  return parameters.length < 12 ? parameters : macroParameters(parameters)
}

const EQ_RANGE_DB = 18

type EqBandView = { index: number; enabled: boolean; frequency: number; gain: number; q: number; shape: number; slope: number; color: string }
const EQ_COLORS = ['#f2b84b', '#64d6f5', '#70e09a', '#ef8dab', '#ad8cff', '#5de0cf', '#ef995e', '#d6dd68']

function EqPanel({ effectId, params, bypassed, bandCount, setParam }: { effectId: string; params: Record<string, number>; bypassed: boolean; bandCount: 4 | 8; setParam(param: string, value: number): void }) {
  const [selected, setSelected] = useState(0)
  const defaults = useMemo(() => bandCount === 4 ? [80, 400, 2500, 10000] : [30, 100, 300, 800, 2500, 6000, 12000, 16000], [bandCount])
  const bands = useMemo<EqBandView[]>(() => defaults.map((frequency, index) => ({
    index,
    enabled: (params[`band${index}.enabled`] ?? (index === 0 ? 1 : 0)) >= .5,
    frequency: params[`band${index}.freq`] ?? (bandCount === 4 && index === 0 ? params.lowFreq : bandCount === 4 && index === 1 ? params.midFreq : bandCount === 4 && index === 3 ? params.highFreq : undefined) ?? frequency,
    gain: params[`band${index}.gain`] ?? (bandCount === 4 && index === 0 ? params.lowGain : bandCount === 4 && index === 1 ? params.midGain : bandCount === 4 && index === 3 ? params.highGain : undefined) ?? 0,
    q: params[`band${index}.q`] ?? (bandCount === 4 && index === 1 ? params.midQ : undefined) ?? .71,
    shape: Math.round(params[`band${index}.type`] ?? (index === 0 ? 3 : index === bandCount - 1 ? 2 : 0)),
    slope: Math.round(params[`band${index}.slope`] ?? 12),
    color: EQ_COLORS[index]!,
  })), [bandCount, defaults, params])
  const band = bands[Math.min(selected, bandCount - 1)]!
  const shapes = band.index === 0 ? [{ value: 1, name: 'Low Shelf' }, { value: 3, name: 'Low Cut' }, { value: 0, name: 'Bell' }] : band.index === bandCount - 1 ? [{ value: 2, name: 'High Shelf' }, { value: 4, name: 'High Cut' }, { value: 0, name: 'Bell' }] : [{ value: 0, name: 'Bell' }, { value: 5, name: 'Notch' }]
  return <div className={`eq-panel bands-${bandCount}`}>
    <InteractiveEqDisplay effectId={effectId} bands={bands} selected={band.index} bypassed={bypassed} scale={params.scale ?? 1} adaptiveQ={(params.adaptiveQ ?? (bandCount === 8 ? 1 : 0)) >= .5} onSelect={setSelected} onParam={setParam} />
    <div className="eq-band-tabs">{bands.map((item) => <button key={item.index} aria-pressed={item.enabled} className={`${selected === item.index ? 'active' : ''} ${item.enabled ? 'enabled' : 'disabled'}`} style={{ '--eq-color': item.color } as React.CSSProperties} onClick={() => setSelected(item.index)} onDoubleClick={() => setParam(`band${item.index}.enabled`, item.enabled ? 0 : 1)} title={`Band ${item.index + 1} · double-click to ${item.enabled ? 'disable' : 'enable'}`}><i /><span>{item.index + 1}</span></button>)}</div>
    <div className="eq-selected-controls" style={{ '--eq-color': band.enabled ? band.color : '#697681' } as React.CSSProperties}>
      <button className={band.enabled ? 'eq-enable active' : 'eq-enable'} aria-pressed={band.enabled} onClick={() => setParam(`band${band.index}.enabled`, band.enabled ? 0 : 1)}><CirclePower size={14} /><span>{band.enabled ? 'ON' : 'OFF'}</span></button>
      <Knob parameterId={`band${band.index}.q`} value={band.q} min={.2} max={12} step={.01} defaultValue={.71} label="Q" format={(value) => value.toFixed(2)} onChange={(value) => setParam(`band${band.index}.q`, value)} />
      <Knob parameterId={`band${band.index}.gain`} value={band.gain} min={-18} max={18} step={.1} defaultValue={0} label="GAIN" format={dbFormat} onChange={(value) => setParam(`band${band.index}.gain`, value)} />
      <Knob parameterId={`band${band.index}.freq`} value={band.frequency} min={20} max={20000} step={1} scale="log" defaultValue={defaults[band.index]!} label="FREQ" format={hzFormat} onChange={(value) => setParam(`band${band.index}.freq`, value)} />
      <div className="eq-filter-selects"><label>SHAPE<select value={band.shape} onChange={(event) => { const shape = Number(event.target.value); setParam(`band${band.index}.type`, shape); if (shape === 3 || shape === 4) setParam(`band${band.index}.slope`, band.slope) }}>{shapes.map((shape) => <option key={shape.value} value={shape.value}>{shape.name}</option>)}</select></label>{(band.shape === 3 || band.shape === 4) && <label>SLOPE<select value={band.slope} onChange={(event) => setParam(`band${band.index}.slope`, Number(event.target.value))}>{[12, 24, 36, 48].map((slope) => <option key={slope} value={slope}>{slope} dB/oct</option>)}</select></label>}</div>
    </div>
    {bandCount === 8 && <div className="eq8-global"><strong>STEREO · 8 BAND</strong><button className={(params.adaptiveQ ?? 1) >= .5 ? 'active' : ''} onClick={() => setParam('adaptiveQ', (params.adaptiveQ ?? 1) >= .5 ? 0 : 1)}>ADAPT Q</button><Knob value={(params.scale ?? 1) * 100} min={0} max={200} step={1} defaultValue={100} label="SCALE" format={(value) => `${Math.round(value)}%`} onChange={(value) => setParam('scale', value / 100)} /><Knob value={params.outputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} /></div>}
  </div>
}

function InteractiveEqDisplay({ effectId, bands, selected, bypassed, scale, adaptiveQ, onSelect, onParam }: { effectId: string; bands: EqBandView[]; selected: number; bypassed: boolean; scale: number; adaptiveQ: boolean; onSelect(index: number): void; onParam(param: string, value: number): void }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const dragging = useRef<number | null>(null)
  const dragPreview = useRef<{ index: number; frequency: number; gain: number } | null>(null)
  const lastDragValues = useRef<{ frequency: number; gain: number } | null>(null)
  const engine = useEngine()
  const draw = useCallback(() => {
    const canvas = ref.current; const context = canvas?.getContext('2d'); if (!canvas || !context) return
    const displayWidth = Math.max(1, Math.round(canvas.clientWidth)); const displayHeight = Math.max(1, Math.round(canvas.clientHeight))
    if (canvas.width !== displayWidth || canvas.height !== displayHeight) { canvas.width = displayWidth; canvas.height = displayHeight }
    const { width, height } = canvas; const toY = (db: number) => height / 2 - Math.max(-EQ_RANGE_DB, Math.min(EQ_RANGE_DB, db)) / EQ_RANGE_DB * height * .46
    context.clearRect(0, 0, width, height); drawFrequencyGrid(context, width, height, '#2b3743')
    context.strokeStyle = '#2b3743'; context.lineWidth = 1
    for (let y = 0; y <= height; y += height / 4) { context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke() }
    drawEffectSpectrum(context, engine.getEffectSpectrum(effectId), width, height, bypassed ? '#69737a' : '#638c9a')
    const preview = dragPreview.current
    const visibleBands = preview ? bands.map((band) => band.index === preview.index ? { ...band, frequency: preview.frequency, gain: preview.gain } : band) : bands
    context.beginPath(); context.strokeStyle = bypassed ? '#56636d' : '#62d6fa'; context.lineWidth = 2
    let pathVisible = false
    for (let x = 0; x < width; x += 1) {
      const hz = displayFrequencyAtX(x, width)
      const value = visibleBands.reduce((sum, band) => sum + previewEqBand(band, hz, scale, adaptiveQ), 0)
      const outsideCut = value <= -EQ_RANGE_DB + .05 && visibleBands.some((band) => band.enabled && ((band.shape === 3 && hz < band.frequency) || (band.shape === 4 && hz > band.frequency)))
      if (outsideCut) { pathVisible = false; continue }
      if (!pathVisible) { context.moveTo(x, toY(value)); pathVisible = true } else context.lineTo(x, toY(value))
    }
    context.stroke()
    for (const band of visibleBands.filter((item) => item.enabled)) { const x = frequencyToX(band.frequency, width); const y = toY(band.gain); context.beginPath(); context.arc(x, y, band.index === selected ? 6 : 5, 0, Math.PI * 2); context.fillStyle = bypassed ? '#64717a' : band.color; context.fill(); context.strokeStyle = band.index === selected ? '#fff' : '#10171c'; context.lineWidth = band.index === selected ? 1.5 : 1; context.stroke(); context.fillStyle = '#10171c'; context.font = 'bold 6px sans-serif'; context.textAlign = 'center'; context.textBaseline = 'middle'; context.fillText(String(band.index + 1), x, y) }
  }, [adaptiveQ, bands, bypassed, effectId, engine, scale, selected])
  useEffect(() => { draw(); const observer = new ResizeObserver(draw); if (ref.current) observer.observe(ref.current); const unsubscribe = subscribeAnalyzerFrame(draw); return () => { observer.disconnect(); unsubscribe() } }, [draw])
  const point = (event: React.PointerEvent<HTMLCanvasElement>) => { const bounds = event.currentTarget.getBoundingClientRect(); return { x: (event.clientX - bounds.left) * event.currentTarget.width / bounds.width, y: (event.clientY - bounds.top) * event.currentTarget.height / bounds.height } }
  const update = (event: React.PointerEvent<HTMLCanvasElement>) => { const index = dragging.current; if (index === null) return; const p = point(event); const frequency = Math.round(parameterFrequencyAtX(p.x, event.currentTarget.width)); const gain = Math.round(Math.max(-18, Math.min(18, (event.currentTarget.height / 2 - p.y) / (event.currentTarget.height * .46) * EQ_RANGE_DB)) * 10) / 10; dragPreview.current = { index, frequency, gain }; draw(); const last = lastDragValues.current; if (last?.frequency === frequency && last.gain === gain) return; lastDragValues.current = { frequency, gain }; onParam(`band${index}.freq`, frequency); onParam(`band${index}.gain`, gain) }
  const closestEnabled = (event: React.PointerEvent<HTMLCanvasElement> | React.WheelEvent<HTMLCanvasElement>) => { const p = point(event as React.PointerEvent<HTMLCanvasElement>); return bands.filter((band) => band.enabled).map((band) => ({ band, distance: Math.hypot(frequencyToX(band.frequency, event.currentTarget.width) - p.x, event.currentTarget.height / 2 - band.gain / EQ_RANGE_DB * event.currentTarget.height * .46 - p.y) })).sort((a, b) => a.distance - b.distance)[0] }
  const down = (event: React.PointerEvent<HTMLCanvasElement>) => { const closest = closestEnabled(event); if (!closest || closest.distance > 16) return; dragging.current = closest.band.index; lastDragValues.current = null; onSelect(closest.band.index); event.currentTarget.setPointerCapture(event.pointerId); update(event) }
  const up = (event: React.PointerEvent<HTMLCanvasElement>) => { dragging.current = null; dragPreview.current = null; lastDragValues.current = null; if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId) }
  const wheel = (event: React.WheelEvent<HTMLCanvasElement>) => { const closest = closestEnabled(event); if (!closest || closest.distance > 18) return; event.preventDefault(); onSelect(closest.band.index); onParam(`band${closest.band.index}.q`, Math.max(.2, Math.min(12, Math.round((closest.band.q * (event.deltaY < 0 ? 1.08 : 1 / 1.08)) * 100) / 100))) }
  return <canvas className="eq-display interactive" ref={ref} width="450" height="48" onPointerDown={down} onPointerMove={update} onPointerUp={up} onPointerCancel={up} onWheel={wheel} title="포인트 드래그: 주파수/게인 · 포인트 위 휠: Q" />
}

function previewEqBand(band: EqBandView, hz: number, scale = 1, adaptiveQ = false): number { if (!band.enabled) return 0; const gain = band.gain * scale; const q = adaptiveQ ? band.q * (1 + Math.abs(gain) / 36) : band.q; const distance = Math.log2(hz / band.frequency); const slope = band.slope / 12; if (band.shape === 1) return gain / (1 + Math.exp(distance * 4)); if (band.shape === 2) return gain / (1 + Math.exp(-distance * 4)); if (band.shape === 3) return distance < 0 ? -18 * Math.min(1, -distance * q * slope) : 0; if (band.shape === 4) return distance > 0 ? -18 * Math.min(1, distance * q * slope) : 0; if (band.shape === 5) return -18 * Math.exp(-(distance ** 2) * q); return gain * Math.exp(-(distance ** 2) * q * .8) }

function drawFrequencyGrid(context: CanvasRenderingContext2D, width: number, height: number, color: string): void {
  context.save()
  context.lineWidth = 1
  for (const tick of FREQUENCY_TICKS) {
    const x = frequencyToX(tick.hz, width)
    context.globalAlpha = tick.major ? .72 : .24
    context.strokeStyle = color
    context.beginPath(); context.moveTo(Math.round(x) + .5, 0); context.lineTo(Math.round(x) + .5, height); context.stroke()
  }
  context.globalAlpha = 1
  context.fillStyle = '#6c7b85'
  context.font = '6px monospace'
  context.textBaseline = 'bottom'
  for (const tick of FREQUENCY_TICKS) {
    if (!tick.label) continue
    const x = frequencyToX(tick.hz, width)
    context.textAlign = x < 12 ? 'left' : x > width - 12 ? 'right' : 'center'
    context.fillText(tick.label, Math.max(2, Math.min(width - 2, x)), height - 2)
  }
  context.restore()
}

function drawEffectSpectrum(context: CanvasRenderingContext2D, spectrum: readonly number[], width: number, height: number, color: string): void {
  if (!spectrum.some((value) => value > 1e-5)) return
  context.beginPath(); context.moveTo(0, height)
  for (let index = 0; index < spectrum.length; index += 1) {
    const x = frequencyToX(spectrumFrequencyAtIndex(index, spectrum.length), width)
    const db = 20 * Math.log10(Math.max(1e-5, spectrum[index] ?? 0))
    const y = height - Math.max(0, Math.min(1, (db + 72) / 72)) * height * .9
    context.lineTo(x, y)
  }
  context.lineTo(width, height); context.closePath()
  const gradient = context.createLinearGradient(0, 0, 0, height)
  gradient.addColorStop(0, `${color}55`); gradient.addColorStop(1, `${color}08`)
  context.fillStyle = gradient; context.fill()
  context.strokeStyle = `${color}88`; context.lineWidth = 1; context.stroke()
}

/**
 * Draws the EQ magnitude response measured by the native filters. The engine is
 * the source of truth for the curve; the analytic fallback only exists so the
 * browser preview (no Tauri IPC) still shows the shape being dialled in.
 */
// Kept temporarily for project-file migration previews that still carry the
// legacy low/mid/high aliases; new devices use InteractiveEqDisplay above.
// oxlint-disable-next-line eslint/no-unused-vars
function EqDisplay({ effectId, params, bypassed, onParam }: { effectId: string; params: Record<string, number>; bypassed: boolean; onParam(param: string, value: number): void }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const dragging = useRef<'low' | 'mid' | 'high' | null>(null)
  const engine = useEngine()
  const [measured, setMeasured] = useState<{ frequencies: number[]; combinedDb: number[] } | null>(null)
  const nodes = useMemo(() => [
    { id: 'low' as const, frequency: params.lowFreq ?? 90, gain: params.lowGain ?? 0, color: '#f1c75b' },
    { id: 'mid' as const, frequency: params.midFreq ?? 1200, gain: params.midGain ?? 0, color: '#67d8fa' },
    { id: 'high' as const, frequency: params.highFreq ?? 8000, gain: params.highGain ?? 0, color: '#ef8dab' },
  ], [params.highFreq, params.highGain, params.lowFreq, params.lowGain, params.midFreq, params.midGain])

  useEffect(() => {
    let cancelled = false
    // The knob has already pushed the value to the engine through the realtime
    // queue, so wait a frame before asking the engine what it now sounds like.
    const timer = window.setTimeout(() => {
      void engine.getEqResponse(effectId, 160).then((response) => {
        if (!cancelled) setMeasured(response && { frequencies: response.frequencies, combinedDb: response.combinedDb })
      })
    }, 60)
    return () => { cancelled = true; window.clearTimeout(timer) }
  }, [engine, effectId, params])

  useEffect(() => {
    const canvas = ref.current
    const context = canvas?.getContext('2d')
    if (!canvas || !context) return
    const { width, height } = canvas
    context.clearRect(0, 0, width, height)
    drawFrequencyGrid(context, width, height, '#2b3743')
    context.strokeStyle = '#2b3743'
    context.lineWidth = 1
    for (let y = 0; y <= height; y += height / 4) { context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke() }
    context.strokeStyle = '#3a4956'
    context.beginPath(); context.moveTo(0, height / 2); context.lineTo(width, height / 2); context.stroke()

    const toY = (db: number) => height / 2 - Math.max(-EQ_RANGE_DB, Math.min(EQ_RANGE_DB, db)) / EQ_RANGE_DB * height * 0.46
    context.strokeStyle = bypassed ? '#55636f' : measured ? '#63d5ff' : '#8fa4b3'
    context.lineWidth = 2
    context.beginPath()
    if (measured) {
      for (const [index, hz] of measured.frequencies.entries()) {
        const x = frequencyToX(hz, width)
        const y = toY(measured.combinedDb[index] ?? 0)
        if (index === 0) context.moveTo(x, y); else context.lineTo(x, y)
      }
    } else {
      for (let x = 0; x < width; x += 1) {
        const hz = displayFrequencyAtX(x, width)
        const lowDistance = Math.log2(hz / (params.lowFreq ?? 90))
        const midDistance = Math.log2(hz / (params.midFreq ?? 1200))
        const highDistance = Math.log2(hz / (params.highFreq ?? 8000))
        const midWidth = 1 / Math.max(.2, params.midQ ?? .9)
        const gain = (params.lowGain ?? 0) / (1 + Math.exp(lowDistance * 4)) + (params.midGain ?? 0) * Math.exp(-(midDistance ** 2) / (2 * midWidth ** 2)) + (params.highGain ?? 0) / (1 + Math.exp(-highDistance * 4))
        if (x === 0) context.moveTo(x, toY(gain)); else context.lineTo(x, toY(gain))
      }
    }
    context.stroke()
    for (const [index, node] of nodes.entries()) {
      const x = frequencyToX(node.frequency, width)
      const y = toY(node.gain)
      context.beginPath(); context.arc(x, y, 5, 0, Math.PI * 2)
      context.fillStyle = bypassed ? '#53616b' : node.color; context.fill()
      context.strokeStyle = '#0b1116'; context.lineWidth = 1.5; context.stroke()
      context.fillStyle = '#10171c'; context.font = 'bold 6px sans-serif'; context.textAlign = 'center'; context.textBaseline = 'middle'; context.fillText(String(index + 1), x, y + .5)
    }
  }, [params, measured, bypassed, nodes])

  const pointFromEvent = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = event.currentTarget
    const bounds = canvas.getBoundingClientRect()
    return { x: (event.clientX - bounds.left) * canvas.width / bounds.width, y: (event.clientY - bounds.top) * canvas.height / bounds.height }
  }
  const updateNode = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const band = dragging.current
    if (!band) return
    const canvas = event.currentTarget
    const point = pointFromEvent(event)
    const rawFrequency = parameterFrequencyAtX(point.x, canvas.width)
    const [minimum, maximum] = band === 'low' ? [20, 2_000] : band === 'mid' ? [40, 16_000] : [500, 20_000]
    const frequency = Math.max(minimum, Math.min(maximum, rawFrequency))
    const gain = Math.max(-EQ_RANGE_DB, Math.min(EQ_RANGE_DB, (canvas.height / 2 - point.y) / (canvas.height * .46) * EQ_RANGE_DB))
    onParam(`${band}Freq`, Math.round(frequency))
    onParam(`${band}Gain`, Math.round(gain * 10) / 10)
  }
  const beginNodeDrag = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = event.currentTarget
    const point = pointFromEvent(event)
    const closest = nodes.map((node) => ({ id: node.id, distance: Math.hypot(frequencyToX(node.frequency, canvas.width) - point.x, (canvas.height / 2 - node.gain / EQ_RANGE_DB * canvas.height * .46) - point.y) })).sort((a, b) => a.distance - b.distance)[0]
    if (!closest || closest.distance > 15) return
    dragging.current = closest.id
    canvas.setPointerCapture(event.pointerId)
    updateNode(event)
  }
  const endNodeDrag = (event: React.PointerEvent<HTMLCanvasElement>) => {
    dragging.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }

  return <canvas className="eq-display interactive" ref={ref} width="210" height="62" title="EQ 포인트를 드래그해 주파수와 게인을 조절" onPointerDown={beginNodeDrag} onPointerMove={updateNode} onPointerUp={endNodeDrag} onPointerCancel={endNodeDrag} />
}

function DisperserPanel({ effectId, params, bypassed, setParam }: { effectId: string; params: Record<string, number>; bypassed: boolean; setParam(param: string, value: number): void }) {
  const frequency = Math.max(20, Math.min(20_000, params.frequency ?? 3_050))
  const amount = Math.max(0, Math.min(1, params.amount ?? .25))
  const pinch = Math.max(0, Math.min(1, params.pinch ?? .45))
  return <div className="disperser-panel">
    <DisperserDisplay effectId={effectId} frequency={frequency} amount={amount} pinch={pinch} bypassed={bypassed} onFrequency={(value) => setParam('frequency', value)} />
    <div className="parameter-row disperser-controls">
      <Knob value={frequency} min={20} max={20_000} step={1} scale="log" defaultValue={3_050} label="FREQUENCY" format={hzFormat} onChange={(value) => setParam('frequency', value)} />
      <Knob value={amount} min={0} max={1} step={1 / 64} defaultValue={.25} label="AMOUNT" format={(value) => `${Math.round(value * 64)} STG`} onChange={(value) => setParam('amount', value)} />
      <Knob value={pinch} min={0} max={1} step={.01} defaultValue={.45} label="PINCH" format={percentFormat} onChange={(value) => setParam('pinch', value)} />
    </div>
    <div className="disperser-quality">64× BIQUAD ALL-PASS <span>·</span> UNITY MAGNITUDE</div>
  </div>
}

function DisperserDisplay({ effectId, frequency, amount, pinch, bypassed, onFrequency }: { effectId: string; frequency: number; amount: number; pinch: number; bypassed: boolean; onFrequency(value: number): void }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const engine = useEngine()
  useEffect(() => {
    const draw = () => {
    const canvas = ref.current
    const context = canvas?.getContext('2d')
    if (!canvas || !context) return
    const { width, height } = canvas
    const pad = 10
    const graphBottom = height - 17
    context.clearRect(0, 0, width, height)
    const background = context.createLinearGradient(0, 0, 0, height)
    background.addColorStop(0, '#211b20')
    background.addColorStop(1, '#0f1418')
    context.fillStyle = background
    context.fillRect(0, 0, width, height)
    drawFrequencyGrid(context, width, graphBottom, '#3e353a')
    context.strokeStyle = '#3e353a'
    context.lineWidth = 1
    for (let row = 1; row < 4; row += 1) {
      const y = row * graphBottom / 4
      context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke()
    }
    drawEffectSpectrum(context, engine.getEffectSpectrum(effectId), width, graphBottom, bypassed ? '#766b70' : '#a35a68')

    const stages = Math.round(amount * 64)
    const q = .12 * 100 ** pinch
    const samples = Array.from({ length: 181 }, (_, index) => {
      const x = index / 180 * width
      const hz = displayFrequencyAtX(x, width)
      return { x, delay: disperserGroupDelay(hz, frequency, q, stages) }
    })
    const maxDelay = Math.max(1, ...samples.map((sample) => sample.delay))
    const fullAmountDelay = stages > 0 ? maxDelay / stages * 64 : 1
    const points = samples.map((sample) => ({ x: sample.x, y: graphBottom - pad - sample.delay / fullAmountDelay * (graphBottom - pad * 2) }))
    context.beginPath(); context.moveTo(points[0]!.x, graphBottom)
    for (const point of points) context.lineTo(point.x, point.y)
    context.lineTo(points.at(-1)!.x, graphBottom); context.closePath()
    const fill = context.createLinearGradient(0, pad, 0, graphBottom)
    fill.addColorStop(0, bypassed ? '#6e656922' : '#ed33474a')
    fill.addColorStop(1, '#42182008')
    context.fillStyle = fill
    context.fill()
    context.beginPath()
    for (const [index, point] of points.entries()) { if (index === 0) context.moveTo(point.x, point.y); else context.lineTo(point.x, point.y) }
    context.strokeStyle = bypassed ? '#70676b' : '#ef394b'
    context.lineWidth = 2
    context.shadowColor = bypassed ? 'transparent' : '#ef394b'
    context.shadowBlur = bypassed ? 0 : 5
    context.stroke(); context.shadowBlur = 0

    const frequencyX = frequencyToX(frequency, width)
    context.strokeStyle = bypassed ? '#777' : '#ff5260'
    context.lineWidth = 1
    context.beginPath(); context.moveTo(frequencyX, 0); context.lineTo(frequencyX, graphBottom); context.stroke()
    context.fillStyle = bypassed ? '#777' : '#ff5260'
    context.beginPath(); context.moveTo(frequencyX - 6, 0); context.lineTo(frequencyX + 6, 0); context.lineTo(frequencyX, 7); context.closePath(); context.fill()
    context.font = '8px monospace'
    context.textAlign = 'left'
    context.fillStyle = '#c8bfc2'
    context.fillText(`${hzFormat(frequency)}  ${frequencyNote(frequency)}`, 7, height - 5)
    context.textAlign = 'right'
    context.fillStyle = '#786e73'
    context.fillText(`${stages} STAGES`, width - 7, height - 5)
    }
    draw()
    return subscribeAnalyzerFrame(draw)
  }, [amount, bypassed, effectId, engine, frequency, pinch])

  const updateFrequency = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect()
    const x = (event.clientX - bounds.left) / bounds.width * event.currentTarget.width
    onFrequency(Math.round(parameterFrequencyAtX(x, event.currentTarget.width)))
  }
  const beginFrequencyDrag = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    updateFrequency(event)
  }
  const moveFrequency = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) updateFrequency(event)
  }
  const endFrequencyDrag = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }
  return <canvas ref={ref} className="disperser-display" width="286" height="108" title="Drag horizontally to set the all-pass center frequency" onPointerDown={beginFrequencyDrag} onPointerMove={moveFrequency} onPointerUp={endFrequencyDrag} onPointerCancel={endFrequencyDrag} />
}

function disperserGroupDelay(hz: number, center: number, q: number, stages: number): number {
  if (stages === 0) return 0
  const sampleRate = 48_000
  const omega0 = 2 * Math.PI * Math.min(center, sampleRate * .45) / sampleRate
  const alpha = Math.sin(omega0) / (2 * Math.max(.01, q))
  const a0 = 1 + alpha
  const b0 = (1 - alpha) / a0
  const b1 = -2 * Math.cos(omega0) / a0
  const a1 = b1
  const a2 = b0
  const phase = (omega: number) => {
    const cos1 = Math.cos(omega); const sin1 = Math.sin(omega)
    const cos2 = Math.cos(2 * omega); const sin2 = Math.sin(2 * omega)
    const numeratorReal = b0 + b1 * cos1 + cos2
    const numeratorImag = -b1 * sin1 - sin2
    const denominatorReal = 1 + a1 * cos1 + a2 * cos2
    const denominatorImag = -a1 * sin1 - a2 * sin2
    return Math.atan2(numeratorImag, numeratorReal) - Math.atan2(denominatorImag, denominatorReal)
  }
  const omega = 2 * Math.PI * hz / sampleRate
  const epsilon = 1e-4
  const delta = phase(omega + epsilon) - phase(Math.max(0, omega - epsilon))
  const wrappedDelta = Math.atan2(Math.sin(delta), Math.cos(delta))
  return Math.max(0, -wrappedDelta / (2 * epsilon) * stages)
}

function frequencyNote(frequency: number): string {
  const note = Math.round(69 + 12 * Math.log2(frequency / 440))
  const names = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']
  return `${names[((note % 12) + 12) % 12]}${Math.floor(note / 12) - 1}`
}

function ShaperDisplay({ params, bypassed, onCurve }: { params: Record<string, number>; bypassed: boolean; onCurve(curve: number): void }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const curve = Math.max(0, Math.min(CURVE_OPTIONS.length - 1, Math.round(params.curve ?? 0)))
  const driveDb = params.driveDb ?? 6
  const mix = Math.max(0, Math.min(1, params.mix ?? 1))

  useEffect(() => {
    const canvas = ref.current
    const context = canvas?.getContext('2d')
    if (!canvas || !context) return
    const { width, height } = canvas
    const pad = 9
    const graphWidth = width - pad * 2
    const graphHeight = height - pad * 2
    const xToCanvas = (value: number) => pad + (value + 1) * 0.5 * graphWidth
    const yToCanvas = (value: number) => pad + (1 - (value + 1) * 0.5) * graphHeight
    const drive = 10 ** (driveDb / 20)

    context.clearRect(0, 0, width, height)
    const background = context.createLinearGradient(0, 0, 0, height)
    background.addColorStop(0, '#18232b')
    background.addColorStop(1, '#0d1419')
    context.fillStyle = background
    context.fillRect(0, 0, width, height)
    context.strokeStyle = '#33414c'
    context.lineWidth = 1
    for (const value of [-1, -.5, 0, .5, 1]) {
      context.beginPath(); context.moveTo(xToCanvas(value), pad); context.lineTo(xToCanvas(value), height - pad); context.stroke()
      context.beginPath(); context.moveTo(pad, yToCanvas(value)); context.lineTo(width - pad, yToCanvas(value)); context.stroke()
    }
    context.setLineDash([3, 3])
    context.strokeStyle = '#65758166'
    context.beginPath(); context.moveTo(xToCanvas(-1), yToCanvas(-1)); context.lineTo(xToCanvas(1), yToCanvas(1)); context.stroke()
    context.setLineDash([])

    const transfer = Array.from({ length: 161 }, (_, index) => {
      const x = -1 + index / 80
      const wet = shaperPreview(curve, x * drive)
      return { x, value: x * (1 - mix) + wet * mix }
    })
    const visualPeak = Math.max(.0001, ...transfer.map((sample) => Math.abs(sample.value)))
    const samples: Array<{ x: number; y: number }> = []
    for (let index = 0; index <= 160; index += 1) {
      const sample = transfer[index]!
      samples.push({ x: xToCanvas(sample.x), y: yToCanvas(Math.max(-1.1, Math.min(1.1, sample.value / visualPeak))) })
    }
    context.beginPath()
    context.moveTo(samples[0]!.x, yToCanvas(0))
    for (const sample of samples) context.lineTo(sample.x, sample.y)
    context.lineTo(samples.at(-1)!.x, yToCanvas(0))
    context.closePath()
    const fill = context.createLinearGradient(0, pad, 0, height - pad)
    fill.addColorStop(0, bypassed ? '#59667133' : '#62d7ff30')
    fill.addColorStop(1, '#17303b08')
    context.fillStyle = fill
    context.fill()
    context.beginPath()
    for (const [index, sample] of samples.entries()) { if (index === 0) context.moveTo(sample.x, sample.y); else context.lineTo(sample.x, sample.y) }
    context.strokeStyle = bypassed ? '#65727c' : '#68d9ff'
    context.lineWidth = 2
    context.shadowColor = bypassed ? 'transparent' : '#43c8f4'
    context.shadowBlur = bypassed ? 0 : 5
    context.stroke()
    context.shadowBlur = 0
  }, [bypassed, curve, driveDb, mix])

  return <div className="shaper-display"><canvas ref={ref} width="210" height="94" title={`${CURVE_OPTIONS[curve]} transfer curve`} /><div className="shaper-mode-tabs">{CURVE_OPTIONS.map((name, index) => <button key={name} className={curve === index ? 'active' : ''} onClick={() => onCurve(index)}>{name}</button>)}</div></div>
}

function shaperPreview(curve: number, input: number): number {
  if (curve === 1) return Math.max(-1, Math.min(1, input))
  if (curve === 2) return Math.sin(Math.max(-1, Math.min(1, input)) * Math.PI * .5)
  return Math.tanh(input)
}

const pluginRef = (plugin: PluginDescriptor): ExternalPluginRef => ({ format: plugin.format, uid: plugin.uid, name: plugin.name, vendor: plugin.vendor, path: plugin.path, audioInputBuses: plugin.audioInputBuses, audioOutputBuses: plugin.audioOutputBuses, supportsSidechain: plugin.supportsSidechain, hasEditor: plugin.hasEditor, paramCount: plugin.paramCount, parameters: plugin.parameters })

type EffectCatalogEntry = { type: EffectType; description: string; category: string }
type PickerEffect = { key: string; name: string; detail: string; category: string; type: EffectType; plugin?: ExternalPluginRef }

function AddDevice({ onAdd }: { onAdd(type: EffectType, plugin?: ExternalPluginRef): void }) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const buttonRef = useRef<HTMLButtonElement>(null)
  const engine = useEngine()
  const getAnchorElement = useCallback(() => buttonRef.current, [])
  useEffect(() => {
    if (!open || plugins.length || loading) return
    let cancelled = false
    setLoading(true); setError('')
    void scanPluginsOnce(engine).then((result) => { if (!cancelled) setPlugins(result) }).catch((reason) => { if (!cancelled) setError(String(reason)) }).finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [engine, loading, open, plugins.length])
  const entries = useMemo<PickerEffect[]>(() => [
    ...EFFECT_CATALOG.map((effect) => ({ key: effect.type, name: deviceName(effect.type), detail: effect.description, category: effect.category, type: effect.type })),
    ...plugins.filter((plugin) => !plugin.isInstrument).map((plugin) => ({ key: `${plugin.format}:${plugin.uid}:${plugin.path}`, name: plugin.name, detail: `${plugin.format.toUpperCase()} · ${plugin.vendor || 'Unknown vendor'}`, category: pluginEffectCategory(plugin), type: `${plugin.format}:${plugin.uid}` as EffectType, plugin: pluginRef(plugin) })),
  ], [plugins])
  const normalized = query.trim().toLocaleLowerCase()
  const filtered = entries.filter((entry) => !normalized || `${entry.name} ${entry.detail} ${entry.category}`.toLocaleLowerCase().includes(normalized))
  const groups = EFFECT_CATEGORY_ORDER.map((category) => ({ category, items: filtered.filter((entry) => entry.category === category) })).filter((group) => group.items.length)
  const choose = (entry: PickerEffect) => {
    setOpen(false); setQuery('')
    if (!entry.plugin) { onAdd(entry.type); return }
    void hydratePluginRef(engine, entry.plugin, false).then((plugin) => onAdd(entry.type, plugin)).catch((reason) => useProjectStore.getState().showToast(`${entry.name} 파라미터를 불러오지 못했습니다: ${String(reason)}`))
  }
  return <div className="add-device"><button ref={buttonRef} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}><Plus size={18} /><span>이펙트 추가</span></button>{open && <FloatingPanel getAnchorElement={getAnchorElement} onClose={() => { setOpen(false); setQuery('') }} className="device-picker grouped-device-picker"><label className="device-picker-search"><span>⌕</span><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="이펙트 검색" /></label>{normalized ? <div className="device-picker-results">{filtered.map((entry) => <button key={entry.key} onClick={() => choose(entry)}><span>{entry.name}</span><small>{entry.category} · {entry.detail}</small></button>)}{filtered.length === 0 && <small className="device-picker-empty">검색 결과가 없습니다.</small>}</div> : <div className="device-picker-groups">{groups.map((group) => <div className="device-picker-group" data-category={group.category} key={group.category}><button className="device-picker-group-label"><span>{group.category}</span><small>{group.items.length}</small><b>›</b></button><div className="device-picker-submenu"><strong>{group.category.toUpperCase()}</strong>{group.items.map((entry) => <button key={entry.key} onClick={() => choose(entry)}><span>{entry.name}</span><small>{entry.detail}</small></button>)}</div></div>)}</div>}{loading && <small className="device-picker-note">플러그인을 안전하게 검색하는 중…</small>}{error && <small className="plugin-scan-error">검색 실패: {error}</small>}</FloatingPanel>}</div>
}

function pluginEffectCategory(plugin: PluginDescriptor): string {
  const value = `${plugin.category} ${plugin.name}`.toLocaleLowerCase()
  if (/eq|filter/.test(value)) return 'EQ & Filter'
  if (/compress|limit|clip|gate|dynamic/.test(value)) return 'Dynamics'
  if (/delay|echo|reverb|space/.test(value)) return 'Time & Space'
  if (/chorus|flang|phase|tremolo|modulat/.test(value)) return 'Modulation'
  if (/distort|saturat|drive|color|excite/.test(value)) return 'Color & Drive'
  if (/pitch|tune|vocal|vocoder/.test(value)) return 'Pitch & Vocal'
  return 'Utility & Other'
}

function normalizeParameterLabel(label: string): string {
  return label.toLocaleLowerCase().replace(/[^a-z0-9]/g, '')
    .replace('thresh', 'threshold')
    .replace('freq', 'frequency')
    .replace('lowsplit', 'splitlow')
    .replace('highsplit', 'splithigh')
    .replace('velcurve', 'velocitycurve')
    .replace('ingain', 'inputdb')
    .replace('outgain', 'outputdb')
    .replace('drywet', 'mix')
}

function parameterAutomationMenu(track: Track, targetKind: 'effect' | 'instrument', targetId: string, parameterId: string | undefined, label: string, value: number): MenuItem[] {
  const normalizeId = (id: string) => id.replace(/^param:/, '')
  const options = automationOptionsForTrack(track).filter((option) => option.targetKind === targetKind && option.targetId === targetId)
  const normalizedLabel = normalizeParameterLabel(label)
  const candidates = parameterId
    ? options.filter((option) => normalizeId(option.parameterId) === normalizeId(parameterId))
    : options.filter((option) => parameterLabelsMatch(normalizedLabel, normalizeParameterLabel(option.label)))
  const option = candidates.length <= 1 ? candidates[0] : candidates.reduce((closest, candidate) => Math.abs(candidate.defaultValue - value) < Math.abs(closest.defaultValue - value) ? candidate : closest)
  if (!option) return [{ kind: 'item', label: `${label} 파라미터를 찾지 못했습니다.`, disabled: true, run: () => undefined }]
  const lane = (track.automationLanes ?? []).find((candidate) => candidate.targetKind === targetKind && candidate.targetId === targetId && normalizeId(candidate.parameterId) === normalizeId(option.parameterId))
  const store = useProjectStore.getState()
  if (!lane) return [{ kind: 'item', label: `${label} 오토메이션 추가`, run: () => { store.addAutomationLane(track.id, option); store.setTrackAutomationOpen(track.id, true); store.showToast(`${label} 오토메이션을 추가했습니다.`) } }]
  return [
    { kind: 'item', label: `${label} 오토메이션 제거`, danger: true, run: () => { store.removeAutomationLane(track.id, lane.id); store.showToast(`${label} 오토메이션을 제거했습니다.`) } },
    { kind: 'item', label: '트랙에서 보기', run: () => focusAutomationLane(track.id, lane.id) },
  ]
}

function focusAutomationLane(trackId: string, laneId: string): void {
  const store = useProjectStore.getState()
  store.selectTrack(trackId)
  store.setTrackAutomationOpen(trackId, true)
  store.setEditFocus('arrangement')
  requestAnimationFrame(() => requestAnimationFrame(() => {
    const lane = [...document.querySelectorAll<HTMLElement>('[data-automation-lane-id]')].find((element) => element.dataset.automationLaneId === laneId)
    if (!lane) return
    lane.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'smooth' })
    lane.classList.add('automation-lane-focus')
    window.setTimeout(() => lane.classList.remove('automation-lane-focus'), 1400)
  }))
}

function DeviceAutomationModes({ trackId, targetKind, targetId }: { trackId: string; targetKind: 'effect' | 'instrument'; targetId: string }) {
  const allLanes = useProjectStore((state) => state.project.tracks.find((track) => track.id === trackId)?.automationLanes)
  const lanes = useMemo(() => (allLanes ?? []).filter((lane) => lane.targetKind === targetKind && lane.targetId === targetId), [allLanes, targetId, targetKind])
  const setMode = useProjectStore((state) => state.setAutomationLaneMode)
  const active = lanes.length ? lanes[0]?.mode ?? 'read' : 'off'
  return <div className="device-automation-modes" role="group" aria-label="플러그인 오토메이션 모드" title={lanes.length ? `${lanes.length}개 오토메이션 레인` : '연결된 오토메이션 레인 없음'}>{(['off', 'write', 'read', 'latch'] as const).map((mode) => <button key={mode} className={`${mode} ${active === mode ? 'active' : ''}`} disabled={!lanes.length} aria-label={{ off: '오토메이션 끄기', write: '오토메이션 쓰기', read: '오토메이션 읽기', latch: '오토메이션 래치' }[mode]} onClick={() => lanes.forEach((lane) => setMode(trackId, lane.id, mode))} />)}</div>
}

function parameterLabelsMatch(control: string, option: string): boolean {
  return control === option || option.startsWith(control) || control.startsWith(option) || option.includes(control)
}

const dbFormat = (value: number) => `${value > 0 ? '+' : ''}${value.toFixed(1)} dB`
const faderStyle = (value: number) => ({ '--fader-level': `${Math.max(0, Math.min(100, (value + 60) / 72 * 100))}%` } as React.CSSProperties)
const percentFormat = (value: number) => `${Math.round(value * 100)}%`
const msFormat = (value: number) => `${value.toFixed(0)} ms`
const hzFormat = (value: number) => value >= 1000 ? `${(value / 1000).toFixed(2)} kHz` : `${Math.round(value)} Hz`
const CURVE_OPTIONS = ['SOFT CLIP', 'HARD CLIP', 'SINE']
const EFFECT_CATEGORY_ORDER = ['EQ & Filter', 'Dynamics', 'Color & Drive', 'Modulation', 'Pitch & Vocal', 'Time & Space', 'Utility & Other']
const EFFECT_CATALOG: EffectCatalogEntry[] = [
  { type: 'builtin:eq', description: '4-band parametric EQ', category: 'EQ & Filter' }, { type: 'builtin:eq8', description: '8-band parametric EQ', category: 'EQ & Filter' },
  { type: 'builtin:compressor', description: 'Downward dynamics processor', category: 'Dynamics' }, { type: 'builtin:upward-compressor', description: 'Detail recovery below threshold', category: 'Dynamics' }, { type: 'builtin:transient-shaper', description: 'Attack · sustain envelope contouring', category: 'Dynamics' }, { type: 'builtin:multiband-compressor', description: '3-band dynamics · LR4 crossover', category: 'Dynamics' }, { type: 'builtin:mastering-limiter', description: 'True Peak · LUFS · four characters', category: 'Dynamics' }, { type: 'builtin:clipper', description: '4× oversampled peak clipping', category: 'Dynamics' },
  { type: 'builtin:distortion', description: '3-band Tube · Tape · Saturation · Exciter', category: 'Color & Drive' }, { type: 'builtin:waveshaper', description: '3-mode · 4× oversampled shaper', category: 'Color & Drive' }, { type: 'builtin:disperser', description: 'Cascaded all-pass phase dispersion', category: 'Color & Drive' },
  { type: 'builtin:lfo-tremolo', description: 'Volume · pan LFO modulation', category: 'Modulation' }, { type: 'builtin:vocoder', description: '24-band carrier / modulator vocoder', category: 'Modulation' },
  { type: 'builtin:roboter', description: 'Auto-key pitch correction · 5-voice harmonizer', category: 'Pitch & Vocal' },
  { type: 'builtin:resonator', description: 'Harmonic STFT resonator · scale mask', category: 'Pitch & Vocal' },
  { type: 'builtin:formant-shifter', description: 'PSOLA mono / phase-vocoder poly, auto-selected', category: 'Pitch & Vocal' },
  { type: 'builtin:delay', description: 'Stereo echo', category: 'Time & Space' }, { type: 'builtin:reverb', description: 'FDN room reverb', category: 'Time & Space' },
  { type: 'builtin:utility', description: 'Stereo utility · bass mono', category: 'Utility & Other' },
]
function deviceName(type: string, plugin?: ExternalPluginRef): string { return plugin?.name ?? ({ 'builtin:eq': '4band-EQ', 'builtin:eq8': '8band-EQ', 'builtin:utility': 'Utility', 'builtin:compressor': 'Compressor', 'builtin:upward-compressor': 'Upward Compressor', 'builtin:transient-shaper': 'Transient Shaper', 'builtin:multiband-compressor': 'Multiband Compressor', 'builtin:clipper': 'Clipper', 'builtin:distortion': 'Distortion', 'builtin:disperser': 'Disperser', 'builtin:roboter': 'Roboter', 'builtin:resonator': COLORIZER_NAME, 'builtin:formant-shifter': 'Formant Shifter', 'builtin:mastering-limiter': 'Mastering Limiter', 'builtin:vocoder': 'Vocoder', 'builtin:lfo-tremolo': 'LFO Tremolo', 'builtin:delay': 'Echo Space', 'builtin:reverb': 'Room Reverb', 'builtin:waveshaper': 'Drive Shaper' } as Record<string, string>)[type] ?? 'External Plug-in' }
