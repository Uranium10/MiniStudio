import { describe, expect, it } from 'vitest'
import type { MidiNote } from '../engine'
import { findVisibleNoteStart } from './pianoRollMath'

describe('piano-roll viewport lookup', () => {
  it('keeps 5,000 sorted notes searchable without a linear scan', () => {
    const notes: MidiNote[] = Array.from({ length: 5_000 }, (_, index) => ({
      id: `note-${index}`,
      pitch: 36 + index % 60,
      velocity: 100,
      startTicks: index * 120,
      lengthTicks: 100,
      releaseVelocity: 64,
      muted: false,
    }))

    let checksum = 0
    const started = performance.now()
    for (let index = 0; index < 10_000; index += 1) {
      checksum += findVisibleNoteStart(notes, (index * 7919) % 600_000)
    }
    const elapsed = performance.now() - started

    expect(checksum).toBeGreaterThan(0)
    expect(elapsed).toBeLessThan(250)
  })
})
