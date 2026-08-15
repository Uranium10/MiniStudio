import { beforeEach, describe, expect, it } from 'vitest'
import type { AudioAssetInfo } from '../engine'
import { quantizeInContext } from './commands'
import { createDemoProject } from './demoProject'
import { automationOptionsForTrack, clipSourceStep, snapTicksWithSwing, useProjectStore } from './projectStore'

const asset: AudioAssetInfo = { id: 'drop-asset', path: 'C:\\Samples\\kick.wav', name: 'kick.wav', durationSec: 2, sampleRate: 48_000, numChannels: 2, peaks: new Float32Array([0, .5]) }

describe('media placement and quantize', () => {
  beforeEach(() => useProjectStore.getState().setProject(createDemoProject()))

  it('places media on an existing audio track at the requested time', () => {
    const track = useProjectStore.getState().project.tracks.find((item) => item.kind === 'audio')!
    const before = track.clips.length
    useProjectStore.getState().insertAudioAsset(asset, 3.25, track.id)
    const result = useProjectStore.getState().project.tracks.find((item) => item.id === track.id)!
    expect(result.clips).toHaveLength(before + 1)
    expect(result.clips.at(-1)).toMatchObject({ assetId: asset.id, startSec: 3.25, playbackRate: 1, reversed: false })
  })

  it('creates an audio track at a gap index', () => {
    useProjectStore.getState().insertAudioAsset(asset, 1, undefined, 1)
    const result = useProjectStore.getState().project.tracks[1]!
    expect(result.kind).toBe('audio')
    expect(result.clips[0]).toMatchObject({ assetId: asset.id, startSec: 1 })
  })

  it('quantizes selected arrangement clips and piano notes to the active grid', () => {
    const store = useProjectStore.getState()
    const audioTrack = store.project.tracks.find((item) => item.clips.length)!
    const clip = audioTrack.clips[0]!
    store.updateClip(audioTrack.id, clip.id, { startSec: .37 })
    useProjectStore.setState({ selectedClipIds: [clip.id], editFocus: 'arrangement', gridTicks: 480 })
    quantizeInContext()
    const gridSec = .5 * 60 / useProjectStore.getState().project.transport.bpm
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === audioTrack.id)!.clips[0]!.startSec).toBeCloseTo(Math.round(.37 / gridSec) * gridSec)

    const instrument = useProjectStore.getState().project.tracks.find((item) => item.midiClips.length)!
    const midi = instrument.midiClips[0]!
    const note = midi.notes[0]!
    store.updateMidiNotes(instrument.id, midi.id, [note.id], { startTicks: 733 })
    useProjectStore.setState({ editorClip: { trackId: instrument.id, clipId: midi.id }, selectedNoteIds: [note.id], editFocus: 'pianoRoll', pianoGridTicks: 480 })
    quantizeInContext()
    expect(useProjectStore.getState().project.tracks.find((item) => item.id === instrument.id)!.midiClips[0]!.notes.find((item) => item.id === note.id)!.startTicks).toBe(960)
  })

  it('keeps arrangement and piano-roll quantize settings independent and swings offbeats', () => {
    const store = useProjectStore.getState()
    store.setGridTicks(960)
    store.setPianoGridTicks(240)
    store.setArrangementSwing(20)
    store.setPianoSwing(60)
    expect(useProjectStore.getState()).toMatchObject({ gridTicks: 960, pianoGridTicks: 240, arrangementSwing: 20, pianoSwing: 60 })
    expect(snapTicksWithSwing(240, 240, 50)).toBe(300)
    expect(snapTicksWithSwing(480, 240, 50)).toBe(480)
  })

  it('keeps the base clip gain permanent while editing removable envelope points', () => {
    const store = useProjectStore.getState(); const track = store.project.tracks.find((item) => item.clips.length)!; const clip = track.clips[0]!
    const id = store.upsertClipGainPoint(track.id, clip.id, { timeSec: .5, valueDb: -6 })!
    store.updateClipGain(track.id, clip.id, 3)
    let result = useProjectStore.getState().project.tracks.find((item) => item.id === track.id)!.clips.find((item) => item.id === clip.id)!
    expect(result.gainDb).toBe(3)
    expect(result.gainPoints).toEqual([{ id, timeSec: .5, valueDb: -3 }])
    store.removeClipGainPoint(track.id, clip.id, id)
    result = useProjectStore.getState().project.tracks.find((item) => item.id === track.id)!.clips.find((item) => item.id === clip.id)!
    expect(result.gainPoints).toEqual([])
    expect(result.gainDb).toBe(3)
  })

  it('copies automation to the playhead, grid-duplicates it, and deletes the selected copies', () => {
    const store = useProjectStore.getState(); const track = store.project.tracks[0]!
    const option = automationOptionsForTrack(track).find((item) => item.parameterId === 'volumeDb')!
    store.addAutomationLane(track.id, option)
    const lane = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    const first = store.upsertAutomationPoint(track.id, lane.id, { timeSec: 1, value: -12 })!
    const second = store.upsertAutomationPoint(track.id, lane.id, { timeSec: 1.5, value: -6 })!
    store.selectAutomationPoint({ trackId: track.id, laneId: lane.id, pointId: first })
    store.selectAutomationPoint({ trackId: track.id, laneId: lane.id, pointId: second }, true)
    store.copySelectedAutomationPoints()
    store.pasteAutomationPoints(8)
    let result = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    expect(result.points.filter((point) => point.timeSec >= 8).map((point) => point.timeSec)).toEqual([8, 8.5])
    store.duplicateSelectedAutomationPoints()
    result = useProjectStore.getState().project.tracks[0]!.automationLanes![0]!
    expect(result.points.filter((point) => point.timeSec > 8.5)).toHaveLength(2)
    store.deleteSelectedAutomationPoints()
    expect(useProjectStore.getState().project.tracks[0]!.automationLanes![0]!.points).toHaveLength(4)
  })

  it('derives project, half, and double warp rates from the source tempo', () => {
    const clip = useProjectStore.getState().project.tracks.find((item) => item.clips.length)!.clips[0]!
    expect(clipSourceStep({ ...clip, warpMode: 'project', warpSourceBpm: 60, playbackRate: 1, pitchSemitones: 0 }, 120)).toBeCloseTo(2)
    expect(clipSourceStep({ ...clip, warpMode: 'half', warpSourceBpm: 60, playbackRate: 1, pitchSemitones: 0 }, 120)).toBeCloseTo(1)
    expect(clipSourceStep({ ...clip, warpMode: 'double', warpSourceBpm: 60, playbackRate: 1, pitchSemitones: 0 }, 120)).toBeCloseTo(4)
  })
})
