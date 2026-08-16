// Zoom-adaptive bar/beat tick layout for the arrangement ruler.
import { MIDI_PPQ, secondsPerBar, secondsPerBeat, TempoMap, type TempoMapData, type TimeSignature } from '../engine'

export type RulerTick = { sec: number; label: string | null; strong: boolean }

/** Variable-tempo-aware editing grid. The step remains musical while its pixel spacing varies with tempo. */
export function buildMusicalGridLines(width: number, pixelsPerSecond: number, gridTicks: number, tempoMapData: TempoMapData, minimumPixels = 8): RulerTick[] {
  if (!(width > 0) || !(pixelsPerSecond > 0) || !(gridTicks > 0)) return []
  const map = new TempoMap(tempoMapData)
  const endTick = map.secondsToTicks(width / pixelsPerSecond)
  let paintedTicks = Math.max(1, Math.round(gridTicks))
  while (paintedTicks < Number.MAX_SAFE_INTEGER / 2) {
    const firstPx = (map.ticksToSeconds(paintedTicks) - map.ticksToSeconds(0)) * pixelsPerSecond
    const lastPx = (map.ticksToSeconds(endTick + paintedTicks) - map.ticksToSeconds(endTick)) * pixelsPerSecond
    if (Math.min(firstPx, lastPx) >= minimumPixels) break
    paintedTicks *= 2
  }
  const lines: RulerTick[] = []
  let lastX = Number.NEGATIVE_INFINITY
  for (let tick = 0, count = 0; tick <= endTick + paintedTicks && count < 4_000; tick += paintedTicks, count += 1) {
    const sec = map.ticksToSeconds(tick)
    const x = sec * pixelsPerSecond
    if (x - lastX < 3) continue
    lines.push({ sec, strong: Math.abs(map.tickToBarBeat(tick).beat) < 1e-6, label: null })
    lastX = x
  }
  return lines
}

/** Coarsen only the painted subdivision; editing continues to use the exact snap value. */
export function adaptiveGridStepSec(baseStepSec: number, pixelsPerSecond: number, minimumPixels = 8): number {
  if (!(baseStepSec > 0) || !(pixelsPerSecond > 0)) return 0
  let step = baseStepSec
  while (step * pixelsPerSecond < minimumPixels) step *= 2
  return step
}

/** Minimum on-screen spacing before beat ticks collapse back to bar ticks. */
const MIN_BEAT_SPACING_PX = 18
/** Minimum on-screen spacing between two printed labels. */
const MIN_LABEL_SPACING_PX = 54

export function buildRulerTicks(width: number, pixelsPerSecond: number, bpm: number, signature: TimeSignature, tempoMapData?: TempoMapData): RulerTick[] {
  if (tempoMapData && pixelsPerSecond > 0) return buildMappedRulerTicks(width, pixelsPerSecond, new TempoMap(tempoMapData))
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

function buildMappedRulerTicks(width: number, pixelsPerSecond: number, map: TempoMap): RulerTick[] {
  const maxSec = width / pixelsPerSecond
  const endTick = map.secondsToTicks(maxSec)
  const startBeatSec = map.ticksToSeconds(MIDI_PPQ) - map.ticksToSeconds(0)
  const endBeatSec = map.ticksToSeconds(endTick + MIDI_PPQ) - map.ticksToSeconds(endTick)
  const useBeats = Math.min(startBeatSec, endBeatSec) * pixelsPerSecond >= MIN_BEAT_SPACING_PX
  const ticks: RulerTick[] = []
  let lastLabelX = -MIN_LABEL_SPACING_PX
  for (let bar = 1; bar <= 100_000 && ticks.length < 4_000; bar += 1) {
    const barTick = map.barStartTicks(bar)
    const barSec = map.ticksToSeconds(barTick)
    if (barSec > maxSec + 1) break
    const barX = barSec * pixelsPerSecond
    const label = barX - lastLabelX >= MIN_LABEL_SPACING_PX ? String(bar) : null
    if (label) lastLabelX = barX
    ticks.push({ sec: barSec, strong: true, label })
    if (!useBeats) continue
    const signature = map.timeSignatureAtBar(bar)
    const beatTicks = MIDI_PPQ * 4 / signature.denominator
    for (let beat = 1; beat < signature.numerator && ticks.length < 4_000; beat += 1) {
      const sec = map.ticksToSeconds(barTick + beat * beatTicks)
      const x = sec * pixelsPerSecond
      const beatLabel = x - lastLabelX >= MIN_LABEL_SPACING_PX ? `${bar}.${beat + 1}` : null
      if (beatLabel) lastLabelX = x
      ticks.push({ sec, strong: false, label: beatLabel })
    }
  }
  return ticks
}
