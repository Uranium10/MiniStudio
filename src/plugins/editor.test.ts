import { afterEach, describe, expect, it, vi } from 'vitest'
import type { IAudioEngine } from '../engine'
import { openPluginEditorWhenReady, PLUGIN_EDITOR_FOREGROUND_PENDING, pluginEditorGraphSyncPolicy, type PluginEditorForegroundIntent } from './editor'

function installTestWindow(): EventTarget {
  const target = new EventTarget() as EventTarget & Pick<Window, 'setTimeout' | 'clearTimeout'>
  target.setTimeout = globalThis.setTimeout.bind(globalThis) as typeof window.setTimeout
  target.clearTimeout = globalThis.clearTimeout.bind(globalThis) as typeof window.clearTimeout
  vi.stubGlobal('window', target)
  return target
}

describe('openPluginEditorWhenReady', () => {
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals() })

  it('opens on the graph commit without a polling delay or duplicate attempt', async () => {
    const target = installTestWindow()
    const openPluginEditor = vi.fn().mockResolvedValue(undefined)
    const engine = { openPluginEditor } as unknown as IAudioEngine
    const backgroundResync = vi.fn()
    const foregroundPending = vi.fn()
    target.addEventListener('ministudio:graph-synced', backgroundResync)
    target.addEventListener(PLUGIN_EDITOR_FOREGROUND_PENDING, foregroundPending)

    openPluginEditorWhenReady(engine, 'instrument', 'track-serum')
    expect(foregroundPending).toHaveBeenCalledOnce()
    expect((foregroundPending.mock.calls[0]![0] as CustomEvent<PluginEditorForegroundIntent>).detail).toEqual({
      targetKind: 'instrument',
      targetId: 'track-serum',
    })
    expect(openPluginEditor).not.toHaveBeenCalled()

    target.dispatchEvent(new Event('ministudio:graph-synced'))
    expect(backgroundResync).toHaveBeenCalledOnce()
    expect(openPluginEditor).not.toHaveBeenCalled()
    await Promise.resolve()
    await Promise.resolve()
    expect(openPluginEditor).toHaveBeenCalledOnce()
    expect(openPluginEditor).toHaveBeenCalledWith('instrument', 'track-serum', true)

    target.dispatchEvent(new Event('ministudio:graph-synced'))
    await Promise.resolve()
    expect(openPluginEditor).toHaveBeenCalledOnce()
  })

  it('never opens on the old timer while a heavy graph is still rebuilding', async () => {
    vi.useFakeTimers()
    const target = installTestWindow()
    const openPluginEditor = vi.fn().mockResolvedValue(undefined)
    const engine = { openPluginEditor } as unknown as IAudioEngine

    openPluginEditorWhenReady(engine, 'instrument', 'track-bbc')
    await vi.advanceTimersByTimeAsync(10_000)
    expect(openPluginEditor).not.toHaveBeenCalled()

    target.dispatchEvent(new Event('ministudio:graph-synced'))
    await Promise.resolve()
    await Promise.resolve()
    expect(openPluginEditor).toHaveBeenCalledOnce()
    expect(openPluginEditor).toHaveBeenCalledWith('instrument', 'track-bbc', true)
  })
})

describe('pluginEditorGraphSyncPolicy', () => {
  it('never resurrects an ordinary editor during background graph sync', () => {
    expect(pluginEditorGraphSyncPolicy(false, false, false)).toBe('keep')
    expect(pluginEditorGraphSyncPolicy(false, false, true)).toBe('restore')
  })

  it('lets foreground intent replace only unpinned competing editors', () => {
    expect(pluginEditorGraphSyncPolicy(true, true, false)).toBe('keep')
    expect(pluginEditorGraphSyncPolicy(true, false, false)).toBe('drop')
    expect(pluginEditorGraphSyncPolicy(true, false, true)).toBe('restore')
  })
})
