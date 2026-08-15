import type { MidiNote } from '../engine'

export function findVisibleNoteStart(notes: MidiNote[], tick: number) {
  let low = 0
  let high = notes.length
  while (low < high) {
    const middle = (low + high) >>> 1
    if (notes[middle]!.startTicks < tick) low = middle + 1
    else high = middle
  }
  return Math.max(0, low - 16)
}

/** Maps the full piano-gutter row width to a pitch, including black-key dead space. */
export function pianoPitchAtClientY(clientY: number, gutterTop: number, noteHeight: number): number | null {
  if (!Number.isFinite(clientY) || !Number.isFinite(gutterTop) || !Number.isFinite(noteHeight) || noteHeight <= 0) return null
  const row = Math.floor((clientY - gutterTop) / noteHeight)
  return row >= 0 && row < 128 ? 127 - row : null
}
