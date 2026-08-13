import { afterEach, describe, expect, it } from 'vitest'
import type { EffectInstance, Track } from '../engine'
import { createEmptyProject } from './demoProject'
import { useProjectStore } from './projectStore'

const compressor = (id: string, sourceTrackId: string | null = null): EffectInstance => ({
  id,
  type: 'builtin:compressor',
  bypassed: false,
  params: {},
  sidechain: sourceTrackId ? { enabled: true, sourceTrackId } : undefined,
})

const audioTrack = (id: string, effects: EffectInstance[] = []): Track => ({
  id,
  kind: 'audio',
  name: id,
  color: '#55a7ff',
  clips: [],
  midiClips: [],
  instrument: null,
  volumeDb: 0,
  pan: 0,
  muted: false,
  solo: false,
  armed: false,
  effects,
  sends: [],
})

describe('sidechain routing', () => {
  afterEach(() => useProjectStore.getState().clearToast())

  it('stores return-to-master sidechains', () => {
    const project = createEmptyProject()
    project.master.effects = [compressor('master-compressor')]
    useProjectStore.setState({ project, toast: null })

    useProjectStore.getState().setTargetEffectSidechain(
      { kind: 'master', id: 'master' },
      'master-compressor',
      true,
      'bus:bus-a',
    )

    expect(useProjectStore.getState().project.master.effects[0]?.sidechain).toEqual({
      enabled: true,
      sourceTrackId: 'bus:bus-a',
    })
  })

  it('rejects a track-return cycle', () => {
    const project = createEmptyProject()
    project.tracks = [audioTrack('track-a', [compressor('track-compressor', 'bus:bus-a')])]
    project.buses[0]!.effects = [compressor('bus-compressor')]
    useProjectStore.setState({ project, toast: null })

    useProjectStore.getState().setTargetEffectSidechain(
      { kind: 'bus', id: 'bus-a' },
      'bus-compressor',
      true,
      'track-a',
    )

    expect(useProjectStore.getState().project.buses[0]?.effects[0]?.sidechain).toBeUndefined()
    expect(useProjectStore.getState().toast).toContain('순환 참조')
  })
})
