import type { IAudioEngine } from '../engine'

export const PLUGIN_EDITOR_FOREGROUND_PENDING = 'ministudio:plugin-editor-foreground-pending'
export type PluginEditorForegroundIntent = {
  targetKind: 'effect' | 'instrument'
  targetId: string
}

/** Open immediately after the structural graph containing this target commits.
 * This avoids timer polling (80 -> 240 -> 640 ms), which made a fast plug-in
 * wait longer than its own editor initialization. */
export function openPluginEditorWhenReady(engine: IAudioEngine, targetKind: 'effect' | 'instrument', targetId: string, onError?: (error: unknown) => void): void {
  window.dispatchEvent(new CustomEvent<PluginEditorForegroundIntent>(PLUGIN_EDITOR_FOREGROUND_PENDING, {
    detail: { targetKind, targetId },
  }))
  let finished = false
  let opening = false
  let watchdog = 0
  const cleanup = () => {
    window.removeEventListener('ministudio:graph-synced', open)
    window.clearTimeout(watchdog)
  }
  const attempt = () => {
    if (finished || opening) return
    opening = true
    void engine.openPluginEditor(targetKind, targetId, true).then(() => {
      finished = true
      cleanup()
    }).catch((error) => {
      finished = true
      opening = false
      cleanup()
      onError?.(error)
    })
  }
  // Persistent graph-resync listeners run first. Queue explicit insertion in
  // the following microtask so its foreground generation always wins.
  const open = () => queueMicrotask(attempt)
  window.addEventListener('ministudio:graph-synced', open)
  // Never force an editor attach on a timer. Large sample instruments can
  // spend seconds rebuilding the graph; a timed attempt would enter vendor UI
  // code while that instance is still being replaced. All insertion callers
  // subscribe synchronously before App's coalesced graph commit. This timer is
  // diagnostic only and cannot race the graph.
  watchdog = window.setTimeout(() => {
    if (finished) return
    finished = true
    cleanup()
    onError?.(new Error('플러그인 그래프 준비 시간이 초과되었습니다.'))
  }, 60_000)
}
