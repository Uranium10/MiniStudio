// Ruler tick layout regression tests.
import { describe, expect, it } from 'vitest'
import { buildRulerTicks } from './rulerMath'

const FOUR_FOUR = { numerator: 4, denominator: 4 }

describe('buildRulerTicks', () => {
  it('emits beat ticks with a strong tick on every downbeat at 120 bpm', () => {
    const ticks = buildRulerTicks(2000, 100, 120, FOUR_FOUR)
    expect(ticks[0]).toMatchObject({ sec: 0, strong: true, label: '1' })
    // One beat is 0.5s at 120 bpm, so bar two starts on the fifth tick.
    expect(ticks[4]).toMatchObject({ sec: 2, strong: true, label: '2' })
    expect(ticks[1]?.strong).toBe(false)
  })

  it('falls back to bar ticks once beats would be closer than 18px', () => {
    const zoomedOut = buildRulerTicks(4000, 6, 120, FOUR_FOUR)
    // Every tick is a bar line, so all of them are strong.
    expect(zoomedOut.every((tick) => tick.strong)).toBe(true)
    expect(zoomedOut[1]?.sec).toBeCloseTo(2, 6)
  })

  it('thins labels out instead of overprinting them when zoomed far out', () => {
    const ticks = buildRulerTicks(4000, 5, 120, FOUR_FOUR)
    const labelled = ticks.filter((tick) => tick.label !== null)
    for (let index = 1; index < labelled.length; index += 1) {
      expect((labelled[index]!.sec - labelled[index - 1]!.sec) * 5).toBeGreaterThanOrEqual(54)
    }
  })

  it('honours the time signature when placing downbeats', () => {
    const ticks = buildRulerTicks(2000, 100, 120, { numerator: 3, denominator: 4 })
    // Three beats per bar, so bar two is the fourth tick.
    expect(ticks[3]).toMatchObject({ sec: 1.5, strong: true, label: '2' })
    expect(ticks[1]?.strong).toBe(false)
  })

  it('keeps every bar label while dropping off-beat labels at tight zoom', () => {
    // 200px per bar leaves room for bar numbers but not for beat numbers.
    const ticks = buildRulerTicks(2000, 100, 120, FOUR_FOUR)
    expect(ticks.filter((tick) => tick.strong).every((tick) => tick.label !== null)).toBe(true)
    expect(ticks.filter((tick) => !tick.strong).every((tick) => tick.label === null)).toBe(true)
  })

  it('labels individual beats once they are far enough apart', () => {
    const ticks = buildRulerTicks(2000, 300, 120, FOUR_FOUR)
    expect(ticks[1]).toMatchObject({ strong: false, label: '1.2' })
  })

  it('returns nothing for a degenerate zoom or tempo', () => {
    expect(buildRulerTicks(1000, 0, 120, FOUR_FOUR)).toEqual([])
  })
})
