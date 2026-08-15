// Single Zustand tool-state machine with smart-tool latching semantics.
import { create } from 'zustand'

export type ToolId = 'arrow' | 'range' | 'split' | 'erase' | 'paint' | 'mute' | 'listen'
export type SubToolId = Exclude<ToolId, 'arrow' | 'listen'> | 'none'

export type ToolState = {
  activeTool: ToolId
  /** Piano-roll tools are deliberately independent from the arrangement palette. */
  pianoTool: ToolId
  subTool: SubToolId
  isModifierHeld: boolean
  latchedTool: ToolId | null
}

type ToolActions = {
  chooseTool(tool: ToolId): void
  choosePianoTool(tool: ToolId): void
  pressNumber(number: number): void
  pressPianoNumber(number: number): void
  setModifierHeld(held: boolean): void
  latchGesture(): ToolId
  releaseGesture(): void
  resetModifiers(): void
}

export const subToolCycle: readonly SubToolId[] = ['none', 'range', 'split', 'erase', 'paint', 'mute']

export function getEffectiveTool(state: Pick<ToolState, 'activeTool' | 'subTool' | 'isModifierHeld' | 'latchedTool'>): ToolId {
  if (state.latchedTool) return state.latchedTool
  if (state.activeTool === 'arrow' && state.subTool !== 'none' && state.isModifierHeld) return state.subTool
  return state.activeTool
}

export function nextSubTool(current: SubToolId): SubToolId {
  const index = subToolCycle.indexOf(current)
  return subToolCycle[(index + 1) % subToolCycle.length] ?? 'none'
}

const numberTools: readonly ToolId[] = ['arrow', 'range', 'split', 'erase', 'paint', 'mute', 'listen']

export const useToolStore = create<ToolState & ToolActions>((set, get) => ({
  activeTool: 'arrow',
  pianoTool: 'arrow',
  subTool: 'range',
  isModifierHeld: false,
  latchedTool: null,
  chooseTool: (tool) => set({ activeTool: tool }),
  choosePianoTool: (tool) => set({ pianoTool: tool }),
  pressNumber: (number) => {
    const tool = numberTools[number - 1]
    if (!tool) return
    const state = get()
    if (tool === 'arrow' && state.activeTool === 'arrow') set({ subTool: nextSubTool(state.subTool) })
    else set({ activeTool: tool })
  },
  // Unlike the arrangement smart tool, the piano-roll palette has no hidden
  // sub-tool cycle. Digits map one-to-one to the seven visible buttons.
  pressPianoNumber: (number) => {
    const tool = numberTools[number - 1]
    if (tool) set({ pianoTool: tool })
  },
  setModifierHeld: (held) => set({ isModifierHeld: held }),
  latchGesture: () => {
    const tool = getEffectiveTool(get())
    set({ latchedTool: tool })
    return tool
  },
  releaseGesture: () => set({ latchedTool: null }),
  resetModifiers: () => set({ isModifierHeld: false, latchedTool: null }),
}))
