import { describe, expect, it } from 'vitest'
import type { Clip } from '../engine'
import { clipFadeAt, createForwardClipGainSampler, dbToLinearFast, prepareClipGainNodes, sampleClipGainDb } from './clipEnvelopeMath'

const clip = { durationSec: 4, gainDb: 0, fadeInSec: 1, fadeOutSec: 1, gainPoints: [{ id: 'a', timeSec: 1, valueDb: -12 }, { id: 'b', timeSec: 3, valueDb: -6 }] } as Clip

describe('clip envelope drawing math', () => {
  it('interpolates sorted gain points identically for random and forward sampling', () => {
    const nodes = prepareClipGainNodes(clip)
    const forward = createForwardClipGainSampler(nodes)
    for (const time of [0, .5, 1, 2, 3, 3.5, 4]) expect(forward(time)).toBeCloseTo(sampleClipGainDb(nodes, time), 6)
    expect(sampleClipGainDb(nodes, 2)).toBeCloseTo(-9)
  })

  it('bends the outgoing gain segment quadratically', () => {
    const nodes = prepareClipGainNodes({ ...clip, gainPoints: [{ id: 'a', timeSec: 1, valueDb: -12, curve: 1 }, { id: 'b', timeSec: 3, valueDb: -6 }] })
    expect(sampleClipGainDb(nodes, 2)).toBeCloseTo(0)
    expect(sampleClipGainDb(nodes, 1)).toBeCloseTo(-12)
    expect(sampleClipGainDb(nodes, 3)).toBeCloseTo(-6)
  })

  it('applies fade bounds and uses an accurate dB lookup', () => {
    expect(clipFadeAt(clip, 0)).toBe(0)
    expect(clipFadeAt(clip, 2)).toBe(1)
    expect(clipFadeAt(clip, 4)).toBe(0)
    expect(dbToLinearFast(-6)).toBeCloseTo(10 ** (-6 / 20), 5)
  })
})
