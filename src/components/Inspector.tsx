// Selected-track inspector modeled after Studio One's left channel pane.
import { ChevronDown, CirclePower, Plus, Radio, SlidersHorizontal, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { EffectInstance, EffectType } from '../engine'
import { COLORIZER_NAME } from '../effects/builtinEffects'
import { useEngine } from '../hooks/useEngine'
import { useProjectStore } from '../store/projectStore'
import { EditableNumber } from './controls'
import { FloatingPanel, MenuPanel, type MenuItem } from './Menu'
import { subscribeBrowserDrag } from './browserPayload'

export function Inspector() {
  const selectedId = useProjectStore((state) => state.selectedTrackId)
  const track = useProjectStore((state) => state.project.tracks.find((candidate) => candidate.id === selectedId))
  const buses = useProjectStore((state) => state.project.buses)
  const updateTrack = useProjectStore((state) => state.updateTrack)
  const remove = useProjectStore((state) => state.removeSelectedTrack)
  const addEffect = useProjectStore((state) => state.addEffect)
  const removeEffect = useProjectStore((state) => state.removeEffect)
  const toggleEffect = useProjectStore((state) => state.toggleEffectBypass)
  const updateSend = useProjectStore((state) => state.updateSend)
  const removeSend = useProjectStore((state) => state.removeSend)
  const setLowerTab = useProjectStore((state) => state.setLowerTab)
  const engine = useEngine()
  const [effectPicker, setEffectPicker] = useState(false)
  const [effectMenu, setEffectMenu] = useState<{ x: number; y: number; effect: EffectInstance } | null>(null)
  const [insertDropActive, setInsertDropActive] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const nameRef = useRef<HTMLInputElement>(null)
  const effectPickerButtonRef = useRef<HTMLButtonElement>(null)
  const insertListRef = useRef<HTMLDivElement>(null)
  const getEffectPickerAnchor = useCallback(() => effectPickerButtonRef.current, [])
  useEffect(() => subscribeBrowserDrag((state) => {
    if (!track || state.payload.kind !== 'effect') return
    const inside = state.type !== 'cancel' && !!insertListRef.current?.contains(document.elementFromPoint(state.x, state.y))
    setInsertDropActive(inside)
    if (state.type === 'drop' && inside) {
      addEffect(track.id, state.payload.type, state.payload.plugin)
      useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id })
      setLowerTab('effects')
    }
  }), [addEffect, setLowerTab, track])
  const effectMenuItems: MenuItem[] = effectMenu && track ? [
    { kind: 'item', label: '이펙트 체인에서 열기', run: () => useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id }) },
    { kind: 'item', label: effectMenu.effect.bypassed ? '바이패스 해제' : '바이패스', checked: effectMenu.effect.bypassed, run: () => toggleEffect(track.id, effectMenu.effect.id) },
    { kind: 'separator' },
    { kind: 'item', label: '이펙터 삭제', danger: true, run: () => removeEffect(track.id, effectMenu.effect.id) },
  ] : []

  if (!track) return <aside className="inspector empty"><SlidersHorizontal size={22} /><p>트랙을 선택하세요</p></aside>

  return (
    <aside className="inspector">
      <div className="inspector-heading"><span>인스펙터</span><button title="트랙 삭제" onClick={remove}><Trash2 size={14} /></button></div>
      <div className="track-identity">
        <input type="color" value={track.color} onChange={(event) => updateTrack(track.id, { color: event.target.value })} aria-label="Track color" />
        <div><input ref={nameRef} className="track-name-input" value={track.name} readOnly={!renaming} title="더블클릭하여 이름 바꾸기" onDoubleClick={() => { setRenaming(true); queueMicrotask(() => { nameRef.current?.focus(); nameRef.current?.select() }) }} onChange={(event) => updateTrack(track.id, { name: event.target.value })} onBlur={() => setRenaming(false)} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === 'Escape') event.currentTarget.blur() }} /><small>{track.kind === 'instrument' ? `인스트루먼트 · ${track.instrument?.type ?? 'Empty'}` : '스테레오 오디오'}</small></div>
      </div>
      <Section title="채널">
        <label className="field-row"><span>볼륨</span><input type="range" min="-60" max="12" step="0.1" value={track.volumeDb} onChange={(event) => { const value = Number(event.target.value); updateTrack(track.id, { volumeDb: value }); engine.setTrackVolume(track.id, value) }} /><EditableNumber value={track.volumeDb} min={-60} max={12} step={0.1} onChange={(value) => { updateTrack(track.id, { volumeDb: value }); engine.setTrackVolume(track.id, value) }} format={(value) => `${value.toFixed(1)} dB`} /></label>
        <label className="field-row"><span>팬</span><input type="range" min="-1" max="1" step="0.01" value={track.pan} onDoubleClick={() => updateTrack(track.id, { pan: 0 })} onChange={(event) => { const value = Number(event.target.value); updateTrack(track.id, { pan: value }); engine.setTrackPan(track.id, value) }} /><EditableNumber value={track.pan} min={-1} max={1} step={0.01} onChange={(value) => { updateTrack(track.id, { pan: value }); engine.setTrackPan(track.id, value) }} format={(value) => value === 0 ? 'C' : `${Math.round(Math.abs(value) * 100)}${value < 0 ? 'L' : 'R'}`} /></label>
        <div className="state-buttons">
          <button className={track.muted ? 'on mute' : ''} onClick={() => { updateTrack(track.id, { muted: !track.muted }); engine.setTrackMute(track.id, !track.muted) }}>M</button>
          <button className={track.solo ? 'on solo' : ''} onClick={() => { updateTrack(track.id, { solo: !track.solo }); engine.setTrackSolo(track.id, !track.solo) }}>S</button>
          <button className={track.armed ? 'on arm' : ''} onClick={() => updateTrack(track.id, { armed: !track.armed })}><Radio size={12} /> REC</button>
        </div>
      </Section>
      <Section title="인서트">
        <div ref={insertListRef} className={`inspector-list ${insertDropActive ? 'insert-drop-active' : ''}`}>
          {track.effects.map((effect) => <button key={effect.id} onClick={() => useProjectStore.getState().setRackTarget({ kind: 'track', id: track.id })} onContextMenu={(event) => { event.preventDefault(); setEffectMenu({ x: event.clientX, y: event.clientY, effect }) }}><CirclePower size={12} className={effect.bypassed ? 'off' : ''} /><span>{effect.plugin?.name ?? effectName(effect.type)}</span><ChevronDown size={12} /></button>)}
          <button ref={effectPickerButtonRef} className="add-row" aria-haspopup="dialog" aria-expanded={effectPicker} onClick={() => setEffectPicker(!effectPicker)}><Plus size={12} />이펙트 추가</button>
          {effectPicker && <FloatingPanel getAnchorElement={getEffectPickerAnchor} onClose={() => setEffectPicker(false)} className="inspector-effect-picker">{EFFECTS.map((effect) => <button key={effect.type} onClick={() => { addEffect(track.id, effect.type); setEffectPicker(false); setLowerTab('effects') }}><strong>{effect.name}</strong><small>{effect.description}</small></button>)}</FloatingPanel>}
          {effectMenu && <MenuPanel items={effectMenuItems} anchor={{ x: effectMenu.x, y: effectMenu.y }} onClose={() => setEffectMenu(null)} />}
        </div>
      </Section>
      <Section title="센드">
        <div className="inspector-list">{track.sends.map((send) => <div className="send-row inspector-send" key={send.id}><span>{buses.find((bus) => bus.id === send.targetBusId)?.name}</span><button onClick={() => removeSend(track.id, send.id)}>×</button><input type="range" min="-60" max="6" step="0.1" value={send.gainDb} onChange={(event) => { const value = Number(event.target.value); updateSend(track.id, send.id, value); engine.setSendLevel(send.id, value) }} /><EditableNumber value={send.gainDb} min={-60} max={6} step={0.1} onChange={(value) => { updateSend(track.id, send.id, value); engine.setSendLevel(send.id, value) }} format={(value) => `${value.toFixed(1)} dB`} /></div>)}</div>
      </Section>
    </aside>
  )
}

const EFFECTS: Array<{ type: EffectType; name: string; description: string }> = [{ type: 'builtin:eq', name: '4band-EQ', description: '4-band parametric EQ' }, { type: 'builtin:eq8', name: '8band-EQ', description: '8-band parametric EQ' }, { type: 'builtin:utility', name: 'Utility', description: 'Stereo imaging · gain · bass mono' }, { type: 'builtin:compressor', name: 'Compressor', description: '내장 dynamics DSP' }, { type: 'builtin:upward-compressor', name: 'Upward Compressor', description: 'Low-level detail recovery' }, { type: 'builtin:multiband-compressor', name: 'Multiband Compressor', description: '3-band dynamics · LR4 crossover' }, { type: 'builtin:clipper', name: 'Clipper', description: '4× oversampled peak clipping' }, { type: 'builtin:distortion', name: 'Distortion', description: '3-band Tube · Tape · Saturation · Exciter' }, { type: 'builtin:disperser', name: 'Disperser', description: '다단 all-pass 위상 분산 DSP' }, { type: 'builtin:roboter', name: 'Roboter', description: 'Auto-key pitch correction · harmonizer' }, { type: 'builtin:resonator', name: COLORIZER_NAME, description: 'Harmonic spectral resonator' }, { type: 'builtin:mastering-limiter', name: 'Mastering Limiter', description: 'True Peak · LUFS mastering limiter' }, { type: 'builtin:vocoder', name: 'Vocoder', description: 'Sidechain · oscillator filter-bank vocoder' }, { type: 'builtin:lfo-tremolo', name: 'LFO Tremolo', description: 'Volume · pan LFO modulation' }, { type: 'builtin:delay', name: 'Echo Space', description: '내장 stereo delay' }, { type: 'builtin:reverb', name: 'Room Reverb', description: '내장 FDN reverb' }, { type: 'builtin:waveshaper', name: 'Drive Shaper', description: 'Soft · Hard · Sine / 4× OS' }]

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return <section className="inspector-section"><h3>{title}<ChevronDown size={12} /></h3>{children}</section>
}

function effectName(type: string): string {
  return ({ 'builtin:eq': '4band-EQ', 'builtin:eq8': '8band-EQ', 'builtin:utility': 'Utility', 'builtin:compressor': 'Compressor', 'builtin:upward-compressor': 'Upward Compressor', 'builtin:multiband-compressor': 'Multiband Compressor', 'builtin:clipper': 'Clipper', 'builtin:distortion': 'Distortion', 'builtin:disperser': 'Disperser', 'builtin:roboter': 'Roboter', 'builtin:resonator': COLORIZER_NAME, 'builtin:mastering-limiter': 'Mastering Limiter', 'builtin:vocoder': 'Vocoder', 'builtin:lfo-tremolo': 'LFO Tremolo', 'builtin:delay': 'Analog Delay', 'builtin:reverb': 'Room Reverb', 'builtin:waveshaper': 'Drive Shaper' } as Record<string, string>)[type] ?? 'External Plug-in'
}
