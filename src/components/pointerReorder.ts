import type { PointerEvent as ReactPointerEvent } from 'react'

export function reorderDestinationIndex(fromIndex: number, hoveredIndex: number, after: boolean, itemCount: number): number {
  const insertionIndex = after
    ? hoveredIndex + (fromIndex > hoveredIndex ? 1 : 0)
    : hoveredIndex - (fromIndex < hoveredIndex ? 1 : 0)
  return Math.max(0, Math.min(itemCount - 1, insertionIndex))
}

/** WebView-safe reorder gesture that keeps tracking outside the handle. */
export function beginPointerReorder(event: ReactPointerEvent<HTMLElement>, options: {
  itemSelector: string
  indexAttribute: string
  axis: 'horizontal' | 'vertical'
  scrollSelector: string
  onCommit(fromIndex: number, toIndex: number): void
}): void {
  if (event.button !== 0) return
  event.preventDefault()
  event.stopPropagation()
  const handle = event.currentTarget
  const source = handle.closest<HTMLElement>(options.itemSelector)
  const fromIndex = Number.parseInt(source?.getAttribute(options.indexAttribute) ?? '', 10)
  if (!source || !Number.isInteger(fromIndex)) return

  let target = source
  let toIndex = fromIndex
  const scroll = handle.closest<HTMLElement>(options.scrollSelector)
  const pointerId = event.pointerId
  handle.setPointerCapture(pointerId)
  source.classList.add('reorder-source')
  document.body.classList.add('pointer-reordering')

  const clearTarget = () => target.classList.remove('reorder-target', 'reorder-before', 'reorder-after')
  const update = (pointer: PointerEvent) => {
    pointer.preventDefault()
    const directHit = document.elementsFromPoint(pointer.clientX, pointer.clientY)
      .map((element) => element.closest<HTMLElement>(options.itemSelector))
      .find((element): element is HTMLElement => Boolean(element))
    // Track by the requested axis even when the pointer leaves the narrow
    // header/card column. This is the same ergonomic guarantee as a knob's
    // pointer capture and is particularly important inside a WebView.
    const hit = directHit ?? Array.from(document.querySelectorAll<HTMLElement>(options.itemSelector))
      .filter((element) => element.getClientRects().length > 0)
      .sort((left, right) => {
        const leftBounds = left.getBoundingClientRect()
        const rightBounds = right.getBoundingClientRect()
        const coordinate = options.axis === 'horizontal' ? pointer.clientX : pointer.clientY
        const leftCenter = options.axis === 'horizontal' ? leftBounds.left + leftBounds.width / 2 : leftBounds.top + leftBounds.height / 2
        const rightCenter = options.axis === 'horizontal' ? rightBounds.left + rightBounds.width / 2 : rightBounds.top + rightBounds.height / 2
        return Math.abs(coordinate - leftCenter) - Math.abs(coordinate - rightCenter)
      })[0]
    if (hit) {
      const index = Number.parseInt(hit.getAttribute(options.indexAttribute) ?? '', 10)
      if (Number.isInteger(index)) {
        clearTarget()
        target = hit
        const bounds = hit.getBoundingClientRect()
        const after = options.axis === 'horizontal'
          ? pointer.clientX >= bounds.left + bounds.width / 2
          : pointer.clientY >= bounds.top + bounds.height / 2
        // The store expects the final array index after removing the source.
        // Convert the visual insertion edge to that index so dragging across
        // more than one item never lands one slot early or late.
        const itemCount = document.querySelectorAll(options.itemSelector).length
        toIndex = reorderDestinationIndex(fromIndex, index, after, itemCount)
        target.classList.add('reorder-target', after ? 'reorder-after' : 'reorder-before')
      }
    }
    if (scroll) {
      const bounds = scroll.getBoundingClientRect()
      const coordinate = options.axis === 'horizontal' ? pointer.clientX : pointer.clientY
      const start = options.axis === 'horizontal' ? bounds.left : bounds.top
      const end = options.axis === 'horizontal' ? bounds.right : bounds.bottom
      const delta = coordinate < start + 30 ? -14 : coordinate > end - 30 ? 14 : 0
      if (options.axis === 'horizontal') scroll.scrollLeft += delta
      else scroll.scrollTop += delta
    }
  }
  const cleanup = () => {
    window.removeEventListener('pointermove', update)
    window.removeEventListener('pointerup', finish)
    window.removeEventListener('pointercancel', cancel)
    clearTarget()
    source.classList.remove('reorder-source')
    document.body.classList.remove('pointer-reordering')
    if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId)
  }
  const finish = () => {
    cleanup()
    if (toIndex !== fromIndex) options.onCommit(fromIndex, toIndex)
  }
  const cancel = () => {
    toIndex = fromIndex
    cleanup()
  }
  window.addEventListener('pointermove', update, { passive: false })
  window.addEventListener('pointerup', finish, { once: true })
  window.addEventListener('pointercancel', cancel, { once: true })
}
