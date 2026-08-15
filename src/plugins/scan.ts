import type { IAudioEngine, PluginDescriptor } from '../engine'

let pluginCache: PluginDescriptor[] | null = null
let initialLoad: Promise<PluginDescriptor[]> | null = null
let refresh: Promise<PluginDescriptor[]> | null = null
const detailLoads = new Map<string, Promise<PluginDescriptor>>()
const detailedPlugins = new Map<string, PluginDescriptor>()
const listeners = new Set<(plugins: PluginDescriptor[]) => void>()

/** Return the per-user persistent cache immediately, then refresh changed binaries in the background. */
export function scanPluginsOnce(engine: IAudioEngine): Promise<PluginDescriptor[]> {
  if (pluginCache) return Promise.resolve(pluginCache)
  return initialLoad ??= engine.cachedPlugins().then((cached) => {
    pluginCache = cached
    notify(cached)
    void refreshPlugins(engine, false)
    return cached.length ? cached : refresh!
  }).catch((error) => {
    initialLoad = null
    throw error
  })
}

/** Explicit rescan ignores fingerprints; normal startup only probes new or changed binaries. */
export function rescanPlugins(engine: IAudioEngine): Promise<PluginDescriptor[]> {
  return refreshPlugins(engine, true)
}

export function subscribePluginScan(listener: (plugins: PluginDescriptor[]) => void): () => void {
  listeners.add(listener)
  if (pluginCache) listener(pluginCache)
  return () => listeners.delete(listener)
}

/** Populate the expensive controller/parameter tree only after a plug-in is chosen. */
export function hydratePlugin(engine: IAudioEngine, plugin: PluginDescriptor): Promise<PluginDescriptor> {
  const key = `${plugin.format}:${plugin.uid}:${plugin.path}`
  const detailed = detailedPlugins.get(key)
  if (detailed) return Promise.resolve(detailed)
  if (plugin.parameters.length) {
    detailedPlugins.set(key, plugin)
    return Promise.resolve(plugin)
  }
  const active = detailLoads.get(key)
  if (active) return active
  const task = engine.inspectPlugin(plugin).then((detailed) => {
    detailedPlugins.set(key, detailed)
    pluginCache = (pluginCache ?? []).map((item) => `${item.format}:${item.uid}:${item.path}` === key ? detailed : item)
    notify(pluginCache)
    return detailed
  }).catch(() => plugin).finally(() => detailLoads.delete(key))
  detailLoads.set(key, task)
  return task
}

function refreshPlugins(engine: IAudioEngine, force: boolean): Promise<PluginDescriptor[]> {
  if (refresh) return force ? refresh.then(() => refreshPlugins(engine, true)) : refresh
  if (force) detailedPlugins.clear()
  const task = engine.scanPlugins(force).then((plugins) => {
    pluginCache = plugins.map((plugin) => detailedPlugins.get(`${plugin.format}:${plugin.uid}:${plugin.path}`) ?? plugin)
    notify(pluginCache)
    return pluginCache
  }).finally(() => {
    if (refresh === task) refresh = null
  })
  refresh = task
  return task
}

function notify(plugins: PluginDescriptor[]): void {
  for (const listener of listeners) listener(plugins)
  if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent('ministudio:plugins-refreshed', { detail: plugins }))
}
