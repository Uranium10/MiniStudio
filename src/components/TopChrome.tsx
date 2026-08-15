// Studio One-inspired menu and precision tool chrome.
import {
  Crosshair, Download, Ear, Eraser, FolderOpen, Magnet, MousePointer2, PanelRightOpen, Pencil, Redo2,
  Plus, Save, ScanLine, Scissors, Settings, Undo2, VolumeX, ZoomIn, ZoomOut,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useEffect, useState } from 'react'
import { GRID_OPTIONS, type StreamStatus } from '../engine'
import { useEngine } from '../hooks/useEngine'
import {
  deleteInContext, importAudio, openProjectFile, saveProjectFile,
  selectAllInContext, splitSelectionAtPlayhead,
} from '../store/commands'
import { useProjectStore } from '../store/projectStore'
import { getEffectiveTool, type ToolId, useToolStore } from '../store/toolStore'
import { MenuPanel, type MenuItem } from './Menu'
import { TrackAddDialog } from './TrackAddDialog'
import { EditableNumber } from './controls'

const OPEN_TRACK_ADD_DIALOG_EVENT = 'ministudio:open-track-add-dialog'

function requestTrackAddDialog(): void {
  window.dispatchEvent(new Event(OPEN_TRACK_ADD_DIALOG_EVENT))
}

export function MenuBar() {
  const projectName = useProjectStore((state) => state.project.meta.name)
  const [open, setOpen] = useState<string | null>(null)
  const engine = useEngine()
  const menus = useMenuDefinitions()

  return (
    <div className="menu-bar">
      <div className="brand"><span className="brand-mark">M</span><strong>MiniStudio</strong></div>
      <nav aria-label="Application menu">
        {menus.map((menu) => (
          <div className="menu-root" key={menu.title}>
            <button
              className={open === menu.title ? 'open' : ''}
              aria-haspopup="menu"
              aria-expanded={open === menu.title}
              onClick={() => setOpen(open === menu.title ? null : menu.title)}
              // Hovering a sibling while a menu is open switches menus, as in native menu bars.
              onPointerEnter={() => setOpen((current) => (current ? menu.title : current))}
            >{menu.title}</button>
            {open === menu.title && <MenuPanel items={menu.items} onClose={() => setOpen(null)} />}
          </div>
        ))}
      </nav>
      <div className="project-title"><span className="saved-dot" />{projectName}<small>{useProjectStore.getState().project.meta.sampleRate / 1000} kHz</small></div>
      <EngineStatusBadge engine={engine} />
    </div>
  )
}

function EngineStatusBadge({ engine }: { engine: ReturnType<typeof useEngine> }) {
  const [status, setStatus] = useState<StreamStatus>(() => engine.getStreamStatus())
  const openSettings = useProjectStore((state) => state.setAudioSettingsOpen)
  useEffect(() => {
    const timer = window.setInterval(() => setStatus({ ...engine.getStreamStatus() }), 500)
    return () => window.clearInterval(timer)
  }, [engine])
  const tone = status.error ? 'error' : status.running ? 'online' : 'idle'
  const label = status.error ? 'ERROR' : status.running ? 'ONLINE' : 'IDLE'
  return (
    <button className="window-status" onClick={() => openSettings(true)} title={status.error ?? `${status.latencyMs.toFixed(2)} ms · ${status.xruns} xruns · 클릭하여 오디오 설정`}>
      Audio Engine <span className={`status-${tone}`}>{label}</span>
    </button>
  )
}

function useMenuDefinitions(): Array<{ title: string; items: MenuItem[] }> {
  const engine = useEngine()
  const store = useProjectStore()
  const canUndo = store.past.length > 0
  const canRedo = store.future.length > 0
  const hasClipSelection = store.selectedClipIds.length > 0
  const item = (label: string, run: () => void, extra?: Partial<Extract<MenuItem, { kind: 'item' }>>): MenuItem => ({ kind: 'item', label, run, ...extra })
  const separator: MenuItem = { kind: 'separator' }
  const recentHistory: MenuItem[] = store.history.slice(-8).reverse().map((entry) => item(`${formatHistoryTime(entry.timestamp)}  ${entry.label}`, () => undefined, { disabled: true }))

  return [
    {
      title: '파일',
      items: [
        item('새 프로젝트', () => {
          if (!window.confirm('현재 프로젝트를 닫고 새 프로젝트를 시작할까요? 저장하지 않은 변경은 사라집니다.')) return
          void engine.stop()
          store.newProject()
        }, { keys: 'Ctrl+N' }),
        separator,
        item('프로젝트 열기…', () => void openProjectFile(engine), { keys: 'Ctrl+O' }),
        item('프로젝트 저장', () => void saveProjectFile(engine), { keys: 'Ctrl+S' }),
        separator,
        item('오디오 가져오기…', () => void importAudio(engine)),
        item('내보내기…', () => store.setExportDialogOpen(true), { keys: 'Ctrl+Shift+E' }),
        separator,
        item('오디오 · MIDI 설정…', () => store.setAudioSettingsOpen(true)),
      ],
    },
    {
      title: '편집',
      items: [
        item('실행취소', store.undo, { keys: 'Ctrl+Z', disabled: !canUndo }),
        item('다시 실행', store.redo, { keys: 'Ctrl+Shift+Z', disabled: !canRedo }),
        separator,
        item('복사', store.copySelectedClips, { keys: 'Ctrl+C', disabled: !hasClipSelection }),
        item('잘라내기', store.cutSelectedClips, { keys: 'Ctrl+X', disabled: !hasClipSelection }),
        item('붙여넣기', () => store.pasteClipboard(), { keys: 'Ctrl+V', disabled: !store.clipboard }),
        item('제자리 복제', store.duplicateSelectedClips, { keys: 'Ctrl+D', disabled: !hasClipSelection }),
        separator,
        item('전체 선택', selectAllInContext, { keys: 'Ctrl+A' }),
        item('삭제', deleteInContext, { keys: 'Delete', danger: true }),
        separator,
        { kind: 'label', label: '히스토리' },
        ...(recentHistory.length ? recentHistory : [item('기록된 편집 없음', () => undefined, { disabled: true })]),
      ],
    },
    {
      title: '트랙',
      items: [
        item('트랙 추가…', requestTrackAddDialog),
        separator,
        item('선택 트랙 복제', () => store.selectedTrackId && store.duplicateTrack(store.selectedTrackId), { disabled: !store.selectedTrackId }),
        item('선택 트랙 삭제', store.removeSelectedTrack, { disabled: !store.selectedTrackId, danger: true }),
      ],
    },
    {
      title: '클립',
      items: [
        item('플레이헤드에서 분할', splitSelectionAtPlayhead, { keys: 'S', disabled: !hasClipSelection }),
        item('뮤트 전환', store.toggleSelectedClipsMuted, { keys: 'M', disabled: !hasClipSelection }),
        separator,
        item('선택 영역을 루프로', store.setLoopToSelection, { keys: 'Shift+L', disabled: !hasClipSelection }),
        item('루프 켜기 / 끄기', store.toggleLoop, { keys: 'L', checked: store.project.transport.loop.enabled }),
        separator,
        item('선택 클립 삭제', store.deleteSelectedClips, { keys: 'Delete', disabled: !hasClipSelection, danger: true }),
      ],
    },
    {
      title: '보기',
      items: [
        item('믹서', () => store.setLowerTab('mixer'), { checked: store.lowerTab === 'mixer' }),
        item('이펙트 체인', () => store.setLowerTab('effects'), { checked: store.lowerTab === 'effects' }),
        item('피아노롤', store.togglePianoRoll, { keys: 'F2', checked: store.pianoRollOpen }),
        separator,
        item('하단 인터페이스 접기 / 펴기', store.toggleLowerPanel, { keys: 'F3', checked: !store.lowerPanelCollapsed }),
        item('좌측 인스펙터 접기 / 펴기', store.toggleInspector, { keys: 'F4', checked: store.inspectorVisible }),
        item('미디어 브라우저 접기 / 펴기', store.toggleBrowser, { keys: 'F5', checked: store.browserVisible }),
        item('에디터 최대화', store.toggleEditorMaximized, { checked: store.editorMaximized }),
        item('오토 스크롤', store.toggleFollowPlayhead, { keys: 'Shift+F', checked: store.followPlayhead }),
        separator,
        item('가로 확대', () => store.setZoom(store.pixelsPerSecond * 1.25), { keys: '+' }),
        item('가로 축소', () => store.setZoom(store.pixelsPerSecond / 1.25), { keys: '-' }),
        item('전체 보기', () => store.setZoom(22), { keys: 'F' }),
      ],
    },
    {
      title: '도움말',
      items: [
        item('키보드 단축키…', () => store.setShortcutsOpen(true), { keys: 'F1' }),
        separator,
        { kind: 'label', label: 'MiniStudio · Rust 네이티브 엔진' },
        item('VST3 / CLAP 호스팅 상태', () => store.showToast(engine.capabilities().supportsExternalPlugins ? 'VST3 / CLAP 호스팅이 활성화되어 있습니다' : '외부 플러그인 호스팅을 사용할 수 없습니다')),
      ],
    },
  ]
}

function formatHistoryTime(timestamp: number): string {
  return new Intl.DateTimeFormat('ko-KR', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }).format(new Date(timestamp))
}

const tools: Array<{ id: ToolId; label: string; icon: LucideIcon }> = [
  { id: 'arrow', label: '선택', icon: MousePointer2 },
  { id: 'range', label: '범위', icon: ScanLine },
  { id: 'split', label: '스플릿', icon: Scissors },
  { id: 'erase', label: '지우개', icon: Eraser },
  { id: 'paint', label: '그리기', icon: Pencil },
  { id: 'mute', label: '클립 뮤트', icon: VolumeX },
  { id: 'listen', label: '오디션', icon: Ear },
]

export function ToolBar() {
  const [trackAddOpen, setTrackAddOpen] = useState(false)
  const toolState = useToolStore()
  const effective = getEffectiveTool(toolState)
  const editFocus = useProjectStore((state) => state.editFocus)
  const arrangementSnap = useProjectStore((state) => state.snapEnabled)
  const pianoSnap = useProjectStore((state) => state.pianoSnapEnabled)
  const arrangementGrid = useProjectStore((state) => state.gridTicks)
  const pianoGrid = useProjectStore((state) => state.pianoGridTicks)
  const arrangementSwing = useProjectStore((state) => state.arrangementSwing)
  const pianoSwing = useProjectStore((state) => state.pianoSwing)
  const pianoContext = editFocus === 'pianoRoll'
  const snap = pianoContext ? pianoSnap : arrangementSnap
  const gridTicks = pianoContext ? pianoGrid : arrangementGrid
  const swing = pianoContext ? pianoSwing : arrangementSwing
  const follow = useProjectStore((state) => state.followPlayhead)
  const toggleFollow = useProjectStore((state) => state.toggleFollowPlayhead)
  const zoom = useProjectStore((state) => state.pixelsPerSecond)
  const canUndo = useProjectStore((state) => state.past.length > 0)
  const canRedo = useProjectStore((state) => state.future.length > 0)
  const browserVisible = useProjectStore((state) => state.browserVisible)
  const store = useProjectStore.getState()
  const engine = useEngine()

  useEffect(() => {
    const openDialog = () => setTrackAddOpen(true)
    window.addEventListener(OPEN_TRACK_ADD_DIALOG_EVENT, openDialog)
    return () => window.removeEventListener(OPEN_TRACK_ADD_DIALOG_EVENT, openDialog)
  }, [])

  return (
    <div className="tool-bar">
      <TrackAddDialog open={trackAddOpen} onClose={() => setTrackAddOpen(false)} />
      <div className="toolbar-group file-tools">
        <button className="icon-button add-track-button" title="트랙 추가" onClick={requestTrackAddDialog}><Plus size={17} /></button>
        <button className="icon-button" title="프로젝트 열기 (Ctrl+O)" onClick={() => void openProjectFile(engine)}><FolderOpen size={16} /></button>
        <button className="icon-button" title="저장 (Ctrl+S)" onClick={() => void saveProjectFile(engine)}><Save size={16} /></button>
        <button className="icon-button" title="내보내기 (Ctrl+Shift+E)" onClick={() => store.setExportDialogOpen(true)}><Download size={16} /></button>
        <button className="icon-button" title="실행취소 (Ctrl+Z)" disabled={!canUndo} onClick={store.undo}><Undo2 size={16} /></button>
        <button className="icon-button" title="다시 실행 (Ctrl+Shift+Z)" disabled={!canRedo} onClick={store.redo}><Redo2 size={16} /></button>
      </div>
      <div className="toolbar-group tool-picker" aria-label={`현재 도구: ${effective}`}>
        {tools.map(({ id, label, icon: Icon }, index) => (
          <button key={id} className={`tool-button ${toolState.activeTool === id ? 'active' : ''} ${toolState.activeTool === 'arrow' && toolState.subTool === id ? 'subtool-selected' : ''} ${effective === id && toolState.activeTool !== id ? 'temporary' : ''}`} title={`${index + 1} · ${label}${toolState.activeTool === 'arrow' && toolState.subTool === id ? ' · 1번 서브툴' : ''}`} onClick={() => toolState.chooseTool(id)}>
            <Icon size={16} /><kbd>{index + 1}</kbd>
          </button>
        ))}
      </div>
      <div className="toolbar-group snap-tools">
        <button className={`toggle-button ${snap ? 'active' : ''}`} title={`${pianoContext ? '피아노롤' : '어레인지'} 그리드 스냅`} onClick={pianoContext ? store.togglePianoSnap : store.toggleSnap}><Magnet size={15} />{pianoContext ? 'P' : 'A'} 스냅</button>
        <label className="grid-select" title={`${pianoContext ? '피아노롤' : '어레인지'} 퀀타이즈 단위`}>
          <select aria-label="그리드 단위" value={gridTicks} onChange={(event) => (pianoContext ? store.setPianoGridTicks : store.setGridTicks)(Number(event.target.value))}>
            {GRID_OPTIONS.map((grid) => <option key={grid.label} value={grid.ticks}>{grid.label}</option>)}
          </select>
        </label>
        <label className="swing-control" title="스윙 수치는 더블클릭하여 직접 입력할 수 있습니다"><span>SWING</span><input aria-label="스윙" type="range" min="0" max="100" step="1" value={swing} onChange={(event) => (pianoContext ? store.setPianoSwing : store.setArrangementSwing)(Number(event.target.value))} /><EditableNumber value={swing} min={0} max={100} step={1} onChange={pianoContext ? store.setPianoSwing : store.setArrangementSwing} format={(value) => `${Math.round(value)}%`} /></label>
        <button className={`toggle-button ${follow ? 'active' : ''}`} title="재생 중 플레이헤드를 오토 스크롤합니다 (Shift+F)" onClick={toggleFollow}><Crosshair size={14} />오토 스크롤</button>
      </div>
      <div className="toolbar-spacer" />
      <button className="audio-settings-button" onClick={() => store.setAudioSettingsOpen(true)} title="오디오 설정"><Settings size={14} />오디오 설정</button>
      <div className="toolbar-group zoom-tools">
        <button className="icon-button" title="축소 (-)" onClick={() => store.setZoom(zoom / 1.25)}><ZoomOut size={15} /></button>
        <input aria-label="Timeline zoom" type="range" min="4" max="400" value={zoom} onChange={(event) => store.setZoom(Number(event.target.value))} />
        <button className="icon-button" title="확대 (+)" onClick={() => store.setZoom(zoom * 1.25)}><ZoomIn size={15} /></button>
      </div>
      <button className={`import-button ${browserVisible ? 'active' : ''}`} onClick={() => store.toggleBrowser()}><PanelRightOpen size={15} />미디어 브라우저</button>
    </div>
  )
}
