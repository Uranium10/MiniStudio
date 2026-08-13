import { describe, expect, it } from 'vitest'
import { displayFrequencyAtX, frequencyToX, parameterFrequencyAtX, spectrumFrequencyAtIndex } from './frequencyScale'

describe('commercial logarithmic frequency scale', () => {
  it('round-trips audible landmarks on the extended 10 Hz – 30 kHz display', () => {
    for (const frequency of [20, 50, 100, 200, 500, 1_000, 5_000, 20_000]) {
      expect(displayFrequencyAtX(frequencyToX(frequency, 1000), 1000)).toBeCloseTo(frequency, 8)
    }
  })

  it('keeps parameter gestures inside the supported 20 Hz – 20 kHz range', () => {
    expect(parameterFrequencyAtX(0, 1000)).toBe(20)
    expect(parameterFrequencyAtX(1000, 1000)).toBe(20_000)
  })

  it('maps logarithmic analyzer bins back to their real frequencies', () => {
    expect(spectrumFrequencyAtIndex(0, 49)).toBe(20)
    expect(spectrumFrequencyAtIndex(16, 49)).toBeCloseTo(200, 6)
    expect(spectrumFrequencyAtIndex(32, 49)).toBeCloseTo(2_000, 6)
    expect(spectrumFrequencyAtIndex(48, 49)).toBeCloseTo(20_000, 6)
  })
})
