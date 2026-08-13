import type { EffectType, ExternalPluginRef } from '../engine'

export const BROWSER_DRAG_TYPE = 'application/x-minidaw-browser-item'

export type BrowserDragPayload =
  | { kind: 'instrument'; plugin?: ExternalPluginRef }
  | { kind: 'effect'; type: EffectType; plugin?: ExternalPluginRef }
  | { kind: 'media'; path: string; name: string }

export function writeBrowserDrag(event: React.DragEvent, payload: BrowserDragPayload): void {
  event.dataTransfer.effectAllowed = payload.kind === 'effect' ? 'copyLink' : 'copy'
  event.dataTransfer.setData(BROWSER_DRAG_TYPE, JSON.stringify(payload))
  event.dataTransfer.setData('text/plain', payload.kind === 'media' ? payload.path : payload.plugin?.name ?? payload.kind)
}

export function readBrowserDrag(transfer: DataTransfer): BrowserDragPayload | null {
  const value = transfer.getData(BROWSER_DRAG_TYPE)
  if (!value) return null
  try {
    const payload = JSON.parse(value) as BrowserDragPayload
    if (payload.kind === 'media' && typeof payload.path === 'string') return payload
    if (payload.kind === 'instrument') return payload
    if (payload.kind === 'effect' && typeof payload.type === 'string') return payload
  } catch { /* Ignore malformed third-party drag data. */ }
  return null
}
