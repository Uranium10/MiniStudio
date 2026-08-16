// Global shortcut dispatcher honoring text-input focus and smart-tool modifiers.
import { useEffect } from 'react'
import { useEngine } from '../hooks/useEngine'
import {
  copyInContext, deleteInContext, openProjectFile, pasteInContext, saveProjectFile,
  quantizeInContext, seekTo, selectAllInContext, splitSelectionAtPlayhead, togglePlayback,
} from '../store/commands'
import { useProjectStore } from '../store/projectStore'
import { nextSubTool, useToolStore } from '../store/toolStore'

export function useShortcuts(): void {
  const engine = useEngine()

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const tool = useToolStore.getState()
      const store = useProjectStore.getState()
      if (event.key === 'Control' || event.key === 'Meta') {
        tool.setModifierHeld(true)
        return
      }
      // F1 works even from a text field; everything else defers to the editor.
      if (event.key === 'F1') { event.preventDefault(); store.setShortcutsOpen(!store.shortcutsOpen); return }
      if (event.key === 'F2') { event.preventDefault(); store.togglePianoRoll(); return }
      if (event.key === 'F3') { event.preventDefault(); store.toggleLowerPanel(); return }
      if (event.key === 'F4') { event.preventDefault(); store.toggleInspector(); return }
      if (event.key === 'F5') { event.preventDefault(); store.toggleBrowser(); return }
      if (isEditable(event.target)) return
      if (store.virtualPianoOpen) return
      if (store.shortcutsOpen && event.key === 'Escape') { event.preventDefault(); store.setShortcutsOpen(false); return }

      if (/^[1-7]$/.test(event.key) && !event.ctrlKey && !event.metaKey) {
        if (store.editFocus === 'pianoRoll' && store.pianoRollOpen) {
          tool.pressPianoNumber(Number(event.key))
        } else {
          const wasArrow = tool.activeTool === 'arrow'
          const next = event.key === '1' && wasArrow ? nextSubTool(tool.subTool) : null
          tool.pressNumber(Number(event.key))
          if (next) store.showToast(`서브 도구: ${toolLabel(next)}`)
        }
        event.preventDefault()
        return
      }

      const command = keyToCommand(event)
      if (!command) return
      event.preventDefault()
      switch (command) {
        case 'play': void togglePlayback(engine); break
        case 'home': seekTo(engine, 0); break
        case 'loop': store.toggleLoop(); break
        case 'loopSelection': store.setLoopToSelection(); break
        case 'metronome': store.setMetronomeEnabled(!store.metronomeEnabled); break
        case 'delete': deleteInContext(); break
        case 'split': splitSelectionAtPlayhead(); break
        case 'undo': store.undo(); break
        case 'redo': store.redo(); break
        case 'copy': copyInContext(); break
        case 'cut': store.cutSelectedClips(); break
        case 'paste': pasteInContext(); break
        case 'duplicate': store.duplicateSelectedClips(); break
        case 'smartDuplicate': if (store.selectedAutomationPoints.length) store.duplicateSelectedAutomationPoints(); else if (store.editFocus === 'pianoRoll') store.duplicateSelectedMidiNotes(); else store.duplicateSelectedClipsSmart(); break
        case 'muteClips': store.toggleSelectedClipsMuted(); break
        case 'selectAll': selectAllInContext(); break
        case 'quantize': quantizeInContext(); break
        case 'zoomIn': zoomHorizontal(1.18); break
        case 'zoomOut': zoomHorizontal(1 / 1.18); break
        case 'trackIn': store.resizeAllTracks(8); break
        case 'trackOut': store.resizeAllTracks(-8); break
        case 'fit': store.setZoom(22); break
        case 'follow': store.toggleFollowPlayhead(); store.showToast(store.followPlayhead ? '오토 스크롤 해제' : '오토 스크롤'); break
        case 'panel': store.toggleLowerPanel(); break
        case 'tab': store.setLowerTab(store.lowerTab === 'mixer' ? 'effects' : 'mixer'); break
        case 'save': void saveProjectFile(engine); break
        case 'new': if (window.confirm('현재 프로젝트를 닫고 새 프로젝트를 시작할까요? 저장하지 않은 변경은 사라집니다.')) { void engine.stop(); store.newProject() }; break
        case 'open': void openProjectFile(engine); break
        case 'export': store.setExportDialogOpen(true); break
        case 'transposeUp':
        case 'transposeDown': transposeSelection(command === 'transposeUp' ? 1 : -1, event.ctrlKey || event.metaKey); break
      }
    }

    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === 'Control' || event.key === 'Meta') useToolStore.getState().setModifierHeld(false)
    }
    const onBlur = () => useToolStore.getState().resetModifiers()
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', onBlur)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', onBlur)
    }
  }, [engine])
}

/** Transposes every selected note in one batch so it lands as a single undo step. */
function transposeSelection(direction: 1 | -1, octave: boolean): void {
  const store = useProjectStore.getState()
  const editor = store.editorClip
  if (store.editFocus !== 'pianoRoll' || !editor || !store.selectedNoteIds.length) return
  const clip = store.project.tracks.find((track) => track.id === editor.trackId)?.midiClips.find((item) => item.id === editor.clipId)
  if (!clip) return
  const amount = direction * (octave ? 12 : 1)
  const selected = new Set(store.selectedNoteIds)
  store.updateMidiNoteBatch(editor.trackId, editor.clipId, clip.notes.filter((note) => selected.has(note.id)).map((note) => ({ id: note.id, patch: { pitch: note.pitch + amount } })))
}

function zoomHorizontal(factor: number): void {
  const store = useProjectStore.getState()
  if (store.editFocus === 'pianoRoll' && store.pianoRollOpen && store.editorClip) store.setPianoRollZoom(store.pianoRollZoom * factor)
  else store.setZoom(store.pixelsPerSecond * factor)
}

type Command =
  | 'play' | 'home' | 'loop' | 'loopSelection' | 'metronome'
  | 'delete' | 'split' | 'undo' | 'redo' | 'duplicate' | 'smartDuplicate' | 'selectAll' | 'copy' | 'cut' | 'paste' | 'muteClips'
  | 'transposeUp' | 'transposeDown' | 'quantize'
  | 'zoomIn' | 'zoomOut' | 'trackIn' | 'trackOut' | 'fit' | 'follow' | 'panel' | 'tab'
  | 'new' | 'save' | 'open' | 'export'

function keyToCommand(event: KeyboardEvent): Command | null {
  const mod = event.ctrlKey || event.metaKey
  const key = event.key.toLowerCase()
  if (event.code === 'Space') return 'play'
  if (event.key === 'Enter') return 'home'
  if (key === 'b' && !mod) return 'home'
  if (event.key === 'Delete' || event.key === 'Backspace') return 'delete'
  if (event.key === 'ArrowUp') return 'transposeUp'
  if (event.key === 'ArrowDown') return 'transposeDown'
  if (mod && key === 'z') return event.shiftKey ? 'redo' : 'undo'
  if (mod && key === 'y') return 'redo'
  if (mod && key === 'c') return 'copy'
  if (mod && key === 'x') return 'cut'
  if (mod && key === 'v') return 'paste'
  if (mod && key === 'd') return 'duplicate'
  if (mod && key === 'a') return 'selectAll'
  if (mod && key === 'n') return 'new'
  if (mod && key === 's') return 'save'
  if (mod && key === 'o') return 'open'
  if (mod && event.shiftKey && key === 'e') return 'export'
  if (mod) return null
  if (event.shiftKey && key === 'e') return 'trackIn'
  if (event.shiftKey && key === 'w') return 'trackOut'
  if (key === 'w') return 'zoomOut'
  if (key === 'e') return 'zoomIn'
  if (key === 'l') return event.shiftKey ? 'loopSelection' : 'loop'
  if (key === 'c') return 'metronome'
  if (key === 's') return 'split'
  if (key === 'm') return 'muteClips'
  if (key === 'd') return 'smartDuplicate'
  if (key === 'q') return 'quantize'
  if (key === 'f') return event.shiftKey ? 'follow' : 'fit'
  if (event.key === 'Tab') return event.shiftKey ? 'tab' : 'panel'
  if (event.key === '+' || event.key === '=') return 'zoomIn'
  if (event.key === '-' || event.key === '_') return 'zoomOut'
  return null
}

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  if (target instanceof HTMLInputElement) return !target.readOnly && !target.disabled
  if (target instanceof HTMLTextAreaElement) return !target.readOnly && !target.disabled
  return target instanceof HTMLSelectElement && !target.disabled
}

function toolLabel(tool: string): string {
  return ({ none: '없음', range: '범위 선택', split: '스플릿', erase: '지우개', paint: '그리기', mute: '뮤트' } as Record<string, string>)[tool] ?? tool
}
