import { beforeEach, describe, expect, it } from 'vitest'
import type { ExternalPluginRef, Track } from '../engine'
import { createEmptyProject } from './demoProject'
import { automationOptionsForTrack, useProjectStore } from './projectStore'

const plugin: ExternalPluginRef = {
  format: 'vst3', uid: 'test.plugin', name: 'Test Plug-in', vendor: 'Mini', path: 'portable/test.vst3', paramCount: 1,
  parameters: [{ id: '7', name: 'Cutoff', module: 'Filter', min: 0, max: 1, defaultValue: 0.5 }],
}

function instrumentTrack(): Track {
  return {
    id: 'track-plugin', kind: 'instrument', name: 'Plug-in', color: '#55a7ff', clips: [], midiClips: [],
    instrument: { id: 'instrument-plugin', type: 'vst3:test.plugin', params: { '7': 0.5 }, bypassed: false, plugin },
    volumeDb: 0, pan: 0, muted: false, solo: false, armed: false, effects: [], sends: [], automationLanes: [],
  }
}

describe('native plug-in automation gestures', () => {
  beforeEach(() => {
    const project = createEmptyProject()
    project.tracks = [instrumentTrack()]
    useProjectStore.setState({ project, playheadSec: 2, past: [], future: [], history: [], futureHistory: [] })
  })

  it('exposes each described VST parameter exactly once', () => {
    const options = automationOptionsForTrack(useProjectStore.getState().project.tracks[0]!)
    expect(options.filter((option) => option.targetKind === 'instrument' && option.parameterId.replace(/^param:/, '') === '7')).toHaveLength(1)
    expect(options.find((option) => option.parameterId === 'param:7')?.label).toBe('Filter · Cutoff')
  })

  it('creates and writes the moved parameter lane in write mode', () => {
    const store = useProjectStore.getState()
    store.setDeviceAutomationMode('track-plugin', 'instrument', 'track-plugin', 'write')
    store.setPlaying(true)
    useProjectStore.getState().applyPluginParameterChanges([{ targetId: 'track-plugin', parameterId: 'param:7', value: 0.8 }])

    const track = useProjectStore.getState().project.tracks[0]!
    expect(track.instrument?.params['7']).toBe(0.8)
    expect(track.automationOpen).toBe(true)
    expect(track.automationLanes).toHaveLength(1)
    expect(track.automationLanes?.[0]).toMatchObject({ targetKind: 'instrument', targetId: 'track-plugin', parameterId: 'param:7', mode: 'write' })
    expect(track.automationLanes?.[0]?.points).toEqual([expect.objectContaining({ timeSec: 2, value: 0.8 })])
  })

  it('keeps read mode parameter feedback out of write lanes', () => {
    useProjectStore.getState().applyPluginParameterChanges([{ targetId: 'track-plugin', parameterId: 'param:7', value: 0.2 }])
    const track = useProjectStore.getState().project.tracks[0]!
    expect(track.instrument?.params['7']).toBe(0.2)
    expect(track.automationLanes).toEqual([])
  })
})
