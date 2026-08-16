import type { IAudioEngine } from '../engine'

/**
 * Runs a DAW dialog while the native plug-in editor is disabled and lowered from the global
 * topmost band. The helper acknowledges both transitions, so the file picker cannot appear
 * behind the editor or leave it disabled after cancellation/failure.
 */
export async function withPluginEditorModal<T>(engine: IAudioEngine, targetId: string, task: () => Promise<T>): Promise<T> {
  await engine.setPluginEditorModal(targetId, true)
  try {
    return await task()
  } finally {
    await engine.setPluginEditorModal(targetId, false).catch(() => undefined)
  }
}
