export const DISPLAY_MIN_HZ = 10
export const DISPLAY_MAX_HZ = 30_000
export const PARAMETER_MIN_HZ = 20
export const PARAMETER_MAX_HZ = 20_000

export type FrequencyTick = { hz: number; label?: string; major: boolean }

/** Pro-audio logarithmic grid with extra landmarks throughout the low end. */
export const FREQUENCY_TICKS: readonly FrequencyTick[] = [
  { hz: 20, label: '20', major: true },
  { hz: 30, major: false }, { hz: 40, major: false },
  { hz: 50, label: '50', major: true },
  { hz: 60, major: false }, { hz: 70, major: false }, { hz: 80, major: false }, { hz: 90, major: false },
  { hz: 100, label: '100', major: true },
  { hz: 200, label: '200', major: true }, { hz: 300, major: false }, { hz: 400, major: false },
  { hz: 500, label: '500', major: true }, { hz: 600, major: false }, { hz: 700, major: false }, { hz: 800, major: false }, { hz: 900, major: false },
  { hz: 1_000, label: '1k', major: true },
  { hz: 2_000, label: '2k', major: true }, { hz: 3_000, major: false }, { hz: 4_000, major: false },
  { hz: 5_000, label: '5k', major: true }, { hz: 6_000, major: false }, { hz: 7_000, major: false }, { hz: 8_000, major: false }, { hz: 9_000, major: false },
  { hz: 10_000, label: '10k', major: true },
  { hz: 20_000, label: '20k', major: true },
]

export function frequencyToX(hz: number, width: number): number {
  const frequency = Math.max(DISPLAY_MIN_HZ, Math.min(DISPLAY_MAX_HZ, hz))
  return Math.log(frequency / DISPLAY_MIN_HZ) / Math.log(DISPLAY_MAX_HZ / DISPLAY_MIN_HZ) * width
}

export function displayFrequencyAtX(x: number, width: number): number {
  const ratio = Math.max(0, Math.min(1, x / Math.max(1, width)))
  return DISPLAY_MIN_HZ * (DISPLAY_MAX_HZ / DISPLAY_MIN_HZ) ** ratio
}

export function parameterFrequencyAtX(x: number, width: number): number {
  return Math.max(PARAMETER_MIN_HZ, Math.min(PARAMETER_MAX_HZ, displayFrequencyAtX(x, width)))
}

/** Native analyzer bins are logarithmically distributed from 20 Hz to 20 kHz. */
export function spectrumFrequencyAtIndex(index: number, length: number): number {
  if (length <= 1) return PARAMETER_MIN_HZ
  return PARAMETER_MIN_HZ * (PARAMETER_MAX_HZ / PARAMETER_MIN_HZ) ** (Math.max(0, Math.min(length - 1, index)) / (length - 1))
}
