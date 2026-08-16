import { MIDI_PPQ, type TempoMapData, type TempoPoint, type TimeSignaturePoint } from './types'

const DEFAULT_TEMPO = 120
const VALID_DENOMINATORS = new Set([1, 2, 4, 8, 16, 32])

export function defaultTempoMap(bpm = DEFAULT_TEMPO, numerator = 4, denominator = 4): TempoMapData {
  return {
    tempoPoints: [{ tick: 0, bpm: clipBpm(bpm), curve: 'jump' }],
    timeSignatures: [{ bar: 1, numerator: clipNumerator(numerator), denominator: clipDenominator(denominator) }],
  }
}

export function normalizeTempoMap(source: Partial<TempoMapData> | null | undefined, fallbackBpm = DEFAULT_TEMPO, fallbackNumerator = 4, fallbackDenominator = 4): TempoMapData {
  const tempoPoints = (source?.tempoPoints ?? [])
    .filter((point): point is TempoPoint => Number.isFinite(point?.tick) && Number.isFinite(point?.bpm))
    .map((point) => ({ tick: Math.max(0, Math.round(point.tick)), bpm: clipBpm(point.bpm), curve: point.curve === 'linear' ? 'linear' as const : 'jump' as const }))
    .sort((a, b) => a.tick - b.tick)
    .filter((point, index, values) => index === values.length - 1 || values[index + 1]!.tick !== point.tick)
  if (!tempoPoints.length || tempoPoints[0]!.tick !== 0) tempoPoints.unshift({ tick: 0, bpm: tempoPoints[0]?.bpm ?? clipBpm(fallbackBpm), curve: 'jump' })

  const timeSignatures = (source?.timeSignatures ?? [])
    .filter((point): point is TimeSignaturePoint => Number.isFinite(point?.bar) && Number.isFinite(point?.numerator) && Number.isFinite(point?.denominator))
    .map((point) => ({ bar: Math.max(1, Math.round(point.bar)), numerator: clipNumerator(point.numerator), denominator: clipDenominator(point.denominator) }))
    .sort((a, b) => a.bar - b.bar)
    .filter((point, index, values) => index === values.length - 1 || values[index + 1]!.bar !== point.bar)
  if (!timeSignatures.length || timeSignatures[0]!.bar !== 1) timeSignatures.unshift({ bar: 1, numerator: clipNumerator(fallbackNumerator), denominator: clipDenominator(fallbackDenominator) })
  return { tempoPoints, timeSignatures }
}

/** Prepared musical-time converter. Construction allocates; conversions do not. */
export class TempoMap {
  readonly data: TempoMapData
  private readonly cumulativeSeconds: number[]
  private readonly signatureStartTicks: number[]

  constructor(source: TempoMapData) {
    this.data = normalizeTempoMap(source)
    this.cumulativeSeconds = new Array(this.data.tempoPoints.length).fill(0)
    for (let index = 1; index < this.data.tempoPoints.length; index += 1) {
      const previous = this.data.tempoPoints[index - 1]!
      const next = this.data.tempoPoints[index]!
      this.cumulativeSeconds[index] = this.cumulativeSeconds[index - 1]! + this.segmentTicksToSeconds(index - 1, next.tick - previous.tick)
    }
    this.signatureStartTicks = new Array(this.data.timeSignatures.length).fill(0)
    for (let index = 1; index < this.data.timeSignatures.length; index += 1) {
      const previous = this.data.timeSignatures[index - 1]!
      const next = this.data.timeSignatures[index]!
      this.signatureStartTicks[index] = this.signatureStartTicks[index - 1]! + (next.bar - previous.bar) * ticksPerBar(previous)
    }
  }

  ticksToSeconds(ticks: number): number {
    const safe = Math.max(0, Math.round(ticks))
    const index = upperBound(this.data.tempoPoints, safe, (point) => point.tick) - 1
    return this.cumulativeSeconds[index]! + this.segmentTicksToSeconds(index, safe - this.data.tempoPoints[index]!.tick)
  }

  secondsToTicks(seconds: number): number {
    const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0)
    const index = upperBoundNumbers(this.cumulativeSeconds, safe) - 1
    return Math.max(0, Math.round(this.data.tempoPoints[index]!.tick + this.segmentSecondsToTicks(index, safe - this.cumulativeSeconds[index]!)))
  }

  bpmAtTick(ticks: number): number {
    const safe = Math.max(0, ticks)
    const index = upperBound(this.data.tempoPoints, safe, (point) => point.tick) - 1
    const point = this.data.tempoPoints[index]!
    const next = this.data.tempoPoints[index + 1]
    if (point.curve !== 'linear' || !next || next.tick <= point.tick) return point.bpm
    return point.bpm + (next.bpm - point.bpm) * Math.min(1, (safe - point.tick) / (next.tick - point.tick))
  }

  barStartTicks(bar: number): number { return this.barBeatToTick(bar, 0) }

  timeSignatureAtBar(bar: number): TimeSignaturePoint {
    const safeBar = Math.max(1, Math.round(bar))
    return this.data.timeSignatures[upperBound(this.data.timeSignatures, safeBar, (point) => point.bar) - 1]!
  }

  barBeatToTick(bar: number, beat: number): number {
    const safeBar = Math.max(1, Math.round(bar))
    const index = upperBound(this.data.timeSignatures, safeBar, (point) => point.bar) - 1
    const signature = this.data.timeSignatures[index]!
    return Math.round(this.signatureStartTicks[index]! + (safeBar - signature.bar) * ticksPerBar(signature) + Math.max(0, beat) * ticksPerBeat(signature))
  }

  tickToBarBeat(ticks: number): { bar: number; beat: number } {
    const safe = Math.max(0, Math.round(ticks))
    const index = upperBoundNumbers(this.signatureStartTicks, safe) - 1
    const signature = this.data.timeSignatures[index]!
    const local = safe - this.signatureStartTicks[index]!
    const barTicks = ticksPerBar(signature)
    return { bar: signature.bar + Math.floor(local / barTicks), beat: (local % barTicks) / ticksPerBeat(signature) }
  }

  private segmentTicksToSeconds(index: number, deltaTicks: number): number {
    const point = this.data.tempoPoints[index]!
    const next = this.data.tempoPoints[index + 1]
    if (point.curve !== 'linear' || !next || next.tick <= point.tick) return deltaTicks * 60 / (MIDI_PPQ * point.bpm)
    const slope = (next.bpm - point.bpm) / (next.tick - point.tick)
    if (Math.abs(slope) < 1e-12) return deltaTicks * 60 / (MIDI_PPQ * point.bpm)
    return 60 / MIDI_PPQ * Math.log(Math.max(1, point.bpm + slope * deltaTicks) / point.bpm) / slope
  }

  private segmentSecondsToTicks(index: number, seconds: number): number {
    const point = this.data.tempoPoints[index]!
    const next = this.data.tempoPoints[index + 1]
    if (point.curve !== 'linear' || !next || next.tick <= point.tick) return seconds * MIDI_PPQ * point.bpm / 60
    const slope = (next.bpm - point.bpm) / (next.tick - point.tick)
    if (Math.abs(slope) < 1e-12) return seconds * MIDI_PPQ * point.bpm / 60
    return point.bpm * Math.expm1(seconds * MIDI_PPQ * slope / 60) / slope
  }
}

function ticksPerBeat(signature: TimeSignaturePoint): number { return MIDI_PPQ * 4 / signature.denominator }
function ticksPerBar(signature: TimeSignaturePoint): number { return ticksPerBeat(signature) * signature.numerator }
function clipBpm(value: number): number { return Math.max(1, Math.min(999, Number.isFinite(value) ? value : DEFAULT_TEMPO)) }
function clipNumerator(value: number): number { return Math.max(1, Math.min(32, Math.round(Number.isFinite(value) ? value : 4))) }
function clipDenominator(value: number): number { const rounded = Math.round(value); return VALID_DENOMINATORS.has(rounded) ? rounded : 4 }
function upperBound<T>(values: readonly T[], target: number, key: (value: T) => number): number { let low = 0; let high = values.length; while (low < high) { const mid = (low + high) >>> 1; if (key(values[mid]!) <= target) low = mid + 1; else high = mid } return Math.max(1, low) }
function upperBoundNumbers(values: readonly number[], target: number): number { let low = 0; let high = values.length; while (low < high) { const mid = (low + high) >>> 1; if (values[mid]! <= target) low = mid + 1; else high = mid } return Math.max(1, low) }
