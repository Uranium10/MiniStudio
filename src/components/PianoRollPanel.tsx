// Independent resizable MIDI editor pane. It is intentionally separate from the device/mixer pane.
import { useProjectStore } from '../store/projectStore'
import { PianoRoll } from './PianoRoll'

export function PianoRollPanel() {
  const open = useProjectStore((state) => state.pianoRollOpen)
  const setHeight = useProjectStore((state) => state.setPianoRollHeight)

  const beginResize = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    const startY = event.clientY
    const startHeight = useProjectStore.getState().pianoRollHeight
    event.currentTarget.setPointerCapture(event.pointerId)
    const move = (pointer: PointerEvent) => setHeight(startHeight + startY - pointer.clientY)
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      window.removeEventListener('pointercancel', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up, { once: true })
    window.addEventListener('pointercancel', up, { once: true })
  }

  return (
    <section className={`piano-panel ${open ? '' : 'collapsed'}`}>
      {open && <>
        <div className="panel-splitter" onPointerDown={beginResize}><span /></div>
        <PianoRoll />
      </>}
    </section>
  )
}
