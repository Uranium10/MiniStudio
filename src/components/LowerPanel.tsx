// Resizable Ableton-inspired mixer, device rack, and phase-two plug-in pane.
import { ChevronDown, CirclePower, Copy, GripVertical, Layers3, Plus, Power, Trash2, X } from 'lucide-react'
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { Bus, EffectInstance, EffectType, ExternalPluginRef, PluginDescriptor, Track } from '../engine'
import { effectiveMasterGainDb, MUTE_GAIN_DB } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { scanPluginsOnce } from '../plugins/scan'
import { useProjectStore, type LowerTab, type RackTarget } from '../store/projectStore'
import { EditableNumber, Knob, LevelMeter, subscribeMeterFrame } from './controls'
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
  const faderStart = useRef<{ source: number; values: Map<string, number> } | null>(null)
  const engine = useEngine()
  const focusSingle = () => selectTrack(track.id)
  const openFx = () => { focusSingle(); useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id }) }
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
    { kind: 'item', label: '선택 트랙으로 버스 채널 생성', icon: <Layers3 size={13} />, disabled: selectedTrackIds.length < 2, run: () => { createBus() } },
  ] : []
  return (
    <article className={`channel-strip ${selected ? 'selected' : ''}`} onClick={(event) => selectTrack(track.id, event.shiftKey)} onContextMenu={(event) => { event.preventDefault(); if (!selected) selectTrack(track.id); setMenu({ x: event.clientX, y: event.clientY }) }}>
      <div className="channel-color" style={{ background: track.color }} />
      <div className="channel-title"><span>{String(index + 1).padStart(2, '0')}</span><input value={track.name} onChange={(event) => updateTrack(track.id, { name: event.target.value })} /><button className="channel-fx-button" title="이펙트 체인 열기" onClick={(event) => { event.stopPropagation(); openFx() }}>FX</button></div>
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
    <article className={`channel-strip bus-strip ${selected ? 'selected' : ''}`} onClick={() => useProjectStore.getState().selectTrack(null)}>
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
  const update = useProjectStore((state) => state.updateMasterVolume)
  const toggleMute = useProjectStore((state) => state.toggleMasterMute)
  const toggleDim = useProjectStore((state) => state.toggleMasterDim)
  const selectRack = useProjectStore((state) => state.setRackTarget)
  const engine = useEngine()
  const applyGain = (value: number) => { update(value); engine.setMasterVolume(effectiveMasterGainDb({ ...master, volumeDb: value })) }
  return (
    <article className="channel-strip master-strip" onClick={() => useProjectStore.getState().selectTrack(null)}>
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
  const clearTrackSelection = useProjectStore((state) => state.selectTrack)
  const target = useProjectStore((state) => state.rackTarget)
  const tracks = useProjectStore((state) => state.project.tracks)
  const buses = useProjectStore((state) => state.project.buses)
  const master = useProjectStore((state) => state.project.master)
  const addEffect = useProjectStore((state) => state.addTargetEffect)
  const removeEffect = useProjectStore((state) => state.removeTargetEffect)
  const toggleBypass = useProjectStore((state) => state.toggleTargetEffect)
  const reorder = useProjectStore((state) => state.reorderTargetEffect)
  const [menu, setMenu] = useState<{ x: number; y: number; effect: EffectInstance } | null>(null)
  const effects = target.kind === 'track' ? tracks.find((item) => item.id === target.id)?.effects : target.kind === 'bus' ? buses.find((item) => item.id === target.id)?.effects : master.effects
  const targetTrack = target.kind === 'track' ? tracks.find((item) => item.id === target.id) : undefined
  if (!effects) return <div className="rack-empty">믹서 채널을 선택하면 디바이스 체인이 표시됩니다.</div>
  const name = target.kind === 'track' ? tracks.find((item) => item.id === target.id)?.name ?? 'Track' : target.kind === 'bus' ? buses.find((item) => item.id === target.id)?.name ?? 'Bus' : 'MASTER FX'
  const color = target.kind === 'track' ? tracks.find((item) => item.id === target.id)?.color ?? '#55a7ff' : target.kind === 'bus' ? '#c7954c' : '#5ac8e8'
  const contextItems: MenuItem[] = menu ? [
    { kind: 'item', label: '바이패스', icon: <Power size={13} />, run: () => toggleBypass(target, menu.effect.id) },
    { kind: 'item', label: '복제', icon: <Copy size={13} />, run: () => addEffect(target, menu.effect.type, menu.effect.plugin) },
    { kind: 'separator' },
    { kind: 'item', label: '삭제', icon: <Trash2 size={13} />, danger: true, run: () => target.kind === 'track' ? useProjectStore.getState().removeEffect(target.id, menu.effect.id) : removeEffect(target, menu.effect.id) },
  ] : []
  return (
    <div className="device-rack" onPointerDownCapture={() => clearTrackSelection(null)} onClick={() => setMenu(null)}>
      <div className={`rack-track ${target.kind === 'master' ? 'master-rack-target' : ''}`}><span style={{ background: color }} /><strong>{name}</strong><small>{target.kind === 'master' ? 'Master insert chain' : target.kind === 'bus' ? 'Return bus effects' : 'Audio effects'}</small></div>
      <div className="device-chain">
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
  const update = useProjectStore((state) => state.updateTargetEffect)
  const toggle = useProjectStore((state) => state.toggleTargetEffect)
  const showToast = useProjectStore((state) => state.showToast)
  const engine = useEngine()
  const setParam = (param: string, value: number) => { update(target, effect.id, { [param]: value }); engine.setEffectParam(effect.id, param, value) }
  if (collapsed) return <div className={`device-collapsed ${effect.bypassed ? 'bypassed' : ''}`} data-effect-index={index}><button className="collapsed-grip" onPointerDown={(event) => beginPointerReorder(event, { itemSelector: '[data-effect-index]', indexAttribute: 'data-effect-index', axis: 'horizontal', scrollSelector: '.device-chain', onCommit: onReorder })} title="드래그하여 체인 순서 변경"><GripVertical size={12} /></button><button className="collapsed-open" onClick={() => setCollapsed(false)} title="펼치기"><span>{deviceName(effect.type, effect.plugin)}</span></button></div>
  return (
    // Only the grip starts reordering. Window-level pointer tracking keeps the
    // gesture alive outside this card and avoids Tauri's native file-drag path.
    <article className={`device-card ${effect.type === 'builtin:multiband-compressor' ? 'multiband-card' : ''} ${effect.type === 'builtin:eq8' ? 'eq8-card' : ''} ${effect.bypassed ? 'bypassed' : ''}`} data-effect-index={index} onDoubleClick={() => { if (effect.plugin) showToast(`${effect.plugin.name}: 네이티브 편집기 창 연결은 다음 호스팅 단계에서 제공됩니다.`) }} onContextMenu={onContextMenu}>
      <header><span className="device-grip" onPointerDown={(event) => beginPointerReorder(event, { itemSelector: '[data-effect-index]', indexAttribute: 'data-effect-index', axis: 'horizontal', scrollSelector: '.device-chain', onCommit: onReorder })} title="드래그하여 체인 순서 변경"><GripVertical size={13} /></span><button className={effect.bypassed ? '' : 'powered'} title={effect.bypassed ? '바이패스 해제' : '바이패스'} onClick={() => toggle(target, effect.id)}><CirclePower size={14} /></button><strong>{deviceName(effect.type, effect.plugin)}</strong><span>{effect.plugin?.format.toUpperCase() ?? effect.type.replace('builtin:', '').toUpperCase()}</span><button onClick={() => setCollapsed(true)} title="접기"><ChevronDown size={13} /></button></header>
      <div className="device-body">
        {(effect.type === 'builtin:eq' || effect.type === 'builtin:eq8') && <EqPanel effectId={effect.id} params={effect.params} bypassed={effect.bypassed} bandCount={effect.type === 'builtin:eq8' ? 8 : 4} setParam={setParam} />}
        {(effect.type === 'builtin:compressor' || effect.plugin?.supportsSidechain || (effect.plugin?.audioInputBuses ?? 0) > 1) && <SidechainControl effect={effect} target={target} />}
        {effect.type === 'builtin:compressor' && <><div className="parameter-row"><Knob value={effect.params.threshold ?? -18} min={-60} max={0} step={0.1} defaultValue={-18} label="THRESH" format={dbFormat} onChange={(value) => setParam('threshold', value)} /><Knob value={effect.params.ratio ?? 3} min={1} max={20} step={0.1} defaultValue={3} label="RATIO" format={(v) => `${v.toFixed(1)}:1`} onChange={(value) => setParam('ratio', value)} /><Knob value={(effect.params.attack ?? .01) * 1000} min={1} max={500} step={1} defaultValue={10} label="ATTACK" format={msFormat} onChange={(value) => setParam('attack', value / 1000)} /><Knob value={(effect.params.release ?? .2) * 1000} min={10} max={1000} step={1} defaultValue={200} label="RELEASE" format={msFormat} onChange={(value) => setParam('release', value / 1000)} /></div><div className="parameter-row"><Knob value={effect.params.knee ?? 12} min={0} max={24} step={0.5} defaultValue={12} label="KNEE" format={dbFormat} onChange={(value) => setParam('knee', value)} /><Knob value={effect.params.makeupDb ?? 0} min={-12} max={24} step={0.1} defaultValue={0} label="MAKEUP" format={dbFormat} onChange={(value) => setParam('makeupDb', value)} /></div></>}
        {effect.type === 'builtin:multiband-compressor' && <MultibandCompressorPanel effectId={effect.id} params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:utility' && <UtilityPanel params={effect.params} setParam={setParam} />}
        {effect.type === 'builtin:distortion' && <DistortionPanel effectId={effect.id} params={effect.params} bypassed={effect.bypassed} setParam={setParam} />}
        {effect.type === 'builtin:disperser' && <DisperserPanel effectId={effect.id} params={effect.params} bypassed={effect.bypassed} setParam={setParam} />}
        {effect.type === 'builtin:delay' && <><div className="delay-display"><i /><i /><i /><i /><i /></div><div className="parameter-row"><Knob value={effect.params.time ?? .25} min={.01} max={2} step={.01} defaultValue={.25} label="TIME" format={(v) => `${v.toFixed(2)} s`} onChange={(value) => setParam('time', value)} /><Knob value={effect.params.feedback ?? .3} min={0} max={.95} step={.01} defaultValue={.3} label="FEEDBACK" format={percentFormat} onChange={(value) => setParam('feedback', value)} /><Knob value={effect.params.damping ?? .35} min={.01} max={1} step={.01} defaultValue={.35} label="DAMPING" format={percentFormat} onChange={(value) => setParam('damping', value)} /><Knob value={effect.params.mix ?? .25} min={0} max={1} step={.01} defaultValue={.25} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div><ToggleRow label="PING PONG" on={(effect.params.pingPong ?? 0) >= 0.5} onToggle={(on) => setParam('pingPong', on ? 1 : 0)} /></>}
        {effect.type === 'builtin:reverb' && <><div className="reverb-display"><span /><span /><span /><span /></div><div className="parameter-row"><Knob value={effect.params.decaySec ?? 2.4} min={.1} max={20} step={.1} defaultValue={2.4} label="DECAY" format={(v) => `${v.toFixed(1)} s`} onChange={(value) => setParam('decaySec', value)} /><Knob value={effect.params.damping ?? .4} min={0} max={1} step={.01} defaultValue={.4} label="DAMPING" format={percentFormat} onChange={(value) => setParam('damping', value)} /><Knob value={effect.params.width ?? .8} min={0} max={1} step={.01} defaultValue={.8} label="WIDTH" format={percentFormat} onChange={(value) => setParam('width', value)} /><Knob value={effect.params.diffusion ?? .7} min={0} max={.92} step={.01} defaultValue={.7} label="DIFFUSE" format={percentFormat} onChange={(value) => setParam('diffusion', value)} /><Knob value={effect.params.mix ?? .25} min={0} max={1} step={.01} defaultValue={.25} label="DRY / WET" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div></>}
        {effect.type === 'builtin:waveshaper' && <><ShaperDisplay params={effect.params} bypassed={effect.bypassed} onCurve={(curve) => setParam('curve', curve)} /><div className="parameter-row shaper-controls"><Knob value={effect.params.driveDb ?? 6} min={0} max={36} step={.1} defaultValue={6} label="DRIVE" format={dbFormat} onChange={(value) => setParam('driveDb', value)} /><Knob value={effect.params.mix ?? 1} min={0} max={1} step={.01} defaultValue={1} label="MIX" format={percentFormat} onChange={(value) => setParam('mix', value)} /></div><div className="shaper-quality">4× OVERSAMPLING <span>·</span> DC FILTER</div></>}
        {effect.plugin && <div className="plugin-device-placeholder"><Power size={24} /><strong>{effect.plugin.name}</strong><span>{effect.plugin.format.toUpperCase()} · {effect.plugin.audioInputBuses || 0} IN BUS / {effect.plugin.audioOutputBuses || 0} OUT BUS · {effect.plugin.vendor || 'Unknown vendor'}</span></div>}
      </div>
    </article>
  )
}

function ToggleRow({ label, on, onToggle }: { label: string; on: boolean; onToggle(on: boolean): void }) {
  return <div className="toggle-row"><button className={on ? 'active' : ''} role="switch" aria-checked={on} onClick={() => onToggle(!on)}><i />{label}</button></div>
}

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
  return <div className="distortion-panel" style={{ '--distortion-color': color } as React.CSSProperties}>
    <DistortionDisplay effectId={effectId} params={params} bypassed={bypassed} selected={selected} onSelect={setSelected} onParam={setParam} />
    <div className="distortion-controls">
      <label>MODEL<select value={Math.round(params[`${selected}Mode`] ?? 0)} onChange={(event) => setParam(`${selected}Mode`, Number(event.target.value))}>{DISTORTION_MODES.map((mode, index) => <option key={mode} value={index}>{mode}</option>)}</select></label>
      <Knob value={params[`${selected}GainDb`] ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="GAIN" format={dbFormat} onChange={(value) => setParam(`${selected}GainDb`, value)} />
      <Knob value={params[`${selected}DriveDb`] ?? 6} min={0} max={36} step={.1} defaultValue={6} label="DRIVE" format={dbFormat} onChange={(value) => setParam(`${selected}DriveDb`, value)} />
      <Knob value={params.splitLow ?? 180} min={20} max={Math.max(21, (params.splitHigh ?? 4500) / 1.05)} step={1} scale="log" defaultValue={180} label="LOW FREQ" format={hzFormat} onChange={(value) => setParam('splitLow', value)} />
      <Knob value={params.splitHigh ?? 4500} min={Math.min(19_999, (params.splitLow ?? 180) * 1.05)} max={20_000} step={10} scale="log" defaultValue={4500} label="HIGH FREQ" format={hzFormat} onChange={(value) => setParam('splitHigh', value)} />
      <Knob value={params[`${selected}Mix`] ?? .75} min={0} max={1} step={.01} defaultValue={.75} label="MIX" format={percentFormat} onChange={(value) => setParam(`${selected}Mix`, value)} />
    </div>
    <div className="distortion-tabs">{DISTORTION_BANDS.map((band) => <button key={band.id} className={selected === band.id ? 'active' : ''} style={{ '--band-color': band.color } as React.CSSProperties} onClick={() => setSelected(band.id)}><b>{band.id.toUpperCase()}</b><span>{DISTORTION_MODES[Math.round(params[`${band.id}Mode`] ?? 0)]}</span></button>)}</div>
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
  useEffect(() => { draw(); return subscribeMeterFrame(draw) }, [draw])
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
      <Knob value={threshold} min={-60} max={0} step={.1} defaultValue={-20} label="THRESH" format={dbFormat} onChange={(value) => setParam(`${prefix}Threshold`, value)} />
      <Knob value={ratio} min={1} max={20} step={.1} defaultValue={2} label="RATIO" format={(value) => `${value.toFixed(1)}:1`} onChange={(value) => setParam(`${prefix}Ratio`, value)} />
      <Knob value={attack * 1000} min={1} max={500} step={1} defaultValue={15} label="ATTACK" format={msFormat} onChange={(value) => setParam(`${prefix}Attack`, value / 1000)} />
      <Knob value={release * 1000} min={10} max={1000} step={1} defaultValue={180} label="RELEASE" format={msFormat} onChange={(value) => setParam(`${prefix}Release`, value / 1000)} />
      <Knob value={makeup} min={-12} max={24} step={.1} defaultValue={0} label="MAKEUP" format={dbFormat} onChange={(value) => setParam(`${prefix}MakeupDb`, value)} />
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
  const engine = useEngine()
  const [voices, setVoices] = useState(0)
  const [pickerOpen, setPickerOpen] = useState(false)
  const [plugins, setPlugins] = useState<PluginDescriptor[]>([])
  const [loading, setLoading] = useState(false)
  const pickerRef = useRef<HTMLButtonElement>(null)
  const getPickerAnchor = useCallback(() => pickerRef.current, [])
  useEffect(() => { const timer = window.setInterval(() => setVoices(engine.getActiveVoiceCount(track.id)), 250); return () => window.clearInterval(timer) }, [engine, track.id])
  useEffect(() => {
    if (!pickerOpen || plugins.length || loading) return
    let cancelled = false
    setLoading(true)
    void scanPluginsOnce(engine).then((result) => { if (!cancelled) setPlugins(result.filter((plugin) => plugin.isInstrument)) }).finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [engine, loading, pickerOpen, plugins.length])
  const setParam = (id: string, value: number) => { update(track.id, { [id]: value }); engine.setInstrumentParam(track.id, id, value) }
  return <article className={`device-card instrument-card ${instrument.bypassed ? 'bypassed' : ''}`}>
    <header><span className="device-grip">♪</span><button className={instrument.bypassed ? '' : 'powered'} title={instrument.bypassed ? '인스트루먼트 켜기' : '인스트루먼트 바이패스'} onClick={() => toggleBypass(track.id)}><CirclePower size={14} /></button><strong>{instrument.plugin?.name ?? 'Test Tone'}</strong><span>{instrument.plugin?.format.toUpperCase() ?? `${voices} / ${Math.round(instrument.params.polyphony ?? 16)} voices`}</span><button ref={pickerRef} onClick={() => setPickerOpen((open) => !open)} title="인스트루먼트 선택">▾</button></header>
    <div className="device-body">
      {instrument.plugin && <div className="plugin-device-placeholder"><Power size={24} /><strong>{instrument.plugin.name}</strong><span>{instrument.plugin.vendor || 'External instrument'} · MIDI IN / Stereo OUT</span></div>}
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
    {pickerOpen && <FloatingPanel getAnchorElement={getPickerAnchor} onClose={() => setPickerOpen(false)} className="device-picker"><strong>VST3 / CLAP INSTRUMENTS</strong>{loading && <small>플러그인을 검색하는 중…</small>}{plugins.map((plugin) => <button key={`${plugin.format}:${plugin.uid}:${plugin.path}`} onClick={() => { setPlugin(track.id, pluginRef(plugin)); setPickerOpen(false) }}><span>{plugin.name}</span><small>{plugin.format.toUpperCase()} · {plugin.vendor || plugin.category}</small></button>)}{!loading && plugins.length === 0 && <small>설치된 인스트루먼트를 찾지 못했습니다.</small>}</FloatingPanel>}
  </article>
}

const EQ_RANGE_DB = 18

type EqBandView = { index: number; enabled: boolean; frequency: number; gain: number; q: number; shape: number; slope: number; color: string }
const EQ_COLORS = ['#f2b84b', '#64d6f5', '#70e09a', '#ef8dab', '#ad8cff', '#5de0cf', '#ef995e', '#d6dd68']

function EqPanel({ effectId, params, bypassed, bandCount, setParam }: { effectId: string; params: Record<string, number>; bypassed: boolean; bandCount: 4 | 8; setParam(param: string, value: number): void }) {
  const [selected, setSelected] = useState(0)
  const defaults = useMemo(() => bandCount === 4 ? [80, 400, 2500, 10000] : [30, 100, 300, 800, 2500, 6000, 12000, 16000], [bandCount])
  const bands = useMemo<EqBandView[]>(() => defaults.map((frequency, index) => ({
    index,
    enabled: (params[`band${index}.enabled`] ?? (bandCount === 8 && index === 7 ? 0 : 1)) >= .5,
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
      <Knob value={band.q} min={.2} max={12} step={.01} defaultValue={.71} label="Q" format={(value) => value.toFixed(2)} onChange={(value) => setParam(`band${band.index}.q`, value)} />
      <Knob value={band.gain} min={-18} max={18} step={.1} defaultValue={0} label="GAIN" format={dbFormat} onChange={(value) => setParam(`band${band.index}.gain`, value)} />
      <Knob value={band.frequency} min={20} max={20000} step={1} scale="log" defaultValue={defaults[band.index]!} label="FREQ" format={hzFormat} onChange={(value) => setParam(`band${band.index}.freq`, value)} />
      <div className="eq-filter-selects"><label>SHAPE<select value={band.shape} onChange={(event) => { const shape = Number(event.target.value); setParam(`band${band.index}.type`, shape); if (shape === 3 || shape === 4) setParam(`band${band.index}.slope`, band.slope) }}>{shapes.map((shape) => <option key={shape.value} value={shape.value}>{shape.name}</option>)}</select></label>{(band.shape === 3 || band.shape === 4) && <label>SLOPE<select value={band.slope} onChange={(event) => setParam(`band${band.index}.slope`, Number(event.target.value))}>{[12, 24, 36, 48].map((slope) => <option key={slope} value={slope}>{slope} dB/oct</option>)}</select></label>}</div>
    </div>
    {bandCount === 8 && <div className="eq8-global"><strong>STEREO · 8 BAND</strong><button className={(params.adaptiveQ ?? 1) >= .5 ? 'active' : ''} onClick={() => setParam('adaptiveQ', (params.adaptiveQ ?? 1) >= .5 ? 0 : 1)}>ADAPT Q</button><Knob value={(params.scale ?? 1) * 100} min={0} max={200} step={1} defaultValue={100} label="SCALE" format={(value) => `${Math.round(value)}%`} onChange={(value) => setParam('scale', value / 100)} /><Knob value={params.outputDb ?? 0} min={-24} max={24} step={.1} defaultValue={0} label="OUTPUT" format={dbFormat} onChange={(value) => setParam('outputDb', value)} /></div>}
  </div>
}

function InteractiveEqDisplay({ effectId, bands, selected, bypassed, scale, adaptiveQ, onSelect, onParam }: { effectId: string; bands: EqBandView[]; selected: number; bypassed: boolean; scale: number; adaptiveQ: boolean; onSelect(index: number): void; onParam(param: string, value: number): void }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const dragging = useRef<number | null>(null)
  const dragPreview = useRef<{ index: number; frequency: number; gain: number } | null>(null)
  const engine = useEngine()
  const draw = useCallback(() => {
    const canvas = ref.current; const context = canvas?.getContext('2d'); if (!canvas || !context) return
    const { width, height } = canvas; const toY = (db: number) => height / 2 - Math.max(-EQ_RANGE_DB, Math.min(EQ_RANGE_DB, db)) / EQ_RANGE_DB * height * .46
    context.clearRect(0, 0, width, height); drawFrequencyGrid(context, width, height, '#2b3743')
    context.strokeStyle = '#2b3743'; context.lineWidth = 1
    for (let y = 0; y <= height; y += height / 4) { context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke() }
    drawEffectSpectrum(context, engine.getEffectSpectrum(effectId), width, height, bypassed ? '#69737a' : '#638c9a')
    const preview = dragPreview.current
    const visibleBands = preview ? bands.map((band) => band.index === preview.index ? { ...band, frequency: preview.frequency, gain: preview.gain } : band) : bands
    context.beginPath(); context.strokeStyle = bypassed ? '#56636d' : '#62d6fa'; context.lineWidth = 2
    for (let x = 0; x < width; x += 1) {
      const hz = displayFrequencyAtX(x, width)
      const value = visibleBands.reduce((sum, band) => sum + previewEqBand(band, hz, scale, adaptiveQ), 0)
      if (x === 0) context.moveTo(x, toY(value)); else context.lineTo(x, toY(value))
    }
    context.stroke()
    for (const band of visibleBands) { const x = frequencyToX(band.frequency, width); const y = toY(band.gain); context.beginPath(); context.arc(x, y, band.index === selected ? 6 : 5, 0, Math.PI * 2); context.fillStyle = band.enabled && !bypassed ? band.color : '#64717a'; context.fill(); context.strokeStyle = band.index === selected ? '#fff' : '#10171c'; context.lineWidth = band.index === selected ? 1.5 : 1; context.stroke(); context.fillStyle = '#10171c'; context.font = 'bold 6px sans-serif'; context.textAlign = 'center'; context.textBaseline = 'middle'; context.fillText(String(band.index + 1), x, y) }
  }, [adaptiveQ, bands, bypassed, effectId, engine, scale, selected])
  useEffect(() => { draw(); return subscribeMeterFrame(draw) }, [draw])
  const point = (event: React.PointerEvent<HTMLCanvasElement>) => { const bounds = event.currentTarget.getBoundingClientRect(); return { x: (event.clientX - bounds.left) * event.currentTarget.width / bounds.width, y: (event.clientY - bounds.top) * event.currentTarget.height / bounds.height } }
  const update = (event: React.PointerEvent<HTMLCanvasElement>) => { const index = dragging.current; if (index === null) return; const p = point(event); const frequency = Math.round(parameterFrequencyAtX(p.x, event.currentTarget.width)); const gain = Math.round(Math.max(-18, Math.min(18, (event.currentTarget.height / 2 - p.y) / (event.currentTarget.height * .46) * EQ_RANGE_DB)) * 10) / 10; dragPreview.current = { index, frequency, gain }; draw(); onParam(`band${index}.freq`, frequency); onParam(`band${index}.gain`, gain) }
  const down = (event: React.PointerEvent<HTMLCanvasElement>) => { const p = point(event); const closest = bands.map((band) => ({ index: band.index, distance: Math.hypot(frequencyToX(band.frequency, event.currentTarget.width) - p.x, event.currentTarget.height / 2 - band.gain / EQ_RANGE_DB * event.currentTarget.height * .46 - p.y) })).sort((a, b) => a.distance - b.distance)[0]; if (!closest || closest.distance > 16) return; dragging.current = closest.index; onSelect(closest.index); event.currentTarget.setPointerCapture(event.pointerId); update(event) }
  const up = (event: React.PointerEvent<HTMLCanvasElement>) => { dragging.current = null; dragPreview.current = null; if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId) }
  return <canvas className="eq-display interactive" ref={ref} width="450" height="48" onPointerDown={down} onPointerMove={update} onPointerUp={up} onPointerCancel={up} />
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
    return subscribeMeterFrame(draw)
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
    const compensation = 1 / Math.sqrt(drive)

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

    const samples: Array<{ x: number; y: number }> = []
    for (let index = 0; index <= 160; index += 1) {
      const x = -1 + index / 80
      const wet = shaperPreview(curve, x * drive) * compensation
      samples.push({ x: xToCanvas(x), y: yToCanvas(Math.max(-1.1, Math.min(1.1, x * (1 - mix) + wet * mix))) })
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

const pluginRef = (plugin: PluginDescriptor): ExternalPluginRef => ({ format: plugin.format, uid: plugin.uid, name: plugin.name, vendor: plugin.vendor, path: plugin.path, audioInputBuses: plugin.audioInputBuses, audioOutputBuses: plugin.audioOutputBuses, supportsSidechain: plugin.supportsSidechain, paramCount: plugin.paramCount, parameters: plugin.parameters })

function AddDevice({ onAdd }: { onAdd(type: EffectType, plugin?: ExternalPluginRef): void }) {
  const [open, setOpen] = useState(false)
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
  const effects = plugins.filter((plugin) => !plugin.isInstrument)
  return <div className="add-device"><button ref={buttonRef} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}><Plus size={18} /><span>이펙트 추가</span></button>{open && <FloatingPanel getAnchorElement={getAnchorElement} onClose={() => setOpen(false)} className="device-picker"><strong>BUILT-IN DSP</strong>{EFFECT_CATALOG.map(({ type, description }) => <button key={type} onClick={() => { onAdd(type); setOpen(false) }}><span>{deviceName(type)}</span><small>{description}</small></button>)}<strong>VST3 / CLAP</strong>{loading && <small>플러그인을 안전하게 검색하는 중…</small>}{error && <small className="plugin-scan-error">검색 실패: {error}</small>}{effects.map((plugin) => <button key={`${plugin.format}:${plugin.uid}:${plugin.path}`} onClick={() => { onAdd(`${plugin.format}:${plugin.uid}` as EffectType, pluginRef(plugin)); setOpen(false) }}><span>{plugin.name}</span><small>{plugin.format.toUpperCase()} · {plugin.vendor || plugin.category}</small></button>)}{!loading && !error && effects.length === 0 && <small>설치된 이펙트 플러그인을 찾지 못했습니다.</small>}</FloatingPanel>}</div>
}

const dbFormat = (value: number) => `${value > 0 ? '+' : ''}${value.toFixed(1)} dB`
const faderStyle = (value: number) => ({ '--fader-level': `${Math.max(0, Math.min(100, (value + 60) / 72 * 100))}%` } as React.CSSProperties)
const percentFormat = (value: number) => `${Math.round(value * 100)}%`
const msFormat = (value: number) => `${value.toFixed(0)} ms`
const hzFormat = (value: number) => value >= 1000 ? `${(value / 1000).toFixed(2)} kHz` : `${Math.round(value)} Hz`
const CURVE_OPTIONS = ['SOFT CLIP', 'HARD CLIP', 'SINE']
const EFFECT_CATALOG: Array<{ type: EffectType; description: string }> = [{ type: 'builtin:eq', description: '4-band parametric EQ' }, { type: 'builtin:eq8', description: '8-band parametric EQ' }, { type: 'builtin:utility', description: 'Stereo utility · bass mono' }, { type: 'builtin:compressor', description: 'Dynamics processor' }, { type: 'builtin:multiband-compressor', description: '3-band dynamics · LR4 crossover' }, { type: 'builtin:distortion', description: '3-band Tube · Tape · Saturation · Exciter' }, { type: 'builtin:disperser', description: 'Cascaded all-pass phase dispersion' }, { type: 'builtin:delay', description: 'Stereo echo' }, { type: 'builtin:reverb', description: 'FDN room reverb' }, { type: 'builtin:waveshaper', description: '3-mode · 4× oversampled shaper' }]
function deviceName(type: string, plugin?: ExternalPluginRef): string { return plugin?.name ?? ({ 'builtin:eq': '4band-EQ', 'builtin:eq8': '8band-EQ', 'builtin:utility': 'Utility', 'builtin:compressor': 'Compressor', 'builtin:multiband-compressor': 'Multiband Compressor', 'builtin:distortion': 'Distortion', 'builtin:disperser': 'Disperser', 'builtin:delay': 'Echo Space', 'builtin:reverb': 'Room Reverb', 'builtin:waveshaper': 'Drive Shaper' } as Record<string, string>)[type] ?? 'External Plug-in' }
