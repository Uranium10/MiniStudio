import type { ProjectState } from '../engine'
import { deserializeProject, serializeProject } from './projectFiles'

const SNAPSHOT_KEY = 'ministudio.autosave.snapshot.v1'
const DIRTY_KEY = 'ministudio.autosave.dirty.v1'

export type RecoverySnapshot = { savedAt: number; project: ProjectState }

function browserStorage(): Storage | null {
  try { return window.localStorage } catch { return null }
}

export function writeRecoverySnapshot(project: ProjectState, storage = browserStorage()): boolean {
  if (!storage) return false
  try {
    storage.setItem(SNAPSHOT_KEY, JSON.stringify({ savedAt: Date.now(), project: serializeProject(project) }))
    storage.setItem(DIRTY_KEY, '1')
    return true
  } catch {
    return false
  }
}

export function readRecoverySnapshot(storage = browserStorage()): RecoverySnapshot | null {
  if (!storage || storage.getItem(DIRTY_KEY) !== '1') return null
  try {
    const envelope = JSON.parse(storage.getItem(SNAPSHOT_KEY) ?? '') as { savedAt?: unknown; project?: unknown }
    if (typeof envelope.savedAt !== 'number' || typeof envelope.project !== 'string') return null
    return { savedAt: envelope.savedAt, project: deserializeProject(envelope.project) }
  } catch {
    return null
  }
}

export function markSessionClean(storage = browserStorage()): void {
  try { storage?.setItem(DIRTY_KEY, '0') } catch { /* Storage may be unavailable. */ }
}

export function discardRecoverySnapshot(storage = browserStorage()): void {
  try { storage?.removeItem(SNAPSHOT_KEY); storage?.setItem(DIRTY_KEY, '0') } catch { /* Storage may be unavailable. */ }
}
