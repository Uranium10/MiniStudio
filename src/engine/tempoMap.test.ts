import { describe, expect, it } from 'vitest'
import { TempoMap, defaultTempoMap } from './tempoMap'

describe('TempoMap', () => {
  it('preserves fixed-tempo conversion exactly', () => {
    const map = new TempoMap(defaultTempoMap(120))
    expect(map.ticksToSeconds(960)).toBe(.5)
    expect(map.secondsToTicks(.5)).toBe(960)
  })

  it('analytically converts and inverts a linear ramp', () => {
    const map = new TempoMap({
      tempoPoints: [{ tick: 0, bpm: 60, curve: 'linear' }, { tick: 9_600, bpm: 180, curve: 'jump' }],
      timeSignatures: [{ bar: 1, numerator: 4, denominator: 4 }],
    })
    for (let ticks = 0; ticks < 20_000; ticks += 137) expect(map.secondsToTicks(map.ticksToSeconds(ticks))).toBeCloseTo(ticks, 0)
    expect(map.bpmAtTick(4_800)).toBe(120)
  })

  it('keeps bar positions correct across signature changes', () => {
    const map = new TempoMap({
      tempoPoints: [{ tick: 0, bpm: 120, curve: 'jump' }],
      timeSignatures: [{ bar: 1, numerator: 4, denominator: 4 }, { bar: 5, numerator: 3, denominator: 4 }],
    })
    expect(map.barStartTicks(5)).toBe(15_360)
    expect(map.barStartTicks(6)).toBe(18_240)
    expect(map.tickToBarBeat(18_240)).toEqual({ bar: 6, beat: 0 })
  })
})
