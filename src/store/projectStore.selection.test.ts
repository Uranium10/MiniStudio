import { beforeEach, describe, expect, it } from 'vitest'
import { MIDI_PPQ, secondsPerBeat } from '../engine'
import { createDemoProject } from './demoProject'
import { automationOptionsForTrack, effectDefaults, useProjectStore } from './projectStore'

describe('multi-track selection and routing', () => {
  beforeEach(() => {
    const project = createDemoProject()
    useProjectStore.setState({ project, selectedTrackId: project.tracks[0]!.id, selectedTrackIds: [project.tracks[0]!.id], rackTarget: { kind: 'track', id: project.tracks[0]!.id }, lowerTab: 'mixer', lowerPanelHeight: 350, mixerPanelHeight: 350, effectsPanelHeight: 350, focusedEffectId: null, past: [], future: [], history: [], futureHistory: [] })
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
    expect(defaults.quality).toBe(1)
    expect(defaults.transient).toBeCloseTo(.72)
    expect(defaults.midi).toBe(0)
    expect(defaults.scale).toBe(0)
    expect(Array.from({ length: 12 }, (_, pitch) => defaults[`pitch${pitch}`])).toEqual([1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1])
  })

  it('restores a deleted instrument and its plug-in assignment through undo', () => {
    const id = useProjectStore.getState().addInstrumentTrack({ format: 'vst3', uid: 'test', name: 'Test Instrument', vendor: 'MiniStudio', path: 'test.vst3' })
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
    useProjectStore.getState().replaceTrackInstrument(track.id, { format: 'clap', uid: 'replacement', name: 'Replacement', vendor: 'MiniStudio', path: 'replacement.clap' })
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === track.id)?.instrument?.type).toBe('clap:replacement')
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === track.id)?.midiClips.map((clip) => clip.id)).toEqual(clipIds)
    useProjectStore.getState().undo()
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === track.id)?.instrument?.type).toBe(originalType)
  })

  it('sizes new MIDI clips from their duration and grows them for out-of-range events', () => {
    const trackId = useProjectStore.getState().addInstrumentTrack()
    const bpm = useProjectStore.getState().project.transport.bpm
    const clipId = useProjectStore.getState().addMidiClip(trackId, 0, 1)!
    let clip = useProjectStore.getState().project.tracks.find((track) => track.id === trackId)!.midiClips.find((item) => item.id === clipId)!
    expect(clip.loopLengthTicks).toBe(Math.round(1 / secondsPerBeat(bpm) * MIDI_PPQ))

    const noteEnd = clip.loopLengthTicks + MIDI_PPQ * 2
    useProjectStore.getState().addMidiNote(trackId, clipId, { pitch: 60, velocity: 100, startTicks: noteEnd - 240, lengthTicks: 240, releaseVelocity: 64, muted: false })
    clip = useProjectStore.getState().project.tracks.find((track) => track.id === trackId)!.midiClips.find((item) => item.id === clipId)!
    expect(clip.durationSec).toBeCloseTo(noteEnd / MIDI_PPQ * secondsPerBeat(bpm), 6)
    expect(clip.loopLengthTicks).toBe(noteEnd)

    useProjectStore.getState().upsertMidiControlPoints(trackId, clipId, 1, [{ ticks: noteEnd + 480, value: 64 }])
    clip = useProjectStore.getState().project.tracks.find((track) => track.id === trackId)!.midiClips.find((item) => item.id === clipId)!
    expect(clip.loopLengthTicks).toBe(noteEnd + 481)
  })

  it('starts the transient shaper neutral with a balanced detector', () => {
    expect(effectDefaults('builtin:transient-shaper')).toEqual({ attack: 0, sustain: 0, thresholdDb: -36, speed: .5, clip: 0 })
  })

  it('seeds external rack effects from their published parameter defaults', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    useProjectStore.getState().addEffect(track.id, 'vst3:test-effect', { format: 'vst3', uid: 'test-effect', name: 'Test Effect', vendor: 'MiniStudio', path: 'test.vst3', parameters: [{ id: 'macro-1', name: 'Macro 1', module: 'Macros', min: 0, max: 1, defaultValue: .37 }] })
    const effect = useProjectStore.getState().project.tracks[0]!.effects.at(-1)!
    expect(effect.params).toEqual({ 'macro-1': .37 })
  })

  it('focuses every inserted effect and duplicates its complete state next to the source', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    useProjectStore.getState().addEffect(track.id, 'builtin:distortion')
    let state = useProjectStore.getState()
    const source = state.project.tracks[0]!.effects.at(-1)!
    expect(state.focusedEffectId).toBe(source.id)
    expect(state.lowerTab).toBe('effects')
    state.updateTargetEffect({ kind: 'track', id: track.id }, source.id, { lowDriveDb: 17 })
    state.duplicateTargetEffect({ kind: 'track', id: track.id }, source.id)
    state = useProjectStore.getState()
    const sourceIndex = state.project.tracks[0]!.effects.findIndex((effect) => effect.id === source.id)
    const duplicate = state.project.tracks[0]!.effects[sourceIndex + 1]!
    expect(duplicate.id).toBe(state.focusedEffectId)
    expect(duplicate.params.lowDriveDb).toBe(17)
    expect(duplicate.params).not.toBe(state.project.tracks[0]!.effects[sourceIndex]!.params)
  })

  it('publishes bypass as an automatable effect parameter', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    useProjectStore.getState().addEffect(track.id, 'builtin:compressor')
    const effect = useProjectStore.getState().project.tracks[0]!.effects.at(-1)!
    expect(automationOptionsForTrack(useProjectStore.getState().project.tracks[0]!).find((option) => option.targetId === effect.id && option.parameterId === '__bypass')).toMatchObject({ min: 0, max: 1, label: 'Bypass' })
  })

  it('groups automation as INSERT, SEND, then individual devices', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    const options = automationOptionsForTrack(track)
    expect(options.filter((option) => option.targetKind === 'track').every((option) => option.category === 'INSERT')).toBe(true)
    expect(options.filter((option) => option.targetKind === 'send').every((option) => option.category === 'SEND')).toBe(true)
    expect(options.some((option) => option.category.startsWith('INSTRUMENT ·') || option.category.startsWith('FX ·'))).toBe(false)
  })

  it('records armed write automation while transport is running', () => {
    const track = useProjectStore.getState().project.tracks[0]!
    const volume = automationOptionsForTrack(track).find((option) => option.parameterId === 'volumeDb')!
    useProjectStore.getState().addAutomationLane(track.id, volume)
    const lane = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    useProjectStore.getState().setAutomationLaneMode(track.id, lane.id, 'write')
    useProjectStore.getState().setPlayhead(1.25)
    useProjectStore.getState().setPlaying(true)
    useProjectStore.getState().updateTrack(track.id, { volumeDb: -7 })
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.points).toEqual([expect.objectContaining({ timeSec: 1.25, value: -7 })])
  })

  it('remembers independent mixer and fixed-height effects panel sizes', () => {
    const store = useProjectStore.getState()
    store.setLowerTab('mixer')
    store.setLowerPanelHeight(610)
    store.setLowerTab('effects')
    store.setLowerPanelHeight(999)
    expect(useProjectStore.getState().effectsPanelHeight).toBe(350)
    store.setLowerPanelHeight(40)
    store.setLowerTab('mixer')
    expect(useProjectStore.getState().mixerPanelHeight).toBe(610)
    expect(useProjectStore.getState().lowerPanelHeight).toBe(610)
  })

  it('inserts a browser instrument at the requested track boundary and undoes it', () => {
    const before = useProjectStore.getState().project.tracks.map((track) => track.id)
    const id = useProjectStore.getState().addInstrumentTrack({ format: 'vst3', uid: 'drop-test', name: 'Dropped Synth', vendor: 'Mini', path: 'drop.vst3' }, 2)
    const after = useProjectStore.getState().project.tracks
    expect(after[2]).toMatchObject({ id, kind: 'instrument', name: 'Dropped Synth' })
    expect(after.filter((track) => track.id !== id).map((track) => track.id)).toEqual(before)
    useProjectStore.getState().undo()
    expect(useProjectStore.getState().project.tracks.map((track) => track.id)).toEqual(before)
  })
})
