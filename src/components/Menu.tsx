// Shared floating panels used by the menu bar, context menus, and pickers.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

export type MenuItem =
  | { kind: 'separator' }
  | { kind: 'label'; label: string }
  | { kind: 'item'; label: string; keys?: string; danger?: boolean; disabled?: boolean; checked?: boolean; icon?: ReactNode; run(): void }

type Point = { x: number; y: number }

type FloatingPanelProps = {
  children: ReactNode
  onClose(): void
  anchor?: Point
  getAnchorElement?: () => HTMLElement | null
  className?: string
  role?: 'dialog' | 'listbox' | 'menu'
}

const VIEWPORT_GAP = 6
const ANCHOR_GAP = 3

/**
 * Renders into document.body so no workspace panel can clip or cover the popup.
 * Element-anchored panels flip above their trigger when there is no room below.
 */
export function FloatingPanel({ children, onClose, anchor, getAnchorElement, className = '', role = 'dialog' }: FloatingPanelProps) {
  const ref = useRef<HTMLDivElement>(null)
  const [style, setStyle] = useState<CSSProperties>({ left: 0, top: 0, visibility: 'hidden' })
  const anchorX = anchor?.x
  const anchorY = anchor?.y

  const updatePosition = useCallback(() => {
    const panel = ref.current
    if (!panel) return

    const anchorElement = getAnchorElement?.() ?? null
    const anchorRect = anchorElement?.getBoundingClientRect()
    const panelRect = panel.getBoundingClientRect()
    const desiredLeft = anchorX ?? anchorRect?.left ?? VIEWPORT_GAP
    const desiredTop = anchorY ?? ((anchorRect?.bottom ?? VIEWPORT_GAP) + ANCHOR_GAP)
    const maxLeft = Math.max(VIEWPORT_GAP, window.innerWidth - panelRect.width - VIEWPORT_GAP)
    const maxTop = Math.max(VIEWPORT_GAP, window.innerHeight - panelRect.height - VIEWPORT_GAP)
    const left = Math.min(Math.max(VIEWPORT_GAP, desiredLeft), maxLeft)
    const shouldFlip = anchorX === undefined && anchorY === undefined && anchorRect && desiredTop + panelRect.height > window.innerHeight - VIEWPORT_GAP
    const flippedTop = anchorRect ? anchorRect.top - panelRect.height - ANCHOR_GAP : desiredTop
    const top = Math.min(Math.max(VIEWPORT_GAP, shouldFlip ? flippedTop : desiredTop), maxTop)

    setStyle({ left, top, visibility: 'visible' })
  }, [anchorX, anchorY, getAnchorElement])

  useLayoutEffect(() => {
    updatePosition()
    const panel = ref.current
    const anchorElement = getAnchorElement?.() ?? null
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(updatePosition)
    if (panel) observer?.observe(panel)
    if (anchorElement) observer?.observe(anchorElement)
    window.addEventListener('resize', updatePosition)
    window.addEventListener('scroll', updatePosition, true)
    return () => {
      observer?.disconnect()
      window.removeEventListener('resize', updatePosition)
      window.removeEventListener('scroll', updatePosition, true)
    }
  }, [getAnchorElement, updatePosition])

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node
      const anchorElement = getAnchorElement?.() ?? null
      if (!ref.current?.contains(target) && !anchorElement?.contains(target)) onClose()
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      event.stopPropagation()
      onClose()
    }
    window.addEventListener('pointerdown', onPointerDown, true)
    window.addEventListener('keydown', onKeyDown, true)
    window.addEventListener('blur', onClose)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown, true)
      window.removeEventListener('keydown', onKeyDown, true)
      window.removeEventListener('blur', onClose)
    }
  }, [getAnchorElement, onClose])

  return createPortal(
    <div
      ref={ref}
      className={`floating-panel ${className}`}
      style={style}
      role={role}
      onPointerDown={(event) => event.stopPropagation()}
      onClick={(event) => event.stopPropagation()}
    >
      {children}
    </div>,
    document.body,
  )
}

/** Renders a standard command menu in the shared top-level floating layer. */
export function MenuPanel({ items, onClose, anchor, className = '' }: { items: MenuItem[]; onClose(): void; anchor?: Point; className?: string }) {
  const originRef = useRef<HTMLSpanElement>(null)
  const getAnchorElement = useCallback(() => originRef.current?.parentElement ?? null, [])

  return (
    <>
      {!anchor && <span ref={originRef} className="menu-anchor-sentinel" aria-hidden="true" />}
      <FloatingPanel anchor={anchor} getAnchorElement={anchor ? undefined : getAnchorElement} onClose={onClose} className={`menu-panel ${className}`} role="menu">
        {items.map((item, index) => {
          if (item.kind === 'separator') return <hr key={`sep-${index}`} />
          if (item.kind === 'label') return <strong key={`label-${index}`}>{item.label}</strong>
          return (
            <button
              key={`${item.label}-${index}`}
              type="button"
              role="menuitem"
              className={`${item.danger ? 'danger' : ''} ${item.checked ? 'checked' : ''}`}
              disabled={item.disabled}
              onClick={() => { item.run(); onClose() }}
            >
              <i className="menu-mark">{item.checked ? '✓' : item.icon}</i>
              <span>{item.label}</span>
              {item.keys && <kbd>{item.keys}</kbd>}
            </button>
          )
        })}
      </FloatingPanel>
    </>
  )
}
