// Clip clipboard, loop range, and effective mix-gain regression tests.
import { beforeEach, describe, expect, it } from 'vitest'
import { effectiveBusGainDb, effectiveMasterGainDb, MIDI_PPQ, MUTE_GAIN_DB, secondsPerBeat } from '../engine'
import { midiDuplicateStepTicks, studioOneDuplicateStepTicks, useProjectStore } from './projectStore'
import { createDemoProject } from './demoProject'

const reset = () => useProjectStore.getState().setProject(createDemoProject())
const audioTrack = () => useProjectStore.getState().project.tracks[0]!

describe('clip clipboard', () => {
  beforeEach(reset)

  it('pastes at the playhead while preserving relative spacing', () => {
    const store = useProjectStore.getState()
    const [first, second] = audioTrack().clips
    useProjectStore.setState({ selectedClipIds: [first!.id, second!.id], selectedTrackId: audioTrack().id, playheadSec: 100 })
    store.copySelectedClips()
    useProjectStore.getState().pasteClipboard()

    const pastedIds = new Set(useProjectStore.getState().selectedClipIds)
    const pasted = audioTrack().clips.filter((clip) => pastedIds.has(clip.id)).sort((a, b) => a.startSec - b.startSec)
    expect(pasted).toHaveLength(2)
    expect(pasted[0]!.startSec).toBeCloseTo(100, 6)
    expect(pasted[1]!.startSec - pasted[0]!.startSec).toBeCloseTo(second!.startSec - first!.startSec, 6)
  })

  it('gives pasted clips and their notes fresh ids', () => {
    const store = useProjectStore.getState()
    const source = audioTrack().clips[0]!
    useProjectStore.setState({ selectedClipIds: [source.id], selectedTrackId: audioTrack().id, playheadSec: 50 })
    store.copySelectedClips()
    useProjectStore.getState().pasteClipboard()
    expect(useProjectStore.getState().selectedClipIds[0]).not.toBe(source.id)
  })

  it('cut removes the originals and still fills the clipboard', () => {
    const store = useProjectStore.getState()
    const source = audioTrack().clips[0]!
    const before = audioTrack().clips.length
    useProjectStore.setState({ selectedClipIds: [source.id], selectedTrackId: audioTrack().id })
    store.cutSelectedClips()
    expect(audioTrack().clips).toHaveLength(before - 1)
    expect(useProjectStore.getState().clipboard?.entries).toHaveLength(1)
  })

  it('does nothing when nothing is selected', () => {
    useProjectStore.setState({ selectedClipIds: [], clipboard: null })
    useProjectStore.getState().copySelectedClips()
    expect(useProjectStore.getState().clipboard).toBeNull()
  })

  it('moves a MIDI clip between instrument tracks without changing its identity', () => {
    const source = useProjectStore.getState().project.tracks.find((track) => track.kind === 'instrument')!
    useProjectStore.getState().addInstrumentTrack()
    const target = useProjectStore.getState().project.tracks.at(-1)!
    const clip = source.midiClips[0]!

    expect(useProjectStore.getState().moveClipToTrack(source.id, target.id, clip.id)).toBe(true)
    expect(useProjectStore.getState().project.tracks.find((track) => track.id === source.id)!.midiClips).not.toContainEqual(expect.objectContaining({ id: clip.id }))
    expect(useProjectStore.getState().project.tracks.find((track) => track.id === target.id)!.midiClips).toContainEqual(expect.objectContaining({ id: clip.id }))
    expect(useProjectStore.getState().selectedTrackId).toBe(target.id)
  })

  it('rejects moving a MIDI clip onto an audio track', () => {
    const source = useProjectStore.getState().project.tracks.find((track) => track.kind === 'instrument')!
    const clip = source.midiClips[0]!
    expect(useProjectStore.getState().moveClipToTrack(source.id, audioTrack().id, clip.id)).toBe(false)
    expect(useProjectStore.getState().project.tracks.find((track) => track.id === source.id)!.midiClips).toContainEqual(expect.objectContaining({ id: clip.id }))
  })

  it('D-style duplication advances selected arrangement clips by the next musical block', () => {
    const store = useProjectStore.getState()
    const originals = audioTrack().clips.slice(0, 2)
    const untouchedInstrument = store.project.tracks.find((track) => track.kind === 'instrument')!
    const start = Math.min(...originals.map((clip) => clip.startSec))
    const end = Math.max(...originals.map((clip) => clip.startSec + clip.durationSec))
    const ticksPerSecond = MIDI_PPQ / secondsPerBeat(store.project.transport.bpm)
    const extentTicks = Math.max(1, Math.ceil((end - start) * ticksPerSecond - 1e-6))
    const expectedDelta = studioOneDuplicateStepTicks(extentTicks, store.gridTicks, store.snapEnabled) / ticksPerSecond
    useProjectStore.setState({ selectedClipIds: originals.map((clip) => clip.id), editFocus: 'arrangement' })
    store.duplicateSelectedClipsSmart()
    const selected = new Set(useProjectStore.getState().selectedClipIds)
    const copies = audioTrack().clips.filter((clip) => selected.has(clip.id)).sort((a, b) => a.startSec - b.startSec)

    expect(copies).toHaveLength(originals.length)
    expect(copies.map((clip) => clip.startSec)).toEqual(originals.map((clip) => clip.startSec + expectedDelta))
    expect(useProjectStore.getState().project.tracks.find((track) => track.id === untouchedInstrument.id)).toBe(untouchedInstrument)
  })

  it('tracks arrangement and piano-roll focus independently of panel visibility', () => {
    const track = useProjectStore.getState().project.tracks.find((item) => item.kind === 'instrument')!
    const note = track.midiClips[0]!.notes[0]!
    useProjectStore.getState().selectMidiNote(note.id)
    expect(useProjectStore.getState().editFocus).toBe('pianoRoll')
    useProjectStore.getState().selectClip(audioTrack().clips[0]!.id)
    expect(useProjectStore.getState().editFocus).toBe('arrangement')
  })
})

describe('MIDI playback defaults', () => {
  beforeEach(reset)

  it('does not silently loop the demo pattern after the playhead passes it', () => {
    const instrument = useProjectStore.getState().project.tracks.find((track) => track.kind === 'instrument')!
    expect(instrument.midiClips[0]?.loopEnabled).toBe(false)
  })

  it('clips piano-roll horizontal zoom to its usable range', () => {
    useProjectStore.getState().setPianoRollZoom(1)
    expect(useProjectStore.getState().pianoRollZoom).toBe(32)
    useProjectStore.getState().setPianoRollZoom(999)
    expect(useProjectStore.getState().pianoRollZoom).toBe(160)
  })
})

describe('MIDI note duplication', () => {
  beforeEach(reset)

  it('uses half-bar, bar, then doubled-bar blocks for smart duplication', () => {
    const signature = { numerator: 4, denominator: 4 }
    expect(midiDuplicateStepTicks([{ startTicks: 0, lengthTicks: 240 }], signature)).toBe(1920)
    expect(midiDuplicateStepTicks([{ startTicks: 0, lengthTicks: 3840 }], signature)).toBe(3840)
    expect(midiDuplicateStepTicks([{ startTicks: 0, lengthTicks: 3841 }], signature)).toBe(7680)
  })

  it('copies selected notes with fresh ids while preserving the originals', () => {
    const store = useProjectStore.getState()
    const track = store.project.tracks.find((item) => item.kind === 'instrument')!
    const clip = track.midiClips[0]!
    const originals = clip.notes.slice(0, 3)
    const before = clip.notes.length
    const ids = store.duplicateMidiNotes(track.id, clip.id, originals.map((note) => note.id), 1920, 12)
    const updated = useProjectStore.getState().project.tracks.find((item) => item.id === track.id)!.midiClips[0]!
    const copies = updated.notes.filter((note) => ids.includes(note.id))

    expect(updated.notes).toHaveLength(before + originals.length)
    expect(ids).toHaveLength(originals.length)
    expect(ids.every((id) => !originals.some((note) => note.id === id))).toBe(true)
    expect(copies.map((note) => note.startTicks)).toEqual(originals.map((note) => note.startTicks + 1920))
    expect(copies.map((note) => note.pitch)).toEqual(originals.map((note) => note.pitch + 12))
    expect(useProjectStore.getState().selectedNoteIds).toEqual(ids)
  })

  it('D-style duplication advances to the next logical piano grid block', () => {
    const store = useProjectStore.getState()
    const track = store.project.tracks.find((item) => item.kind === 'instrument')!
    const clip = track.midiClips[0]!
    const originals = clip.notes.slice(0, 3)
    useProjectStore.setState({ editorClip: { trackId: track.id, clipId: clip.id }, pianoRollOpen: true, selectedNoteIds: originals.map((note) => note.id) })
    store.duplicateSelectedMidiNotes()
    const updated = useProjectStore.getState().project.tracks.find((item) => item.id === track.id)!.midiClips[0]!
    const selected = new Set(useProjectStore.getState().selectedNoteIds)
    const copies = updated.notes.filter((note) => selected.has(note.id))
    const start = Math.min(...originals.map((note) => note.startTicks))
    const end = Math.max(...originals.map((note) => note.startTicks + note.lengthTicks))
    const delta = studioOneDuplicateStepTicks(end - start, store.pianoGridTicks, store.pianoSnapEnabled)
    expect(copies.map((note) => note.startTicks)).toEqual(originals.map((note) => note.startTicks + delta))
  })

  it('uses the exact selected extent when piano Snap is off', () => {
    expect(studioOneDuplicateStepTicks(777, 240, false)).toBe(777)
  })
})

describe('loop range', () => {
  beforeEach(reset)

  it('normalises a backwards drag and enables the loop', () => {
    useProjectStore.getState().setLoopRange(30, 12)
    const loop = useProjectStore.getState().project.transport.loop
    expect(loop.startSec).toBeCloseTo(12, 6)
    expect(loop.endSec).toBeCloseTo(30, 6)
    expect(loop.enabled).toBe(true)
  })

  it('spans the full extent of the selected clips', () => {
    const track = audioTrack()
    const [first, second] = track.clips
    useProjectStore.setState({ selectedClipIds: [first!.id, second!.id] })
    useProjectStore.getState().setLoopToSelection()
    const loop = useProjectStore.getState().project.transport.loop
    expect(loop.startSec).toBeCloseTo(Math.min(first!.startSec, second!.startSec), 6)
    expect(loop.endSec).toBeCloseTo(Math.max(first!.startSec + first!.durationSec, second!.startSec + second!.durationSec), 6)
  })
})

describe('effective mix gains', () => {
  it('drops a muted bus to the silence floor', () => {
    expect(effectiveBusGainDb({ id: 'b', name: 'B', effects: [], volumeDb: -6, muted: false })).toBe(-6)
    expect(effectiveBusGainDb({ id: 'b', name: 'B', effects: [], volumeDb: -6, muted: true })).toBe(MUTE_GAIN_DB)
  })

  it('applies dim as a -20 dB offset and lets mute win over it', () => {
    expect(effectiveMasterGainDb({ volumeDb: -1, effects: [], muted: false, dim: false })).toBe(-1)
    expect(effectiveMasterGainDb({ volumeDb: -1, effects: [], muted: false, dim: true })).toBe(-21)
    expect(effectiveMasterGainDb({ volumeDb: -1, effects: [], muted: true, dim: true })).toBe(MUTE_GAIN_DB)
  })
})
