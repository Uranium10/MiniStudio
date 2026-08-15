import { invoke } from '@tauri-apps/api/core'
import { emitTo, listen } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { CirclePower, FolderOpen, Pin, Save, SlidersHorizontal, Waves } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import {
  PLUGIN_SHELL_ACTION,
  PLUGIN_SHELL_CONNECTION,
  PLUGIN_SHELL_PIN_CHANGED,
  PLUGIN_SHELL_REACTIVATE,
  PLUGIN_SHELL_REQUEST,
  PLUGIN_SHELL_STATE,
  type PluginShellAction,
  type PluginShellState,
  type PluginShellPinChanged,
  type PluginShellTarget,
} from '../plugins/shellBridge'
import './PluginEditorShell.css'

type ConnectionState = {
  status: 'connecting' | 'ready' | 'failed'
  error?: string
}

export function PluginEditorShell() {
  const target = useMemo<PluginShellTarget>(() => {
    const query = new URLSearchParams(window.location.search)
    return {
      targetKind: query.get('targetKind') === 'effect' ? 'effect' : 'instrument',
      targetId: query.get('targetId') ?? '',
      windowLabel: query.get('windowLabel') ?? getCurrentWebviewWindow().label,
    }
  }, [])
  const [state, setState] = useState<PluginShellState | null>(null)
  const [connection, setConnection] = useState<ConnectionState>({ status: 'connecting' })
  const [pinned, setPinned] = useState(false)

  useEffect(() => {
    let disposed = false
    const unlisteners: Array<() => void> = []
    const keep = (unlisten: () => void) => {
      if (disposed) unlisten()
      else unlisteners.push(unlisten)
    }
    const refresh = () => {
      if (disposed) return
      setConnection({ status: 'connecting' })
      void emitTo('main', PLUGIN_SHELL_REQUEST, target)
      // The native view can finish attaching before this webview registers its
      // event listener. Re-check once on mount/reactivation without polling.
      void invoke<boolean>('engine_plugin_editor_is_open', {
        targetKind: target.targetKind,
        targetId: target.targetId,
      })
        .then((open) => {
          if (!disposed && open) setConnection({ status: 'ready' })
        })
        .catch(() => undefined)
    }

    void listen<PluginShellState>(PLUGIN_SHELL_STATE, ({ payload }) => {
      if (!disposed && payload.targetId === target.targetId) setState(payload)
    }).then(keep)
    void listen(PLUGIN_SHELL_REACTIVATE, refresh).then(keep)
    void listen<ConnectionState>(PLUGIN_SHELL_CONNECTION, ({ payload }) => {
      if (!disposed) setConnection(payload)
    }).then(keep)

    // Always refresh after listener registration. Newly created shells begin
    // hidden, so relying on `isVisible()` loses the initial ready event.
    refresh()
    const unregister = () => {
      void emitTo('main', PLUGIN_SHELL_ACTION, {
        ...target,
        action: 'unregister',
      } satisfies PluginShellAction)
    }
    window.addEventListener('beforeunload', unregister)
    return () => {
      disposed = true
      unlisteners.splice(0).forEach((unlisten) => unlisten())
      window.removeEventListener('beforeunload', unregister)
      unregister()
    }
  }, [target])

  const action = (
    name: PluginShellAction['action'],
    mode?: PluginShellState['automationMode'],
  ) => {
    void emitTo('main', PLUGIN_SHELL_ACTION, { ...target, action: name, mode } satisfies PluginShellAction)
  }
  const modes = ['off', 'write', 'read', 'latch'] as const
  const togglePinned = () => {
    const next = !pinned
    setPinned(next)
    void invoke('engine_set_plugin_editor_pinned', {
      targetId: target.targetId,
      pinned: next,
    }).catch(() => setPinned(!next))
    void emitTo('main', PLUGIN_SHELL_PIN_CHANGED, {
      ...target,
      pinned: next,
    } satisfies PluginShellPinChanged)
  }

  return <main className="plugin-editor-shell">
    <header className="plugin-shell-toolbar">
      <button
        className={`plugin-shell-power ${state?.bypassed ? '' : 'active'}`}
        title={state?.bypassed ? '플러그인 켜기' : '플러그인 끄기'}
        onClick={() => action('toggle-bypass')}
      ><CirclePower size={16} /></button>
      <div className="plugin-shell-identity">
        <strong>{state?.name ?? 'Plug-in'}</strong>
        <span>{state ? `${state.format} · ${state.vendor || 'Unknown vendor'}` : 'MiniStudio native host'}</span>
      </div>
      <div className="plugin-shell-actions">
        <button className={`plugin-shell-pin ${pinned ? 'active' : ''}`} title={pinned ? '호스팅 창 고정 해제' : '호스팅 창 고정'} onClick={togglePinned}><Pin size={14} /><span>PIN</span></button>
        <button title="프리셋 저장" onClick={() => action('save-preset')}><Save size={14} /><span>SAVE</span></button>
        <button title="프리셋 불러오기" onClick={() => action('load-preset')}><FolderOpen size={14} /><span>LOAD</span></button>
        {state?.hasSidechain && <button title="이펙트 랙의 사이드체인 설정으로 이동" onClick={() => action('show-sidechain')}><Waves size={14} /><span>SIDECHAIN</span></button>}
        <button title="오토메이션을 트랙에서 보기" onClick={() => action('show-automation')}><SlidersHorizontal size={14} /><span>AUTO</span></button>
        <div
          className="plugin-shell-automation"
          title={state?.automationCount ? `${state.automationCount}개 오토메이션 연결됨` : '연결된 오토메이션 없음'}
        >
          {modes.map((mode) => <button
            key={mode}
            disabled={!state?.automationCount}
            className={`${mode} ${state?.automationMode === mode ? 'active' : ''}`}
            aria-label={`오토메이션 ${mode}`}
            onClick={() => action('set-automation-mode', mode)}
          />)}
        </div>
      </div>
    </header>
    <section className={`plugin-shell-native-surface ${connection.status}`}>
      <div>
        {connection.status === 'connecting' && <><span />네이티브 플러그인 화면을 연결하는 중…</>}
        {connection.status === 'ready' && <>네이티브 플러그인 화면이 연결되었습니다.</>}
        {connection.status === 'failed' && <>
          <b>플러그인 화면 연결 실패</b>
          <small>{connection.error ?? '독립 편집기 전환을 확인해 주세요.'}</small>
        </>}
      </div>
    </section>
  </main>
}
