// Single engine implementation-selection point for future native-engine swaps.
import type { IAudioEngine } from './IAudioEngine'
import { RustEngine } from './rust/RustEngine'

export type { IAudioEngine } from './IAudioEngine'
export * from './types'
export * from './tempoMap'
export { describeEngineError } from './rust/RustEngine'

export function createEngine(): IAudioEngine {
  return new RustEngine()
}
