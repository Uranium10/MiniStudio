import { beforeEach, describe, expect, it } from 'vitest'
import { createDemoProject } from './demoProject'
import { getProjectSnapshot, useProjectStore } from './projectStore'

describe('project store performance invariants', () => {
  beforeEach(() => useProjectStore.getState().setProject(createDemoProject()))

  it('shares immutable waveform buffers across edits and undo history', () => {
    const before = useProjectStore.getState().project
    const peaks = before.assets['asset-0']?.peaks
    const untouchedTrack = before.tracks[1]

    useProjectStore.getState().updateTrack('track-0', { volumeDb: -9 })
    const after = useProjectStore.getState()

    expect(after.project.assets).toBe(before.assets)
    expect(after.project.assets['asset-0']?.peaks).toBe(peaks)
    expect(after.past.at(-1)?.assets['asset-0']?.peaks).toBe(peaks)
    expect(after.project.tracks[1]).toBe(untouchedTrack)
  })

  it('coalesces continuous parameter changes into one undo step', () => {
    const initial = useProjectStore.getState().project.master.volumeDb
    useProjectStore.getState().updateMasterVolume(-2)
    useProjectStore.getState().updateMasterVolume(-3)
    useProjectStore.getState().updateMasterVolume(-4)

    expect(useProjectStore.getState().past).toHaveLength(1)
    useProjectStore.getState().undo()
    expect(useProjectStore.getState().project.master.volumeDb).toBe(initial)
  })

  it('keeps playhead updates outside the persistent project graph', () => {
    const project = useProjectStore.getState().project
    useProjectStore.getState().setPlayhead(27.5)

    expect(useProjectStore.getState().project).toBe(project)
    expect(getProjectSnapshot().transport.playheadSec).toBe(27.5)
  })
})
