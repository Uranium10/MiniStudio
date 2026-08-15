import { describe, expect, it } from 'vitest'
import { hexToHsp, hspToHex, perceivedBrightness } from './colorMath'

describe('HSP track colours', () => {
  it('measures perceived brightness with the HSP channel weights', () => {
    expect(perceivedBrightness(1, 1, 1)).toBeCloseTo(1)
    expect(perceivedBrightness(1, 0, 0)).toBeCloseTo(Math.sqrt(.299))
    expect(hexToHsp('#000000').p).toBe(0)
    expect(hexToHsp('#ffffff').p).toBe(100)
  })

  it('produces in-gamut colours at the requested perceived brightness', () => {
    const hex = hspToHex({ h: 205, s: 78, p: 55 })
    const result = hexToHsp(hex)
    expect(hex).toMatch(/^#[\da-f]{6}$/)
    expect(result.h).toBeCloseTo(205, 0)
    expect(result.s).toBeCloseTo(78, 0)
    expect(result.p).toBeCloseTo(55, 0)
  })
})
