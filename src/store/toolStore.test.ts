// Smart-tool state-machine regression tests.
import { beforeEach, describe, expect, it } from 'vitest'
import { getEffectiveTool, nextSubTool, useToolStore } from './toolStore'

describe('smart tool state', () => {
  beforeEach(() => useToolStore.setState({ activeTool: 'arrow', subTool: 'none', isModifierHeld: false, latchedTool: null }))

  it('cycles the exact sub-tool order', () => {
    expect(nextSubTool('none')).toBe('range')
    expect(nextSubTool('range')).toBe('split')
    expect(nextSubTool('split')).toBe('erase')
    expect(nextSubTool('erase')).toBe('paint')
    expect(nextSubTool('paint')).toBe('mute')
    expect(nextSubTool('mute')).toBe('none')
  })

  it('first returns to arrow and only a repeated 1 cycles', () => {
    const store = useToolStore.getState()
    store.chooseTool('split')
    useToolStore.getState().pressNumber(1)
    expect(useToolStore.getState().activeTool).toBe('arrow')
    expect(useToolStore.getState().subTool).toBe('none')
    useToolStore.getState().pressNumber(1)
    expect(useToolStore.getState().subTool).toBe('range')
  })

  it('uses the sub-tool only while modifier is held', () => {
    const state = { activeTool: 'arrow' as const, subTool: 'split' as const, isModifierHeld: true, latchedTool: null }
    expect(getEffectiveTool(state)).toBe('split')
    expect(getEffectiveTool({ ...state, isModifierHeld: false })).toBe('arrow')
  })

  it('latches a gesture across modifier changes', () => {
    useToolStore.setState({ subTool: 'erase', isModifierHeld: true })
    expect(useToolStore.getState().latchGesture()).toBe('erase')
    useToolStore.getState().setModifierHeld(false)
    expect(getEffectiveTool(useToolStore.getState())).toBe('erase')
    useToolStore.getState().releaseGesture()
    expect(getEffectiveTool(useToolStore.getState())).toBe('arrow')
  })

  it('clears stuck modifiers and latches on blur reset', () => {
    useToolStore.setState({ isModifierHeld: true, latchedTool: 'paint' })
    useToolStore.getState().resetModifiers()
    expect(useToolStore.getState()).toMatchObject({ isModifierHeld: false, latchedTool: null })
  })
})
