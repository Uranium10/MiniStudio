// Deterministic demo session used until the user imports or opens a project.
import type { AudioAssetInfo, Clip, EffectInstance, MidiClip, MidiNote, ProjectState, Track } from '../engine'

const colors = ['#ff7a45', '#f2bd3f', '#28c2a0', '#55a7ff', '#a67cff', '#ef6c9a']
const names = ['Lead Vocal', 'Harmony Stack', 'Neon Drums', 'Midnight Bass', 'Glass Keys', 'Atmosphere']

function makePeaks(seed: number, buckets = 520): Float32Array {
  const values = new Float32Array(buckets * 2)
  let state = seed * 9973
  for (let i = 0; i < buckets; i += 1) {
    state = (state * 16807) % 2147483647
    const envelope = 0.18 + ((state % 1000) / 1000) * 0.72
    const pulse = 0.55 + Math.sin(i * (0.06 + seed * 0.008)) * 0.35
    const peak = Math.min(0.98, Math.max(0.04, envelope * pulse))
    values[i * 2] = -peak
    values[i * 2 + 1] = peak
  }
  return values
}

function effect(id: string, type: EffectInstance['type'], params: Record<string, number>): EffectInstance {
  return { id, type, bypassed: false, params }
}

function clip(track: number, index: number, startSec: number, durationSec: number): Clip {
  return {
    id: `clip-${track}-${index}`,
    assetId: `asset-${track}`,
    name: index === 0 ? names[track] : `${names[track]} ${index + 1}`,
    startSec,
    offsetSec: 0,
    durationSec,
    gainDb: 0,
    fadeInSec: index % 2 === 0 ? 0.12 : 0,
    fadeOutSec: 0.18,
  }
}

function makeTrack(index: number): Track {
  const clipLayouts = [
    [[0, 8], [10, 6], [20, 10], [34, 8]],
    [[2, 5], [12, 8], [26, 6], [36, 7]],
    [[0, 16], [16.5, 16], [33, 12]],
    [[0, 10], [12, 10], [24, 10], [36, 9]],
    [[3, 7], [14, 12], [30, 10]],
    [[0, 13], [15, 13], [30, 15]],
  ][index] ?? []
  const effects = index === 0
    ? [effect('fx-eq-1', 'builtin:eq', { lowGain: -1.5, midGain: 2.5, midFreq: 1800, highGain: 1.2 }), effect('fx-comp-1', 'builtin:compressor', { threshold: -18, ratio: 3, attack: 0.01, release: 0.2, knee: 12 })]
    : index === 4
      ? [effect('fx-delay-1', 'builtin:delay', { time: 0.28, feedback: 0.35, mix: 0.22 })]
      : []
  return {
    id: `track-${index}`,
    kind: 'audio',
    name: names[index] ?? `Audio ${index + 1}`,
    color: colors[index] ?? '#8a97a8',
    clips: clipLayouts.map(([start, duration], clipIndex) => clip(index, clipIndex, start ?? 0, duration ?? 1)),
    midiClips: [],
    instrument: null,
    volumeDb: [-2.8, -5.4, -1.2, -4.1, -7.3, -8.5][index] ?? -6,
    pan: [-0.05, 0.22, 0, -0.12, 0.3, -0.25][index] ?? 0,
    muted: false,
    solo: false,
    armed: index === 0,
    effects,
    sends: [
      { id: `send-${index}-a`, targetBusId: 'bus-a', gainDb: -14 + index, preFader: false },
      { id: `send-${index}-b`, targetBusId: 'bus-b', gainDb: -20 + index, preFader: false },
    ],
  }
}

function makeMidiNote(id: string, pitch: number, startTicks: number, lengthTicks = 720, velocity = 100): MidiNote {
  return { id, pitch, velocity, startTicks, lengthTicks, releaseVelocity: 64, muted: false }
}

function makeMidiClip(): MidiClip {
  const notes: MidiNote[] = []
  const chords = [[48, 55, 60], [46, 53, 58], [43, 50, 55], [46, 53, 60]]
  for (let bar = 0; bar < 4; bar += 1) {
    for (let beat = 0; beat < 4; beat += 1) {
      const chord = chords[bar]!
      for (const [voice, pitch] of chord.entries()) notes.push(makeMidiNote(`demo-note-${bar}-${beat}-${voice}`, pitch, (bar * 4 + beat) * 960, 820, 82 + voice * 10))
      notes.push(makeMidiNote(`demo-note-top-${bar}-${beat}`, 72 + [0, 3, 7, 5][bar]!, (bar * 4 + beat) * 960 + 480, 360, 108))
    }
  }
  return { id: 'midi-clip-demo', name: 'Test Tone Pattern', startSec: 0, durationSec: 48, loopEnabled: false, loopStartTicks: 0, loopLengthTicks: 15_360, notes, ccLanes: [], transposeSemitones: 0, velocityScale: 1, muted: false, color: '#66d3ff' }
}

function makeInstrumentTrack(): Track {
  return {
    id: 'track-instrument-demo', kind: 'instrument', name: 'Test Tone', color: '#66d3ff', clips: [], midiClips: [makeMidiClip()],
    instrument: { id: 'instrument-testtone-demo', type: 'builtin:testtone', bypassed: false, params: { waveform: 2, attack: 0.008, decay: 0.16, sustain: 0.62, release: 0.28, gainDb: -14, polyphony: 16, velocityCurve: 1 } },
    volumeDb: -3, pan: 0, muted: false, solo: false, armed: false, effects: [],
    sends: [{ id: 'send-instrument-a', targetBusId: 'bus-a', gainDb: -18, preFader: false }, { id: 'send-instrument-b', targetBusId: 'bus-b', gainDb: -24, preFader: false }],
  }
}

export function createDemoProject(): ProjectState {
  const assets: Record<string, AudioAssetInfo> = {}
  for (let index = 0; index < names.length; index += 1) {
    assets[`asset-${index}`] = {
      id: `asset-${index}`,
      path: '',
      name: `${names[index]}.wav`,
      durationSec: 48,
      sampleRate: 48_000,
      numChannels: 2,
      peaks: makePeaks(index + 1),
    }
  }
  return {
    formatVersion: 2,
    meta: { name: 'Aurora Session', sampleRate: 48_000, createdAt: new Date().toISOString() },
    transport: {
      bpm: 118,
      timeSignature: { numerator: 4, denominator: 4 },
      playheadSec: 6.4,
      isPlaying: false,
      loop: { enabled: true, startSec: 8, endSec: 40 },
    },
    assets,
    tracks: [...names.map((_, index) => makeTrack(index)), makeInstrumentTrack()],
    buses: [
      { id: 'bus-a', name: 'A · Space', volumeDb: -5, muted: false, effects: [effect('fx-reverb-a', 'builtin:reverb', { decaySec: 2.8, damping: 0.42, width: 0.85, mix: 0.3 })] },
      { id: 'bus-b', name: 'B · Echo', volumeDb: -7, muted: false, effects: [effect('fx-delay-b', 'builtin:delay', { time: 0.375, feedback: 0.42, mix: 0.3, damping: 0.35 })] },
    ],
    master: { volumeDb: -1, muted: false, dim: false, effects: [effect('fx-master-eq', 'builtin:eq', { lowGain: 0.5, midGain: -0.8, midFreq: 340, highGain: 1.2 }), effect('fx-master-comp', 'builtin:compressor', { threshold: -8, ratio: 2, attack: 0.03, release: 0.25, knee: 8, makeupDb: 1 }), effect('fx-master-shaper', 'builtin:waveshaper', { driveDb: 3, curve: 0, outputDb: 0, mix: 0.18, oversample: 4, dcBlock: 1, autoLevel: 1 })] },
  }
}

/** A clean session used by File > New Project. */
export function createEmptyProject(): ProjectState {
  return {
    formatVersion: 2,
    meta: { name: 'Untitled Project', sampleRate: 48_000, createdAt: new Date().toISOString() },
    transport: {
      bpm: 120,
      timeSignature: { numerator: 4, denominator: 4 },
      playheadSec: 0,
      isPlaying: false,
      loop: { enabled: false, startSec: 0, endSec: 8 },
    },
    assets: {},
    tracks: [],
    buses: [
      { id: 'bus-a', name: 'A · Space', volumeDb: -6, muted: false, effects: [] },
      { id: 'bus-b', name: 'B · Echo', volumeDb: -6, muted: false, effects: [] },
    ],
    master: { volumeDb: 0, muted: false, dim: false, effects: [] },
  }
}
