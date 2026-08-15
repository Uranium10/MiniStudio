import type { PointerEvent as ReactPointerEvent } from 'react'
import type { EffectType, ExternalPluginRef } from '../engine'

export type BrowserDragPayload =
  | { kind: 'instrument'; plugin?: ExternalPluginRef }
  | { kind: 'effect'; type: EffectType; plugin?: ExternalPluginRef }
  | { kind: 'media'; path: string; name: string }

export type BrowserDragState = { type: 'move' | 'drop' | 'cancel'; payload: BrowserDragPayload; x: number; y: number }

const DRAG_EVENT = 'minidaw-browser-drag'
/** Pointer must travel this far before a press becomes a drag, so a plain click/double-click
 * on a browser item is never swallowed into a zero-distance drag-and-drop gesture. */
const DRAG_THRESHOLD = 4

let ghost: HTMLDivElement | null = null

function labelFor(payload: BrowserDragPayload): string {
  if (payload.kind === 'media') return payload.name
  if (payload.kind === 'instrument') return payload.plugin?.name ?? 'DefaultSynth'
  return payload.plugin?.name ?? payload.type.replace(/^builtin:/, '')
}

function showGhost(text: string, x: number, y: number): void {
  if (!ghost) {
    ghost = document.createElement('div')
    ghost.className = 'browser-drag-ghost'
    document.body.appendChild(ghost)
  }
  ghost.textContent = text
  ghost.style.left = `${x}px`
  ghost.style.top = `${y}px`
}

function hideGhost(): void {
  ghost?.remove()
  ghost = null
}

/**
 * WebView-safe replacement for HTML5 drag-and-drop. Tauri disables the DOM Drag and Drop API
 * on Windows whenever native window drag-drop is enabled (required so external Explorer file
 * drops resolve to real filesystem paths), so browser-panel items are picked up and dropped
 * with a pointer-capture gesture instead - the same pattern already used for track/effect
 * reordering (see pointerReorder.ts). Drop targets subscribe with subscribeBrowserDrag.
 */
export function beginBrowserDrag(event: ReactPointerEvent<HTMLElement>, payload: BrowserDragPayload): void {
  if (event.button !== 0) return
  const originX = event.clientX
  const originY = event.clientY
  const text = labelFor(payload)
  let dragging = false

  const cleanup = () => {
    window.removeEventListener('pointermove', move)
    window.removeEventListener('pointerup', finish)
    window.removeEventListener('pointercancel', cancel)
    window.removeEventListener('keydown', onKey)
    if (dragging) { document.body.classList.remove('browser-dragging'); hideGhost() }
  }
  const move = (pointer: PointerEvent) => {
    if (!dragging) {
      if (Math.hypot(pointer.clientX - originX, pointer.clientY - originY) < DRAG_THRESHOLD) return
      dragging = true
      document.body.classList.add('browser-dragging')
    }
    showGhost(text, pointer.clientX, pointer.clientY)
    window.dispatchEvent(new CustomEvent<BrowserDragState>(DRAG_EVENT, { detail: { type: 'move', payload, x: pointer.clientX, y: pointer.clientY } }))
  }
  const finish = (pointer: PointerEvent) => {
    const wasDragging = dragging
    cleanup()
    if (wasDragging) window.dispatchEvent(new CustomEvent<BrowserDragState>(DRAG_EVENT, { detail: { type: 'drop', payload, x: pointer.clientX, y: pointer.clientY } }))
  }
  const cancel = () => {
    const wasDragging = dragging
    cleanup()
    if (wasDragging) window.dispatchEvent(new CustomEvent<BrowserDragState>(DRAG_EVENT, { detail: { type: 'cancel', payload, x: 0, y: 0 } }))
  }
  const onKey = (keyEvent: KeyboardEvent) => { if (keyEvent.key === 'Escape') cancel() }
  window.addEventListener('pointermove', move, { passive: false })
  window.addEventListener('pointerup', finish, { once: true })
  window.addEventListener('pointercancel', cancel, { once: true })
  window.addEventListener('keydown', onKey)
}

/** Drop targets call this in a useEffect to react to live browser-item drags. `state.x/y` are
 * viewport coordinates; hit-test them with `document.elementFromPoint` (or a ref's
 * `.contains(...)` on that result) since the gesture holds pointer capture on its origin. */
export function subscribeBrowserDrag(handler: (state: BrowserDragState) => void): () => void {
  const listener = (event: Event) => handler((event as CustomEvent<BrowserDragState>).detail)
  window.addEventListener(DRAG_EVENT, listener)
  return () => window.removeEventListener(DRAG_EVENT, listener)
}
