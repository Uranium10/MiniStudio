// Zoom-adaptive bar/beat tick layout for the arrangement ruler.
import { secondsPerBar, secondsPerBeat, type TimeSignature } from '../engine'

export type RulerTick = { sec: number; label: string | null; strong: boolean }

/** Minimum on-screen spacing before beat ticks collapse back to bar ticks. */
const MIN_BEAT_SPACING_PX = 18
/** Minimum on-screen spacing between two printed labels. */
const MIN_LABEL_SPACING_PX = 54

export function buildRulerTicks(width: number, pixelsPerSecond: number, bpm: number, signature: TimeSignature): RulerTick[] {
  const barSec = secondsPerBar(bpm, signature)
  const beatSec = secondsPerBeat(bpm) * (4 / signature.denominator)
  if (!(barSec > 0) || !(beatSec > 0) || !(pixelsPerSecond > 0)) return []
  // Use beat ticks only while they stay readable, then fall back to bar ticks, and
  // thin the labels independently so they never overprint at any zoom level.
  const useBeats = beatSec * pixelsPerSecond >= MIN_BEAT_SPACING_PX
  const unitSec = useBeats ? beatSec : barSec
  const beatsPerBar = Math.max(1, Math.round(barSec / beatSec))
  // Bar numbers are the primary labels and get an even stride of their own, so
  // zooming out never drops a downbeat label while keeping an off-beat one.
  const barStride = Math.max(1, Math.ceil(MIN_LABEL_SPACING_PX / (barSec * pixelsPerSecond)))
  const labelBeats = useBeats && beatSec * pixelsPerSecond >= MIN_LABEL_SPACING_PX
  const total = Math.min(4000, Math.ceil(width / pixelsPerSecond / unitSec) + 1)
  const ticks: RulerTick[] = []
  for (let index = 0; index < total; index += 1) {
    const bar = useBeats ? Math.floor(index / beatsPerBar) : index
    const beat = useBeats ? index % beatsPerBar : 0
    const strong = beat === 0
    ticks.push({
      sec: index * unitSec,
      strong,
      label: strong ? (bar % barStride === 0 ? String(bar + 1) : null) : labelBeats ? `${bar + 1}.${beat + 1}` : null,
    })
  }
  return ticks
}
