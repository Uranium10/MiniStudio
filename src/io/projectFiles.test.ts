import { describe, expect, it } from 'vitest'
import { createEmptyProject } from '../store/demoProject'
import legacyV1 from './tests/fixtures/project-v1.json'
import legacyV2 from './tests/fixtures/project-v2.json'
import { CURRENT_PROJECT_FORMAT, deserializeProject, serializeProject, toStoredProject } from './projectFiles'

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

describe('project format migration', () => {
  it.each([legacyV1, legacyV2])('migrates every stored fixture through the version chain', (fixture) => {
    const project = deserializeProject(JSON.stringify(fixture))
    expect(project.formatVersion).toBe(CURRENT_PROJECT_FORMAT)
    expect(project.transport.tempoMap.tempoPoints[0]?.bpm).toBe(project.transport.bpm)
    expect(deserializeProject(serializeProject(project))).toEqual(project)
  })

  it('shares one source ref for legacy clips that referenced the same asset', () => {
    const project = deserializeProject(JSON.stringify(legacyV2))
    expect(project.tracks[0]!.clips[0]!.audioSourceRefId).toBe(project.tracks[0]!.clips[1]!.audioSourceRefId)
  })

  it('ignores unknown fields but rejects unsupported future versions explicitly', () => {
    expect(() => deserializeProject(JSON.stringify({ ...legacyV2, formatVersion: 999 }))).toThrow(/더 새로운 MiniStudio 형식/)
  })
})
