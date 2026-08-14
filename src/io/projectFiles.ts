// Tauri-backed project and audio file dialogs with typed JSON conversion.
import { open, save } from '@tauri-apps/plugin-dialog'
import { exists, readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import type { IAudioEngine, ProjectState } from '../engine'

type StoredProject = Omit<ProjectState, 'assets'> & {
  assets: Record<string, Omit<ProjectState['assets'][string], 'peaks'> & { peaks: number[] }>
}

function toStoredProject(project: ProjectState): StoredProject {
  return {
    ...project,
    assets: Object.fromEntries(Object.entries(project.assets).map(([id, asset]) => [id, { ...asset, peaks: Array.from(asset.peaks) }])),
  }
}

function fromStoredProject(stored: StoredProject): ProjectState {
  const project = {
    ...stored,
    assets: Object.fromEntries(Object.entries(stored.assets).map(([id, asset]) => [id, { ...asset, peaks: new Float32Array(asset.peaks) }])),
  } as ProjectState
  return migrateProject(project)
}

export function migrateProject(source: ProjectState | (Omit<ProjectState, 'formatVersion'> & { formatVersion?: number })): ProjectState {
  const project = source as ProjectState
  project.formatVersion = 2
  project.transport = { ...project.transport, timeSignature: project.transport.timeSignature ?? { numerator: 4, denominator: 4 } }
  project.tracks = project.tracks.map((track) => ({
    ...track,
    kind: track.kind ?? 'audio',
    clips: track.clips ?? [],
    midiClips: track.midiClips ?? [],
    instrument: track.instrument ?? null,
  }))
  // Mute/dim arrived after format 2 shipped, so older files default to unmuted.
  project.buses = project.buses.map((bus) => ({ ...bus, muted: bus.muted ?? false }))
  project.master = { ...project.master, muted: project.master.muted ?? false, dim: project.master.dim ?? false }
  return project
}

export async function saveProject(project: ProjectState): Promise<string | null> {
  const path = await save({ defaultPath: `${project.meta.name}.json`, filters: [{ name: 'MiniDAW Project', extensions: ['json'] }] })
  if (!path) return null
  await writeTextFile(path, JSON.stringify(toStoredProject(project), null, 2))
  return path
}

export async function openProject(): Promise<{ path: string; project: ProjectState } | null> {
  const path = await open({ multiple: false, directory: false, filters: [{ name: 'MiniDAW Project', extensions: ['json'] }] })
  if (!path) return null
  const value: unknown = JSON.parse(await readTextFile(path))
  if (!isStoredProject(value)) throw new Error('올바른 MiniDAW 프로젝트가 아닙니다.')
  return { path, project: fromStoredProject(value) }
}

export async function chooseAudioFile(): Promise<string | null> {
  const path = await open({ multiple: false, directory: false, filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'flac', 'ogg', 'm4a'] }] })
  return path
}

export async function chooseExportPath(projectName: string, format: 'wav' | 'mp3' = 'wav'): Promise<string | null> {
  return save({ defaultPath: `${projectName}.${format}`, filters: [{ name: format === 'mp3' ? 'MP3 Audio' : 'Wave Audio', extensions: [format] }] })
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
  for (const track of project.tracks) {
    for (const clip of track.clips) clip.assetId = replacementIds.get(clip.assetId) ?? clip.assetId
  }
  project.assets = hydratedAssets
  return { project, missing }
}

function isStoredProject(value: unknown): value is StoredProject {
  if (!value || typeof value !== 'object') return false
  const candidate = value as Record<string, unknown>
  return typeof candidate.meta === 'object' && typeof candidate.transport === 'object' && Array.isArray(candidate.tracks) && typeof candidate.assets === 'object'
}
