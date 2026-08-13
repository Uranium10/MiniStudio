import type { IAudioEngine, PluginDescriptor } from '../engine'

let pluginScanCache: Promise<PluginDescriptor[]> | null = null

/** One crash-isolated plug-in scan shared by every picker and the F5 browser. */
export function scanPluginsOnce(engine: IAudioEngine): Promise<PluginDescriptor[]> {
  return pluginScanCache ??= engine.scanPlugins().catch((error) => {
    pluginScanCache = null
    throw error
  })
}

export function rescanPlugins(engine: IAudioEngine): Promise<PluginDescriptor[]> {
  pluginScanCache = null
  return scanPluginsOnce(engine)
}
