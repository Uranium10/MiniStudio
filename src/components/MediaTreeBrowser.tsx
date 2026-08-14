import { ChevronRight, FileAudio, Folder, HardDrive, Plus, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type MouseEvent } from 'react'
import { commands } from '../engine/rust/bindings'
import { useEngine } from '../hooks/useEngine'
import { useProjectStore } from '../store/projectStore'
import { MenuPanel, type MenuItem } from './Menu'
import { writeBrowserDrag } from './browserPayload'

type LocationTab = { id: string; title: string; root: string | null }
type FsNode = { path: string; name: string; directory: boolean }
type Context = { x: number; y: number; path: string | null; title: string }

const STORAGE_KEY = 'minidaw.media-browser.tabs.v2'

export function MediaTreeBrowser({ query, onQueryChange }: { query: string; onQueryChange(value: string): void }) {
  const initialTabs = useMemo(loadTabs, [])
  const [tabs, setTabs] = useState<LocationTab[]>(initialTabs)
  const [activeId, setActiveId] = useState(initialTabs[0]!.id)
  const [roots, setRoots] = useState<string[]>([])
  const [revision, setRevision] = useState(0)
  const [context, setContext] = useState<Context | null>(null)
  const active = tabs.find((tab) => tab.id === activeId) ?? tabs[0]!

  useEffect(() => { localStorage.setItem(STORAGE_KEY, JSON.stringify(tabs)) }, [tabs])
  useEffect(() => { void commands.listStorageRoots().then((items) => setRoots(items.length ? items : fallbackRoots())).catch(() => setRoots(fallbackRoots())) }, [])

  const setRoot = useCallback((path: string | null, title?: string) => {
    setTabs((items) => items.map((tab) => tab.id === active.id ? { ...tab, root: path, title: title ?? (path ? basename(path) : '저장소') } : tab))
    setRevision((value) => value + 1)
  }, [active.id])

  const addTab = (root: string | null = null, title = '저장소') => {
    const tab = { id: crypto.randomUUID(), root, title }
    setTabs((items) => [...items, tab]); setActiveId(tab.id)
  }
  const closeTab = (id: string) => {
    if (tabs.length === 1) { setTabs([{ ...tabs[0]!, root: null, title: '저장소' }]); setRevision((value) => value + 1); return }
    const index = tabs.findIndex((tab) => tab.id === id)
    const next = tabs.filter((tab) => tab.id !== id)
    setTabs(next)
    if (activeId === id) setActiveId(next[Math.max(0, index - 1)]!.id)
  }
  const openContext = (event: MouseEvent, path: string | null, title: string) => { event.preventDefault(); event.stopPropagation(); setContext({ x: event.clientX, y: event.clientY, path, title }) }

  const menuItems = useMemo<MenuItem[]>(() => context ? [
    { kind: 'item', label: '루트로 지정', disabled: !context.path, run: () => setRoot(context.path, context.title) },
    { kind: 'item', label: '새 탭에서 열기', disabled: !context.path, run: () => addTab(context.path, context.title) },
    { kind: 'item', label: '경로 복사', disabled: !context.path, run: () => { if (context.path) void navigator.clipboard.writeText(context.path) } },
    { kind: 'separator' },
    { kind: 'item', label: '새로 고침', run: () => setRevision((value) => value + 1) },
    { kind: 'item', label: '저장소 보기', disabled: !active.root, run: () => setRoot(null, '저장소') },
  ] : [], [active.root, context, setRoot])

  return <div className="media-tree-shell">
    <div className="media-location-tabs">
      {tabs.map((tab) => <button key={tab.id} className={tab.id === active.id ? 'active' : ''} onClick={() => setActiveId(tab.id)} title={tab.root ?? '저장소'}><span>{tab.title}</span><i onClick={(event) => { event.stopPropagation(); closeTab(tab.id) }}><X size={10} /></i></button>)}
      <button className="add" title="위치 탭 추가" onClick={() => addTab()}><Plus size={12} /></button>
    </div>
    <label className="browser-search"><Search size={13} /><input value={query} onChange={(event) => onQueryChange(event.target.value)} placeholder="현재 트리 검색" /></label>
    <div className="media-tree" onContextMenu={(event) => openContext(event, active.root, active.title)}>
      {active.root
        ? <FolderNode key={`${active.id}:${active.root}:${revision}`} node={{ path: active.root, name: active.title, directory: true }} depth={0} query={query} initiallyOpen onContext={openContext} />
        : roots.map((path) => <FolderNode key={`${path}:${revision}`} node={{ path, name: path, directory: true }} depth={0} query={query} drive onContext={openContext} />)}
      {!roots.length && !active.root && <div className="browser-empty">저장소를 찾는 중…</div>}
    </div>
    {context && <MenuPanel anchor={context} items={menuItems} onClose={() => setContext(null)} className="context-menu" />}
  </div>
}

function FolderNode({ node, depth, query, initiallyOpen = false, drive = false, onContext }: { node: FsNode; depth: number; query: string; initiallyOpen?: boolean; drive?: boolean; onContext(event: MouseEvent, path: string | null, title: string): void }) {
  const engine = useEngine()
  const [open, setOpen] = useState(initiallyOpen)
  const [children, setChildren] = useState<FsNode[] | null>(null)
  const [error, setError] = useState(false)
  useEffect(() => {
    if (!open || children) return
    let cancelled = false
    void commands.listMediaDirectory(node.path).then((response) => {
      if (response.status === 'error') throw response.error
      if (!cancelled) setChildren(response.data.map((entry) => ({ path: entry.path, name: entry.name, directory: entry.isDirectory })))
    }).catch(() => { if (!cancelled) setError(true) })
    return () => { cancelled = true }
  }, [children, node.path, open])

  const normalized = query.trim().toLocaleLowerCase()
  const visible = children?.filter((child) => child.directory || !normalized || child.name.toLocaleLowerCase().includes(normalized)) ?? []
  return <div className="media-tree-node">
    <button className="media-folder-row" style={{ paddingLeft: 7 + depth * 13 }} onClick={() => setOpen((value) => !value)} onContextMenu={(event) => onContext(event, node.path, node.name)}>
      <ChevronRight size={11} className={open ? 'open' : ''} />{drive ? <HardDrive size={13} /> : <Folder size={13} />}<span>{node.name}</span>
    </button>
    {open && <div>{children === null && !error && <div className="media-tree-status" style={{ paddingLeft: 24 + depth * 13 }}>읽는 중…</div>}{error && <div className="media-tree-status error" style={{ paddingLeft: 24 + depth * 13 }}>열 수 없음</div>}
      {visible.map((child) => child.directory
        ? <FolderNode key={child.path} node={child} depth={depth + 1} query={query} onContext={onContext} />
        : <button key={child.path} className="media-file-row" style={{ paddingLeft: 24 + depth * 13 }} draggable onDragStart={(event) => writeBrowserDrag(event, { kind: 'media', path: child.path, name: child.name })} onDoubleClick={() => { void engine.loadAudioFile(child.path).then((asset) => useProjectStore.getState().addAssetAsTrack(asset)).catch((reason) => useProjectStore.getState().showToast(`오디오를 불러오지 못했습니다: ${String(reason)}`)) }}><FileAudio size={12} /><span>{child.name}</span></button>)}
    </div>}
  </div>
}

function loadTabs(): LocationTab[] {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]') as LocationTab[]
    if (Array.isArray(value) && value.length) return value.filter((tab) => typeof tab.id === 'string' && (tab.root === null || typeof tab.root === 'string'))
  } catch { /* reset malformed persisted state */ }
  return [{ id: crypto.randomUUID(), title: '저장소', root: null }]
}
function fallbackRoots(): string[] { return navigator.userAgent.includes('Windows') ? ['C:\\', 'D:\\'] : ['/'] }
function basename(path: string): string { return path.replace(/[\\/]+$/, '').split(/[\\/]/).at(-1) || path }
