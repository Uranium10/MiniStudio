import { describe, expect, it } from 'vitest'
import { reorderDestinationIndex } from './pointerReorder'

describe('pointer reorder insertion index', () => {
  it('lands on the visible before/after edge in both directions', () => {
    expect(reorderDestinationIndex(0, 2, false, 4)).toBe(1)
    expect(reorderDestinationIndex(0, 2, true, 4)).toBe(2)
    expect(reorderDestinationIndex(3, 1, false, 4)).toBe(1)
    expect(reorderDestinationIndex(3, 1, true, 4)).toBe(2)
  })

  it('keeps the source stable over either half of itself', () => {
    expect(reorderDestinationIndex(2, 2, false, 5)).toBe(2)
    expect(reorderDestinationIndex(2, 2, true, 5)).toBe(2)
  })
})
