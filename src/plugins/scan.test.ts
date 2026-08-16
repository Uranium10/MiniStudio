import { describe, expect, it, vi } from 'vitest'
import type { IAudioEngine, PluginDescriptor } from '../engine'
import { hydratePluginRef } from './scan'

const detailed: PluginDescriptor = {
  format: 'vst3', uid: 'legacy.plugin', name: 'Legacy', vendor: 'Vendor', category: 'Instrument', path: 'legacy.vst3', isInstrument: true,
  paramCount: 1, hasEditor: true, audioInputBuses: 0, audioOutputBuses: 1, supportsSidechain: false,
  parameters: [{ id: '12', name: 'Macro', module: 'Main', min: 0, max: 1, defaultValue: 0.5 }],
}

describe('plug-in reference hydration', () => {
  it('inspects portable references whose old project metadata omitted paramCount', async () => {
    const inspectPlugin = vi.fn().mockResolvedValue(detailed)
    const engine = { inspectPlugin } as unknown as IAudioEngine
    const hydrated = await hydratePluginRef(engine, { format: 'vst3', uid: 'legacy.plugin', name: 'Legacy', vendor: 'Vendor', path: 'legacy.vst3' }, true)

    expect(inspectPlugin).toHaveBeenCalledOnce()
    expect(hydrated.parameters?.[0]?.name).toBe('Macro')
  })

  it('does not re-inspect a confirmed zero-parameter plug-in', async () => {
    const inspectPlugin = vi.fn()
    const engine = { inspectPlugin } as unknown as IAudioEngine
    await hydratePluginRef(engine, { format: 'vst3', uid: 'empty.plugin', name: 'Empty', vendor: 'Vendor', path: 'empty.vst3', paramCount: 0, parameters: [] }, false)
    expect(inspectPlugin).not.toHaveBeenCalled()
  })
})
