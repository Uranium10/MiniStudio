import { invoke } from '@tauri-apps/api/core'
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import type { EffectInstance, ExternalPluginRef, IAudioEngine, ProjectState } from '../engine'
import { openDevicePreset, saveDevicePreset } from '../io/projectFiles'
import { useProjectStore, type RackTarget } from '../store/projectStore'
import { PLUGIN_EDITOR_FOREGROUND_PENDING, pluginEditorGraphSyncPolicy, type PluginEditorForegroundIntent } from './editor'

export const PLUGIN_SHELL_REQUEST = 'ministudio:plugin-shell-request'
export const PLUGIN_SHELL_STATE = 'ministudio:plugin-shell-state'
export const PLUGIN_SHELL_ACTION = 'ministudio:plugin-shell-action'
export const PLUGIN_SHELL_CLOSED = 'ministudio:plugin-shell-closed'
export const PLUGIN_SHELL_REACTIVATE = 'ministudio:plugin-shell-reactivate'
export const PLUGIN_SHELL_CONNECTION = 'ministudio:plugin-shell-connection'
export const PLUGIN_SHELL_PIN_CHANGED = 'ministudio:plugin-shell-pin-changed'
export const PLUGIN_EDITOR_ACTION = 'ministudio:plugin-editor-action'

type NativePluginEditorAction = { targetId: string; generation: number; action: string }

export type PluginShellTarget = {
  windowLabel: string
  targetKind: 'effect' | 'instrument'
  targetId: string
}

export type PluginShellState = PluginShellTarget & {
  name: string
  vendor: string
  format: string
  bypassed: boolean
  hasSidechain: boolean
  automationMode: 'off' | 'write' | 'read' | 'latch'
  automationCount: number
}

export type PluginShellAction = PluginShellTarget & {
  action: 'toggle-bypass' | 'save-preset' | 'load-preset' | 'show-sidechain' | 'show-automation' | 'set-automation-mode' | 'unregister'
  mode?: PluginShellState['automationMode']
}

export type PluginShellPinChanged = PluginShellTarget & { pinned: boolean }

type LocatedPlugin = {
  plugin: ExternalPluginRef
  deviceType: string
  params: Record<string, number>
  bypassed: boolean
  effect?: EffectInstance
  target?: RackTarget
  trackId?: string
}

function locatePlugin(project: ProjectState, kind: PluginShellTarget['targetKind'], id: string): LocatedPlugin | null {
  if (kind === 'instrument') {
    const track = project.tracks.find((candidate) => candidate.id === id)
    const instrument = track?.instrument
    if (!track || !instrument?.plugin) return null
    return { plugin: instrument.plugin, deviceType: instrument.type, params: instrument.params, bypassed: instrument.bypassed, trackId: track.id }
  }
  for (const track of project.tracks) {
    const effect = track.effects.find((candidate) => candidate.id === id)
    if (effect?.plugin) return { plugin: effect.plugin, deviceType: effect.type, params: effect.params, bypassed: effect.bypassed, effect, target: { kind: 'track', id: track.id }, trackId: track.id }
  }
  for (const bus of project.buses) {
    const effect = bus.effects.find((candidate) => candidate.id === id)
    if (effect?.plugin) return { plugin: effect.plugin, deviceType: effect.type, params: effect.params, bypassed: effect.bypassed, effect, target: { kind: 'bus', id: bus.id } }
  }
  const effect = project.master.effects.find((candidate) => candidate.id === id)
  return effect?.plugin ? { plugin: effect.plugin, deviceType: effect.type, params: effect.params, bypassed: effect.bypassed, effect, target: { kind: 'master', id: 'master' } } : null
}

function shellState(target: PluginShellTarget): PluginShellState | null {
  const project = useProjectStore.getState().project
  const located = locatePlugin(project, target.targetKind, target.targetId)
  if (!located) return null
  const lanes = located.trackId == null ? [] : project.tracks.find((track) => track.id === located.trackId)?.automationLanes?.filter((lane) => lane.targetKind === target.targetKind && lane.targetId === target.targetId) ?? []
  const hasSidechain = !!located.effect && (!!located.plugin.supportsSidechain || (located.plugin.audioInputBuses ?? 0) > 1)
  return {
    ...target,
    name: located.plugin.name,
    vendor: located.plugin.vendor,
    format: located.plugin.format.toUpperCase(),
    bypassed: located.bypassed,
    hasSidechain,
    automationMode: lanes[0]?.mode ?? 'off',
    automationCount: lanes.length,
  }
}

async function runAction(engine: IAudioEngine, payload: PluginShellAction): Promise<void> {
  if (payload.action === 'unregister') return
  const store = useProjectStore.getState()
  const located = locatePlugin(store.project, payload.targetKind, payload.targetId)
  if (!located) return
  if (payload.action === 'toggle-bypass') {
    if (payload.targetKind === 'instrument' && located.trackId) store.toggleInstrumentBypass(located.trackId)
    else if (located.target) {
      store.toggleTargetEffect(located.target, payload.targetId)
      engine.setEffectBypass(payload.targetId, !located.bypassed)
    }
    return
  }
  if (payload.action === 'set-automation-mode' && payload.mode && located.trackId) {
    const lanes = store.project.tracks.find((track) => track.id === located.trackId)?.automationLanes?.filter((lane) => lane.targetKind === payload.targetKind && lane.targetId === payload.targetId) ?? []
    lanes.forEach((lane) => store.setAutomationLaneMode(located.trackId!, lane.id, payload.mode!))
    return
  }
  if (payload.action === 'show-automation' && located.trackId) {
    store.selectTrack(located.trackId)
    store.setTrackAutomationOpen(located.trackId, true)
    store.setEditFocus('arrangement')
    await getCurrentWebviewWindow().setFocus()
    return
  }
  if (payload.action === 'show-sidechain' && located.target) {
    store.setRackTarget(located.target)
    useProjectStore.setState({ focusedEffectId: payload.targetId })
    await getCurrentWebviewWindow().setFocus()
    return
  }
  if (payload.action === 'save-preset') {
    const state = await engine.savePluginState(payload.targetKind, payload.targetId)
    const path = await saveDevicePreset({ format: 'ministudio-device-preset', version: 1, name: located.plugin.name, deviceType: located.deviceType, pluginUid: located.plugin.uid, params: { ...located.params }, state })
    if (path) store.showToast(`${located.plugin.name} 프리셋을 저장했습니다.`)
    return
  }
  if (payload.action === 'load-preset') {
    const preset = await openDevicePreset()
    if (!preset) return
    if (preset.deviceType !== located.deviceType || preset.pluginUid !== located.plugin.uid) throw new Error('현재 플러그인과 다른 종류의 프리셋입니다.')
    if (preset.state) await engine.loadPluginState(payload.targetKind, payload.targetId, preset.state)
    if (payload.targetKind === 'instrument' && located.trackId) {
      store.updateInstrument(located.trackId, preset.params)
      Object.entries(preset.params).forEach(([id, value]) => engine.setInstrumentParam(located.trackId!, id, value))
    } else if (located.target) {
      store.updateTargetEffect(located.target, payload.targetId, preset.params)
      Object.entries(preset.params).forEach(([id, value]) => engine.setEffectParam(payload.targetId, id, value))
    }
    store.showToast(`${located.plugin.name} 프리셋을 불러왔습니다.`)
  }
}

/** Main-webview bridge. State is reduced to a tiny signature before emitting,
 * so parameter/analyser traffic never repaints the native editor toolbar. */
export function installPluginShellBridge(engine: IAudioEngine): () => void {
  if (!('__TAURI_INTERNALS__' in window)) return () => undefined
  const targets = new Map<string, { target: PluginShellTarget; signature: string }>()
  const pinnedWindows = new Set<string>()
  let pendingForeground: PluginEditorForegroundIntent | null = null
  let pendingForegroundTimer = 0
  let stopped = false
  const unlisteners: UnlistenFn[] = []
  const publish = (target: PluginShellTarget) => {
    const state = shellState(target)
    if (!state) return
    const signature = JSON.stringify(state)
    const current = targets.get(target.windowLabel)
    if (current?.signature === signature) return
    targets.set(target.windowLabel, { target, signature })
    void emitTo(target.windowLabel, PLUGIN_SHELL_STATE, state)
    const automation = ({ off: 0, write: 1, read: 2, latch: 3 } as const)[state.automationMode]
    void invoke('engine_set_plugin_editor_host_state', {
      targetId: target.targetId,
      bypassed: state.bypassed,
      automation,
    }).catch(() => undefined)
  }
  void listen<PluginShellTarget>(PLUGIN_SHELL_REQUEST, ({ payload }) => { targets.set(payload.windowLabel, { target: payload, signature: '' }); publish(payload) }).then((unlisten) => stopped ? unlisten() : unlisteners.push(unlisten))
  void listen<PluginShellAction>(PLUGIN_SHELL_ACTION, ({ payload }) => {
    if (payload.action === 'unregister') { targets.delete(payload.windowLabel); return }
    void runAction(engine, payload).then(() => publish(payload)).catch((error) => useProjectStore.getState().showToast(`플러그인 창 작업 실패: ${String(error)}`))
  }).then((unlisten) => stopped ? unlisten() : unlisteners.push(unlisten))
  void listen<string>(PLUGIN_SHELL_CLOSED, ({ payload }) => { targets.delete(payload); pinnedWindows.delete(payload) })
    .then((unlisten) => stopped ? unlisten() : unlisteners.push(unlisten))
  void listen<PluginShellPinChanged>(PLUGIN_SHELL_PIN_CHANGED, ({ payload }) => {
    if (payload.pinned) pinnedWindows.add(payload.windowLabel)
    else pinnedWindows.delete(payload.windowLabel)
  }).then((unlisten) => stopped ? unlisten() : unlisteners.push(unlisten))
  const applyNativeAction = async (target: PluginShellTarget, nativeAction: string) => {
        if (nativeAction === 'closed') {
          targets.delete(target.windowLabel)
          pinnedWindows.delete(target.windowLabel)
          await engine.closePluginEditor(target.targetKind, target.targetId).catch(() => undefined)
          return
        }
        if (nativeAction === 'toggle-pin') {
          const pinned = !pinnedWindows.has(target.windowLabel)
          if (pinned) pinnedWindows.add(target.windowLabel)
          else pinnedWindows.delete(target.windowLabel)
          await invoke('engine_set_plugin_editor_pinned', {
            targetId: target.targetId,
            pinned,
          }).catch(() => {
            if (pinned) pinnedWindows.delete(target.windowLabel)
            else pinnedWindows.add(target.windowLabel)
          })
          return
        }
        const action: PluginShellAction = {
          ...target,
          action: nativeAction === 'toggle-power' ? 'toggle-bypass'
            : nativeAction === 'automation-off' ? 'set-automation-mode'
              : nativeAction === 'automation-write' ? 'set-automation-mode'
                : nativeAction === 'automation-read' ? 'set-automation-mode'
                  : nativeAction === 'automation-latch' ? 'set-automation-mode'
                    : nativeAction as PluginShellAction['action'],
          mode: nativeAction.startsWith('automation-')
            ? nativeAction.slice('automation-'.length) as PluginShellState['automationMode']
            : undefined,
        }
        await runAction(engine, action)
        publish(target)
  }
  void listen<NativePluginEditorAction>(PLUGIN_EDITOR_ACTION, ({ payload }) => {
    const entry = [...targets.values()].find(({ target }) => target.targetId === payload.targetId)
    if (!entry) return
    void applyNativeAction(entry.target, payload.action).catch((error) => {
      useProjectStore.getState().showToast(`플러그인 창 작업 실패: ${String(error)}`)
    })
  }).then((unlisten) => stopped ? unlisten() : unlisteners.push(unlisten))
  const foregroundPending = (event: Event) => {
    pendingForeground = (event as CustomEvent<PluginEditorForegroundIntent>).detail
    window.clearTimeout(pendingForegroundTimer)
    // Sample-library instruments can take several seconds to join the native
    // graph. Keep the foreground intent alive until the graph commit consumes
    // it; this long watchdog exists only to recover from a failed commit.
    pendingForegroundTimer = window.setTimeout(() => { pendingForeground = null }, 60_000)
  }
  window.addEventListener(PLUGIN_EDITOR_FOREGROUND_PENDING, foregroundPending)
  const unsubscribe = useProjectStore.subscribe(() => targets.forEach(({ target }) => publish(target)))
  const resyncEditors = () => {
    const project = useProjectStore.getState().project
    targets.forEach(({ target }, windowLabel) => {
      // Structural graph sync recreates plug-in instances. Reattach only
      // editors whose target survived; deleted tracks/effects must not leave a
      // stale host registered for every subsequent graph rebuild.
      if (!locatePlugin(project, target.targetKind, target.targetId)) {
        targets.delete(windowLabel)
        return
      }
      // A new explicit editor is about to open on this same graph commit.
      // Restore only other pinned windows; reopening the former unpinned
      // editor here would race the foreground VST3 attachment.
      const isPendingTarget = !!pendingForeground
        && target.targetKind === pendingForeground.targetKind
        && target.targetId === pendingForeground.targetId
      const policy = pluginEditorGraphSyncPolicy(!!pendingForeground, isPendingTarget, pinnedWindows.has(windowLabel))
      if (policy === 'drop') { targets.delete(windowLabel); return }
      if (policy === 'keep') return
      // Production standalone helpers retain their native editor across a
      // graph swap. Reopening every ordinary target here resurrected windows
      // that had just been closed. Native actions now arrive through a generation-guarded
      // helper event, but only pinned restoration is intentional background work;
      // ordinary editors are opened solely by an explicit foreground action.
      // Graph restoration is background intent. A simultaneous explicit EDIT
      // or newly inserted plug-in must win instead of attaching two GUIs.
      void engine.openPluginEditor(target.targetKind, target.targetId, false).catch(() => undefined)
    })
    pendingForeground = null
    window.clearTimeout(pendingForegroundTimer)
  }
  window.addEventListener('ministudio:graph-synced', resyncEditors)
  return () => { stopped = true; unsubscribe(); window.clearTimeout(pendingForegroundTimer); window.removeEventListener(PLUGIN_EDITOR_FOREGROUND_PENDING, foregroundPending); window.removeEventListener('ministudio:graph-synced', resyncEditors); unlisteners.splice(0).forEach((unlisten) => unlisten()) }
}
