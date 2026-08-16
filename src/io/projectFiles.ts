// Tauri-backed project and audio file dialogs with typed JSON conversion.
import { open, save } from '@tauri-apps/plugin-dialog'
import { exists, readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import { normalizeTempoMap, type AudioSourceRef, type IAudioEngine, type ProjectState } from '../engine'

export const CURRENT_PROJECT_FORMAT = 3 as const

type StoredProject = Omit<ProjectState, 'assets'> & {
  assets: Record<string, Omit<ProjectState['assets'][string], 'peaks'> & { peaks: number[] }>
}

export function toStoredProject(project: ProjectState): StoredProject {
  const portable = structuredClone(project)
  // Installation locations belong to the per-user scan cache, never to a portable project.
  for (const track of portable.tracks) {
    if (track.instrument?.plugin) track.instrument.plugin.path = ''
    for (const effect of track.effects) if (effect.plugin) effect.plugin.path = ''
  }
  for (const bus of portable.buses) for (const effect of bus.effects) if (effect.plugin) effect.plugin.path = ''
  for (const effect of portable.master.effects) if (effect.plugin) effect.plugin.path = ''
  return {
    ...portable,
    assets: Object.fromEntries(Object.entries(portable.assets).map(([id, asset]) => [id, { ...asset, peaks: Array.from(asset.peaks) }])),
  }
}

export type DevicePreset = {
  format: 'ministudio-device-preset'
  version: 1
  name: string
  deviceType: string
  pluginUid?: string
  params: Record<string, number>
  state?: number[]
}

export function fromStoredProject(stored: StoredProject): ProjectState {
  const project = {
    ...stored,
    assets: Object.fromEntries(Object.entries(stored.assets).map(([id, asset]) => [id, { ...asset, peaks: new Float32Array(asset.peaks) }])),
  } as ProjectState
  return migrateProject(project)
}

export function serializeProject(project: ProjectState): string {
  return JSON.stringify(toStoredProject(project))
}

export function deserializeProject(json: string): ProjectState {
  const value: unknown = JSON.parse(json)
  if (!isStoredProject(value)) throw new Error('올바른 MiniStudio 프로젝트가 아닙니다.')
  return fromStoredProject(value as StoredProject)
}

export function migrateProject(source: unknown): ProjectState {
  if (!source || typeof source !== 'object') throw new Error('올바른 MiniStudio 프로젝트가 아닙니다.')
  const candidate = structuredClone(source) as Record<string, unknown>
  const version = typeof candidate.formatVersion === 'number' ? candidate.formatVersion : 1
  if (version > CURRENT_PROJECT_FORMAT) throw new Error(`이 프로젝트는 더 새로운 MiniStudio 형식(v${version})입니다. 이 앱이 지원하는 최신 형식은 v${CURRENT_PROJECT_FORMAT}입니다.`)
  if (version < 1) throw new Error(`지원하지 않는 MiniStudio 프로젝트 형식(v${version})입니다.`)

  // v1 -> v2: track kinds, MIDI containers and strip mute state became explicit.
  if (version < 2) candidate.formatVersion = 2
  const v2 = candidate as unknown as ProjectState
  v2.transport = { ...v2.transport, timeSignature: v2.transport?.timeSignature ?? { numerator: 4, denominator: 4 } }
  v2.tracks = (v2.tracks ?? []).map((track) => ({ ...track, kind: track.kind ?? 'audio', clips: track.clips ?? [], midiClips: track.midiClips ?? [], instrument: track.instrument ?? null }))
  v2.buses = (v2.buses ?? []).map((bus) => ({ ...bus, muted: bus.muted ?? false }))
  v2.master = { ...v2.master, muted: v2.master?.muted ?? false, dim: v2.master?.dim ?? false }

  // v2 -> v3: add the variable musical-time map and the ARA-ready source identity layer.
  const legacyTransport = v2.transport
  legacyTransport.tempoMap = normalizeTempoMap(legacyTransport.tempoMap, legacyTransport.bpm, legacyTransport.timeSignature.numerator, legacyTransport.timeSignature.denominator)
  legacyTransport.bpm = legacyTransport.tempoMap.tempoPoints[0]!.bpm
  legacyTransport.timeSignature = { numerator: legacyTransport.tempoMap.timeSignatures[0]!.numerator, denominator: legacyTransport.tempoMap.timeSignatures[0]!.denominator }
  const refs: Record<string, AudioSourceRef> = { ...(v2.audioSourceRefs ?? {}) }
  const refByAsset = new Map(Object.values(refs).map((ref) => [ref.assetId, ref.id]))
  for (const track of v2.tracks) {
    track.clips = track.clips.map((clip) => {
      const legacy = clip as typeof clip & { assetId?: string; audioSourceRefId?: string }
      if (legacy.audioSourceRefId && refs[legacy.audioSourceRefId]) {
        const { assetId: _discarded, ...current } = legacy
        return current
      }
      const assetId = legacy.assetId ?? ''
      let refId = refByAsset.get(assetId)
      if (!refId) {
        refId = uniqueSourceRefId(refs, assetId)
        refs[refId] = { id: refId, assetId, name: v2.assets?.[assetId]?.name ?? legacy.name ?? 'Audio source', modificationId: null }
        refByAsset.set(assetId, refId)
      }
      const { assetId: _discarded, ...current } = legacy
      return { ...current, audioSourceRefId: refId }
    })
  }
  v2.audioSourceRefs = refs
  v2.formatVersion = CURRENT_PROJECT_FORMAT
  return v2
}

export async function saveProject(project: ProjectState): Promise<string | null> {
  const path = await save({ defaultPath: `${project.meta.name}.json`, filters: [{ name: 'MiniStudio Project', extensions: ['json'] }] })
  if (!path) return null
  await writeTextFile(path, JSON.stringify(toStoredProject(project), null, 2))
  return path
}

export async function openProject(): Promise<{ path: string; project: ProjectState } | null> {
  const path = await open({ multiple: false, directory: false, filters: [{ name: 'MiniStudio Project', extensions: ['json'] }] })
  if (!path) return null
  const value: unknown = JSON.parse(await readTextFile(path))
  if (!isStoredProject(value)) throw new Error('올바른 MiniStudio 프로젝트가 아닙니다.')
  return { path, project: fromStoredProject(value as StoredProject) }
}

export async function chooseAudioFile(): Promise<string | null> {
  const path = await open({ multiple: false, directory: false, filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'flac', 'ogg', 'm4a'] }] })
  return path
}

export async function chooseExportPath(projectName: string, format: 'wav' | 'mp3' = 'wav'): Promise<string | null> {
  return save({ defaultPath: `${projectName}.${format}`, filters: [{ name: format === 'mp3' ? 'MP3 Audio' : 'Wave Audio', extensions: [format] }] })
}

export async function saveDevicePreset(preset: DevicePreset): Promise<string | null> {
  const safeName = preset.name.replace(/[<>:"/\\|?*]+/g, '_') || 'Device Preset'
  const path = await save({ defaultPath: `${safeName}.mspreset`, filters: [{ name: 'MiniStudio Device Preset', extensions: ['mspreset'] }] })
  if (!path) return null
  await writeTextFile(path, JSON.stringify(preset, null, 2))
  return path
}

export async function openDevicePreset(): Promise<DevicePreset | null> {
  const path = await open({ multiple: false, directory: false, filters: [{ name: 'MiniStudio Device Preset', extensions: ['mspreset'] }] })
  if (!path) return null
  const value: unknown = JSON.parse(await readTextFile(path))
  if (!value || typeof value !== 'object') throw new Error('올바른 MiniStudio 디바이스 프리셋이 아닙니다.')
  const preset = value as Partial<DevicePreset>
  if (preset.format !== 'ministudio-device-preset' || preset.version !== 1 || typeof preset.deviceType !== 'string' || !preset.params || typeof preset.params !== 'object') throw new Error('지원하지 않는 디바이스 프리셋입니다.')
  return preset as DevicePreset
}

export async function hydrateProjectAudio(engine: IAudioEngine, source: ProjectState): Promise<{ project: ProjectState; missing: Array<{ id: string; name: string }> }> {
  const project = structuredClone(source)
  const replacementIds = new Map<string, string>()
  const hydratedAssets: ProjectState['assets'] = {}
  const missing: Array<{ id: string; name: string }> = []
  for (const [oldId, asset] of Object.entries(project.assets)) {
    if (!asset.path || !(await exists(asset.path))) {
      missing.push({ id: oldId, name: asset.name })
      hydratedAssets[oldId] = asset
      continue
    }
    try {
      const loaded = await engine.loadAudioFile(asset.path)
      replacementIds.set(oldId, loaded.id)
      hydratedAssets[loaded.id] = loaded
    } catch {
      missing.push({ id: oldId, name: asset.name })
      hydratedAssets[oldId] = asset
    }
  }
  for (const ref of Object.values(project.audioSourceRefs)) ref.assetId = replacementIds.get(ref.assetId) ?? ref.assetId
  project.assets = hydratedAssets
  return { project, missing }
}

function isStoredProject(value: unknown): value is StoredProject {
  if (!value || typeof value !== 'object') return false
  const candidate = value as Record<string, unknown>
  return typeof candidate.meta === 'object' && typeof candidate.transport === 'object' && Array.isArray(candidate.tracks) && typeof candidate.assets === 'object'
}

function uniqueSourceRefId(refs: Record<string, AudioSourceRef>, assetId: string): string {
  const base = `source:${assetId || 'silent'}`
  if (!refs[base]) return base
  let suffix = 2
  while (refs[`${base}:${suffix}`]) suffix += 1
  return `${base}:${suffix}`
}
