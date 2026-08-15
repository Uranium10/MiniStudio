import { describe, expect, it } from 'vitest'
import { createEmptyProject } from '../store/demoProject'
import { discardRecoverySnapshot, markSessionClean, readRecoverySnapshot, writeRecoverySnapshot } from './autosave'

class MemoryStorage implements Storage {
  private values = new Map<string, string>()
  get length() { return this.values.size }
  clear() { this.values.clear() }
  getItem(key: string) { return this.values.get(key) ?? null }
  key(index: number) { return [...this.values.keys()][index] ?? null }
  removeItem(key: string) { this.values.delete(key) }
  setItem(key: string, value: string) { this.values.set(key, value) }
}

describe('crash recovery snapshots', () => {
  it('restores only sessions that were not marked clean', () => {
    const storage = new MemoryStorage()
    const project = createEmptyProject()
    project.meta.name = 'Recover me'
    expect(writeRecoverySnapshot(project, storage)).toBe(true)
    expect(readRecoverySnapshot(storage)?.project.meta.name).toBe('Recover me')
    markSessionClean(storage)
    expect(readRecoverySnapshot(storage)).toBeNull()
  })

  it('can discard a stale recovery candidate', () => {
    const storage = new MemoryStorage()
    writeRecoverySnapshot(createEmptyProject(), storage)
    discardRecoverySnapshot(storage)
    expect(readRecoverySnapshot(storage)).toBeNull()
  })
})
