import type { Clip } from '../engine'

export type ClipGainNode = { id?: string; timeSec: number; valueDb: number; curve: number }

/** Maximum midpoint displacement of a clip-gain quadratic segment. */
export const CLIP_GAIN_CURVE_MID_DB = 9

const DB_MIN = -60
const DB_MAX = 12
const DB_STEP = .1
const DB_TO_LINEAR = Float32Array.from(
  { length: Math.round((DB_MAX - DB_MIN) / DB_STEP) + 1 },
  (_, index) => 10 ** ((DB_MIN + index * DB_STEP) / 20),
)

/** Builds a sorted, immutable interpolation table once per clip redraw. */
export function prepareClipGainNodes(clip: Pick<Clip, 'durationSec' | 'gainDb' | 'gainPoints'>): ClipGainNode[] {
  return [
    { timeSec: 0, valueDb: clip.gainDb, curve: 0 },
    ...(clip.gainPoints ?? []).map((point) => ({ id: point.id, timeSec: Math.max(0, Math.min(clip.durationSec, point.timeSec)), valueDb: point.valueDb, curve: Math.max(-1, Math.min(1, point.curve ?? 0)) })).sort((left, right) => left.timeSec - right.timeSec),
    { timeSec: clip.durationSec, valueDb: clip.gainDb, curve: 0 },
  ]
}

/** O(log n) random-access sampling for hit tests and isolated UI queries. */
export function sampleClipGainDb(nodes: readonly ClipGainNode[], localSec: number): number {
  if (nodes.length < 2) return nodes[0]?.valueDb ?? 0
  let low = 0; let high = nodes.length - 1
  while (low + 1 < high) {
    const middle = (low + high) >>> 1
    if (nodes[middle]!.timeSec < localSec) low = middle
    else high = middle
  }
  const from = nodes[low]!; const to = nodes[high]!
  const mix = Math.max(0, Math.min(1, (localSec - from.timeSec) / Math.max(.000_001, to.timeSec - from.timeSec)))
  return quadraticGainDb(from, to, mix)
}

/** O(1) amortized sampling for left-to-right canvas traversal. */
export function createForwardClipGainSampler(nodes: readonly ClipGainNode[]): (localSec: number) => number {
  let index = 0; let previousTime = Number.NEGATIVE_INFINITY
  return (localSec) => {
    if (localSec < previousTime) index = 0
    previousTime = localSec
    while (index + 2 < nodes.length && localSec > nodes[index + 1]!.timeSec) index += 1
    const from = nodes[index] ?? { timeSec: 0, valueDb: 0, curve: 0 }
    const to = nodes[index + 1] ?? from
    const mix = Math.max(0, Math.min(1, (localSec - from.timeSec) / Math.max(.000_001, to.timeSec - from.timeSec)))
    return quadraticGainDb(from, to, mix)
  }
}

export function clipFadeAt(clip: Pick<Clip, 'durationSec' | 'fadeInSec' | 'fadeOutSec' | 'fadeInCurve' | 'fadeOutCurve'>, localSec: number): number {
  let fade = 1
  if (clip.fadeInSec > 0 && localSec < clip.fadeInSec) fade *= fadeShape(localSec / clip.fadeInSec, clip.fadeInCurve ?? 0)
  if (clip.fadeOutSec > 0 && localSec > clip.durationSec - clip.fadeOutSec) fade *= fadeShape((clip.durationSec - localSec) / clip.fadeOutSec, clip.fadeOutCurve ?? 0)
  return fade
}

/** 0.1 dB lookup avoids thousands of Math.pow calls while redrawing waveforms. */
export function dbToLinearFast(valueDb: number): number {
  const clipped = Math.max(DB_MIN, Math.min(DB_MAX, valueDb))
  return DB_TO_LINEAR[Math.round((clipped - DB_MIN) / DB_STEP)]!
}

function fadeShape(progress: number, curve: number): number {
  return Math.max(0, Math.min(1, progress)) ** (2 ** (Math.max(-1, Math.min(1, curve)) * 2))
}

function quadraticGainDb(from: ClipGainNode, to: ClipGainNode, mix: number): number {
  const control = (from.valueDb + to.valueDb) / 2 + from.curve * CLIP_GAIN_CURVE_MID_DB * 2
  const inverse = 1 - mix
  return inverse * inverse * from.valueDb + 2 * inverse * mix * control + mix * mix * to.valueDb
}
