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
