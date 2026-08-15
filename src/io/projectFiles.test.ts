import { describe, expect, it } from 'vitest'
import { createEmptyProject } from '../store/demoProject'
import { toStoredProject } from './projectFiles'

describe('portable project plug-in references', () => {
  it('removes machine-specific plug-in paths without mutating the live project', () => {
    const project = createEmptyProject()
    project.tracks.push({
      id: 'instrument', kind: 'instrument', name: 'Portable synth', color: '#fff', clips: [], midiClips: [],
      instrument: { id: 'plugin', type: 'vst3:portable.uid', bypassed: false, params: {}, plugin: { format: 'vst3', uid: 'portable.uid', name: 'Portable', vendor: 'Vendor', path: 'D:\\Audio\\Portable.vst3' } },
      volumeDb: 0, pan: 0, muted: false, solo: false, armed: false, effects: [], sends: [],
    })

    const stored = toStoredProject(project)
    expect(stored.tracks[0]?.instrument?.plugin?.path).toBe('')
    expect(stored.tracks[0]?.instrument?.plugin?.uid).toBe('portable.uid')
    expect(project.tracks[0]?.instrument?.plugin?.path).toBe('D:\\Audio\\Portable.vst3')
  })
})
