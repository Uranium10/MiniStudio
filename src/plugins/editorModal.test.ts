import { describe, expect, it, vi } from 'vitest'
import type { IAudioEngine } from '../engine'
import { withPluginEditorModal } from './editorModal'

describe('withPluginEditorModal', () => {
  it('acknowledges modal ownership before work and restores it afterward', async () => {
    const order: string[] = []
    const setPluginEditorModal = vi.fn(async (_targetId: string, modal: boolean) => { order.push(modal ? 'modal-on' : 'modal-off') })
    const engine = { setPluginEditorModal } as unknown as IAudioEngine

    const result = await withPluginEditorModal(engine, 'serum', async () => { order.push('dialog'); return 'saved' })

    expect(result).toBe('saved')
    expect(order).toEqual(['modal-on', 'dialog', 'modal-off'])
  })

  it('restores the editor when a dialog is cancelled by an exception', async () => {
    const setPluginEditorModal = vi.fn().mockResolvedValue(undefined)
    const engine = { setPluginEditorModal } as unknown as IAudioEngine

    await expect(withPluginEditorModal(engine, 'serum', async () => { throw new Error('cancelled') })).rejects.toThrow('cancelled')
    expect(setPluginEditorModal.mock.calls).toEqual([['serum', true], ['serum', false]])
  })
})
