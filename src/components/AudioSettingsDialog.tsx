// Native backend, output device, sample-rate, and buffer-size settings dialog.
import { Activity, AudioLines, RefreshCw, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import type { AudioBackendInfo, AudioDeviceInfo, AudioSettings, MidiInputPortInfo, StreamStatus } from '../engine'
import { useEngine } from '../hooks/useEngine'
import { useProjectStore } from '../store/projectStore'

export function AudioSettingsDialog() {
  const open = useProjectStore((state) => state.audioSettingsOpen)
  const close = useProjectStore((state) => state.setAudioSettingsOpen)
  const showToast = useProjectStore((state) => state.showToast)
  const engine = useEngine()
  const [backends, setBackends] = useState<AudioBackendInfo[]>([])
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([])
  const [settings, setSettings] = useState<AudioSettings | null>(null)
  const [status, setStatus] = useState<StreamStatus>(() => engine.getStreamStatus())
  const [busy, setBusy] = useState(false)
  const [tab, setTab] = useState<'audio' | 'midi'>('audio')
  const [midiPorts, setMidiPorts] = useState<MidiInputPortInfo[]>([])
  const tracks = useProjectStore((state) => state.project.tracks)
  const instrumentTracks = useMemo(() => tracks.filter((track) => track.kind === 'instrument'), [tracks])
  const selectedTrackId = useProjectStore((state) => state.selectedTrackId)
  const [midiTarget, setMidiTarget] = useState('')

  useEffect(() => {
    if (!open) return
    let cancelled = false
    void Promise.all([engine.listAudioBackends(), engine.getAudioSettings(), engine.listMidiInputs()]).then(async ([available, current, ports]) => {
      if (cancelled) return
      setBackends(available); setSettings(current); setMidiPorts(ports); setMidiTarget(instrumentTracks.find((track) => track.id === selectedTrackId)?.id ?? instrumentTracks[0]?.id ?? '')
      setDevices(await engine.listOutputDevices(current.backendId))
    }).catch(() => showToast('오디오 장치 정보를 가져올 수 없습니다'))
    const timer = window.setInterval(() => setStatus(engine.getStreamStatus()), 250)
    return () => { cancelled = true; window.clearInterval(timer) }
  }, [engine, instrumentTracks, open, selectedTrackId, showToast])

  if (!open || !settings) return null
  const selectedDevice = devices.find((device) => device.id === settings.deviceId) ?? devices[0]
  const changeBackend = async (backendId: string) => {
    setBusy(true)
    try {
      const nextDevices = await engine.listOutputDevices(backendId)
      setDevices(nextDevices)
      const device = nextDevices.find((item) => item.isDefault) ?? nextDevices[0]
      setSettings((current) => current && ({ ...current, backendId, deviceId: device?.id ?? 'default', sampleRate: device?.sampleRates[0] ?? 48_000, bufferSize: device?.bufferSizes.find((value) => value >= 256) ?? 256 }))
    } finally { setBusy(false) }
  }
  const apply = async () => {
    setBusy(true)
    try { await engine.setAudioSettings(settings); showToast('오디오 장치를 다시 시작했습니다'); close(false) }
    catch { showToast('선택한 오디오 설정을 적용할 수 없습니다') }
    finally { setBusy(false) }
  }
  const refreshMidi = async () => {
    setBusy(true)
    try { window.dispatchEvent(new Event('ministudio:midi-refresh')); setMidiPorts(await engine.listMidiInputs()) }
    catch { showToast('MIDI 장치를 검색할 수 없습니다') }
    finally { setBusy(false) }
  }

  return <div className="modal-backdrop"><section className="audio-settings-dialog" role="dialog" aria-modal="true" aria-labelledby="audio-settings-title">
    <header><AudioLines size={20} /><div><strong id="audio-settings-title">오디오 설정</strong><small>Rust 네이티브 출력 엔진</small></div><button onClick={() => close(false)}><X size={16} /></button></header>
    <div className="settings-tabs"><button className={tab === 'audio' ? 'active' : ''} onClick={() => setTab('audio')}>오디오</button><button className={tab === 'midi' ? 'active' : ''} onClick={() => setTab('midi')}>MIDI</button></div>
    <div className="audio-settings-body">
      {tab === 'audio' ? <>
      <label><span>호스트 / 백엔드</span><select value={settings.backendId} disabled={busy} onChange={(event) => void changeBackend(event.target.value)}>{backends.map((backend) => <option key={backend.id} value={backend.id}>{backend.name}{backend.asio ? ' · GPLv3 build' : ''}</option>)}</select></label>
      <label><span>출력 장치</span><select value={settings.deviceId} onChange={(event) => setSettings({ ...settings, deviceId: event.target.value })}>{devices.map((device) => <option key={device.id} value={device.id}>{device.name}{device.isDefault ? ' (기본)' : ''}</option>)}</select></label>
      <div className="audio-settings-grid"><label><span>샘플레이트</span><select value={settings.sampleRate} onChange={(event) => setSettings({ ...settings, sampleRate: Number(event.target.value) })}>{(selectedDevice?.sampleRates ?? [44_100,48_000,96_000]).map((rate) => <option key={rate} value={rate}>{rate / 1000} kHz</option>)}</select></label><label><span>버퍼 크기</span><select value={settings.bufferSize} onChange={(event) => setSettings({ ...settings, bufferSize: Number(event.target.value) })}>{(selectedDevice?.bufferSizes ?? [64,128,256,512,1024]).map((size) => <option key={size} value={size}>{size} samples</option>)}</select></label></div>
      <div className={`stream-card ${status.running ? 'running' : 'error'}`}><Activity size={17} /><div><strong>{status.running ? 'STREAM RUNNING' : 'STREAM OFFLINE'}</strong><span>{status.error ?? `${status.latencyMs.toFixed(2)} ms latency · ${status.xruns} xruns · PDC ${status.pdcSamples} samples`}</span></div><RefreshCw size={13} className={busy ? 'spin' : ''} /></div>
      </> : <><div className="midi-settings-tools"><span>장치 변경은 OS 알림으로 감지합니다.</span><button disabled={busy} onClick={() => void refreshMidi()}><RefreshCw size={12} /> 수동 검색</button></div><label><span>수동 모니터링 대상</span><select value={midiTarget} onChange={(event) => setMidiTarget(event.target.value)}>{instrumentTracks.map((track) => <option key={track.id} value={track.id}>{track.name}</option>)}</select></label><div className="midi-port-list">{midiPorts.length ? midiPorts.map((port) => <div key={port.id}><span><strong>{port.name}</strong><small>{port.connected ? `CONNECTED · ${instrumentTracks.find((track) => track.id === port.targetTrackId)?.name ?? 'AUTO'}` : 'AVAILABLE'}</small></span><button disabled={!midiTarget || busy} className={port.connected ? 'connected' : ''} onClick={() => { setBusy(true); const action = port.connected ? engine.disconnectMidiInput(port.id) : engine.connectMidiInput(port.id, midiTarget); void action.then(() => engine.listMidiInputs()).then(setMidiPorts).catch(() => showToast('MIDI 포트를 열 수 없습니다')).finally(() => setBusy(false)) }}>{port.connected ? '연결 해제' : '연결'}</button></div>) : <p>사용 가능한 MIDI 입력 포트가 없습니다.</p>}</div><small className="midi-hint">선택한 인스트루먼트 트랙이 자동 모니터링 대상입니다. 선택이 오디오 트랙이면 암된 인스트루먼트로 폴백합니다.</small></>}
    </div>
    <footer><span>설정 변경 시 오디오 스트림이 다시 시작됩니다.</span><div><button onClick={() => close(false)}>취소</button><button className="primary" disabled={busy} onClick={() => void apply()}>적용</button></div></footer>
  </section></div>
}
