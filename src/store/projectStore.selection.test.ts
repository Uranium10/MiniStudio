import { beforeEach, describe, expect, it } from 'vitest'
import { createDemoProject } from './demoProject'
import { automationOptionsForTrack, useProjectStore } from './projectStore'

describe('multi-track selection and routing', () => {
  beforeEach(() => {
    const project = createDemoProject()
    useProjectStore.setState({ project, selectedTrackId: project.tracks[0]!.id, selectedTrackIds: [project.tracks[0]!.id], rackTarget: { kind: 'track', id: project.tracks[0]!.id }, past: [], future: [] })
  })

  it('selects the inclusive ordered range with Shift semantics', () => {
    const tracks = useProjectStore.getState().project.tracks
    useProjectStore.getState().selectTrack(tracks[2]!.id, true)
    const state = useProjectStore.getState()
    expect(state.selectedTrackIds).toEqual(tracks.slice(0, 3).map((track) => track.id))
    expect(state.selectedTrackId).toBe(tracks[2]!.id)
    expect(state.rackTarget).toEqual({ kind: 'track', id: tracks[2]!.id })
  })

  it('updates a selected volume bank in one project edit', () => {
    const tracks = useProjectStore.getState().project.tracks
    useProjectStore.getState().updateTrackVolumes([{ id: tracks[0]!.id, volumeDb: -4 }, { id: tracks[1]!.id, volumeDb: 2 }])
    expect(useProjectStore.getState().project.tracks.slice(0, 2).map((track) => track.volumeDb)).toEqual([-4, 2])
  })

  it('creates and focuses a true main-output bus for the selected tracks', () => {
    const tracks = useProjectStore.getState().project.tracks
    useProjectStore.setState({ selectedTrackIds: [tracks[0]!.id, tracks[1]!.id], selectedTrackId: tracks[1]!.id })
    const id = useProjectStore.getState().createBusFromSelectedTracks()
    const state = useProjectStore.getState()
    expect(id).toBeTruthy()
    expect(state.project.buses.at(-1)?.id).toBe(id)
    expect(state.project.tracks.slice(0, 2).every((track) => track.outputBusId === id)).toBe(true)
    expect(state.rackTarget).toEqual({ kind: 'bus', id })
  })

  it('adds, clips, and sorts editable automation points', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    const volume = automationOptionsForTrack(track).find((option) => option.parameterId === 'volumeDb')!
    useProjectStore.getState().addAutomationLane(track.id, volume)
    const lane = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    useProjectStore.getState().upsertAutomationPoint(track.id, lane.id, { timeSec: 4, value: 99 })
    useProjectStore.getState().upsertAutomationPoint(track.id, lane.id, { timeSec: 1, value: -99 })
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.points.map((point) => [point.timeSec, point.value])).toEqual([[1, -60], [4, 12]])
  })
})
