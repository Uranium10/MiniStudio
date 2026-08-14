import { beforeEach, describe, expect, it } from 'vitest'
import { createDemoProject } from './demoProject'
import { automationOptionsForTrack, effectDefaults, useProjectStore } from './projectStore'

describe('multi-track selection and routing', () => {
  beforeEach(() => {
    const project = createDemoProject()
    useProjectStore.setState({ project, selectedTrackId: project.tracks[0]!.id, selectedTrackIds: [project.tracks[0]!.id], rackTarget: { kind: 'track', id: project.tracks[0]!.id }, past: [], future: [], history: [], futureHistory: [] })
  })

  it('selects the inclusive ordered range with Shift semantics', () => {
    const tracks = useProjectStore.getState().project.tracks
    useProjectStore.getState().selectTrack(tracks[2]!.id, true)
    const state = useProjectStore.getState()
    expect(state.selectedTrackIds).toEqual(tracks.slice(0, 3).map((track) => track.id))
    expect(state.selectedTrackId).toBe(tracks[2]!.id)
    expect(state.rackTarget).toEqual({ kind: 'track', id: tracks[2]!.id })
  })

  it('follows track focus without forcing the FX tab open', () => {
    const tracks = useProjectStore.getState().project.tracks
    useProjectStore.getState().selectTrack(tracks[1]!.id)
    expect(useProjectStore.getState().selectedTrackIds).toEqual([tracks[1]!.id])
    expect(useProjectStore.getState().rackTarget).toEqual({ kind: 'track', id: tracks[1]!.id })
    expect(useProjectStore.getState().lowerTab).not.toBe('effects')
    useProjectStore.getState().setLowerTab('effects')
    expect(useProjectStore.getState().lowerTab).toBe('effects')
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

  it('stores an editable outgoing curve without losing it when moving the point', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    const volume = automationOptionsForTrack(track).find((option) => option.parameterId === 'volumeDb')!
    useProjectStore.getState().addAutomationLane(track.id, volume)
    const lane = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    const id = useProjectStore.getState().upsertAutomationPoint(track.id, lane.id, { timeSec: 1, value: -12 })!
    useProjectStore.getState().setAutomationCurve(track.id, lane.id, id, .65)
    useProjectStore.getState().upsertAutomationPoint(track.id, lane.id, { id, timeSec: 2, value: -6 })
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.points[0]).toMatchObject({ id, timeSec: 2, value: -6, curve: .65 })
  })

  it('stores and clips an independently resizable automation lane height', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    const volume = automationOptionsForTrack(track).find((option) => option.parameterId === 'volumeDb')!
    useProjectStore.getState().addAutomationLane(track.id, volume)
    const lane = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    useProjectStore.getState().setAutomationLaneHeight(track.id, lane.id, 132)
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.height).toBe(132)
    useProjectStore.getState().setAutomationLaneHeight(track.id, lane.id, 999)
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.height).toBe(240)
    useProjectStore.getState().undo()
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.height).toBeUndefined()
  })

  it('resizes only the selected track bank and keeps new EQ bands off by default', () => {
    const tracks = useProjectStore.getState().project.tracks
    useProjectStore.setState({ selectedTrackId: tracks[1]!.id, selectedTrackIds: [tracks[1]!.id] })
    useProjectStore.getState().resizeSelectedTracks(18)
    expect(useProjectStore.getState().project.tracks[1]!.height).toBe(90)
    expect(useProjectStore.getState().project.tracks[0]!.height).toBeUndefined()
    for (const type of ['builtin:eq', 'builtin:eq8'] as const) {
      const defaults = effectDefaults(type)
      expect(defaults['band0.enabled']).toBe(1)
      expect(Object.keys(defaults).filter((key) => /band\d+\.enabled/.test(key) && key !== 'band0.enabled').every((key) => defaults[key] === 0)).toBe(true)
    }
  })

  it('resizes every track through the global track-height command', () => {
    const before = useProjectStore.getState().project.tracks.map((track) => track.height ?? useProjectStore.getState().trackHeight)
    useProjectStore.getState().resizeAllTracks(8)
    expect(useProjectStore.getState().project.tracks.map((track) => track.height)).toEqual(before.map((height) => height + 8))
  })

  it('starts Colorizer in a deterministic manual C-major mask', () => {
    const defaults = effectDefaults('builtin:resonator')
    expect(defaults.midi).toBe(0)
    expect(defaults.scale).toBe(0)
    expect(Array.from({ length: 12 }, (_, pitch) => defaults[`pitch${pitch}`])).toEqual([1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1])
  })

  it('restores a deleted instrument and its plug-in assignment through undo', () => {
    const id = useProjectStore.getState().addInstrumentTrack({ format: 'vst3', uid: 'test', name: 'Test Instrument', vendor: 'MiniDAW', path: 'test.vst3' })
    useProjectStore.getState().removeTrack(id)
    expect(useProjectStore.getState().project.tracks.some((track) => track.id === id)).toBe(false)
    expect(useProjectStore.getState().history.at(-1)?.label).toContain('삭제')
    useProjectStore.getState().undo()
    expect(useProjectStore.getState().project.tracks.find((track) => track.id === id)?.instrument?.plugin?.uid).toBe('test')
  })

  it('stores sorted, clipped controller and pitch-bend events', () => {
    const track = useProjectStore.getState().project.tracks.find((item) => item.kind === 'instrument')!
    const clip = track.midiClips[0]!
    useProjectStore.getState().upsertMidiControlPoints(track.id, clip.id, 64, [{ ticks: 480, value: 999 }, { ticks: 0, value: -3 }])
    useProjectStore.getState().upsertMidiControlPoints(track.id, clip.id, -1, [{ ticks: 240, value: -99_999 }])
    const lanes = useProjectStore.getState().project.tracks.find((item) => item.id === track.id)!.midiClips[0]!.ccLanes
    expect(lanes.find((lane) => lane.cc === 64)?.points).toEqual([{ ticks: 0, value: 0 }, { ticks: 480, value: 127 }])
    expect(lanes.find((lane) => lane.cc === -1)?.points).toEqual([{ ticks: 240, value: -8192 }])
  })

  it('replaces an instrument without touching its MIDI clips and restores it through undo', () => {
    const track = useProjectStore.getState().project.tracks.find((item) => item.kind === 'instrument')!
    const clipIds = track.midiClips.map((clip) => clip.id)
    const originalType = track.instrument?.type
    useProjectStore.getState().replaceTrackInstrument(track.id, { format: 'clap', uid: 'replacement', name: 'Replacement', vendor: 'MiniDAW', path: 'replacement.clap' })
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === track.id)?.instrument?.type).toBe('clap:replacement')
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === track.id)?.midiClips.map((clip) => clip.id)).toEqual(clipIds)
    useProjectStore.getState().undo()
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === track.id)?.instrument?.type).toBe(originalType)
  })
})
