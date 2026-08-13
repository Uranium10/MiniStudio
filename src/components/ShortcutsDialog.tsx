// Keyboard reference driven by the declarative shortcut registry.
import { Keyboard, X } from 'lucide-react'
import { shortcutGroups } from '../shortcuts/registry'
import { useProjectStore } from '../store/projectStore'

export function ShortcutsDialog() {
  const open = useProjectStore((state) => state.shortcutsOpen)
  const close = useProjectStore((state) => state.setShortcutsOpen)
  if (!open) return null
  return (
    <div className="modal-backdrop" role="presentation" onClick={() => close(false)}>
      <section className="shortcuts-dialog" role="dialog" aria-modal="true" aria-labelledby="shortcuts-title" onClick={(event) => event.stopPropagation()}>
        <header><Keyboard size={19} /><div><strong id="shortcuts-title">키보드 단축키</strong><small>텍스트 입력 중에는 편집 단축키가 동작하지 않습니다.</small></div><button onClick={() => close(false)} title="닫기"><X size={16} /></button></header>
        <div className="shortcuts-body">
          {shortcutGroups.map((group) => (
            <section key={group.title}>
              <h3>{group.title}</h3>
              {group.items.map((shortcut) => <div key={shortcut.id}><span>{shortcut.label}</span><kbd>{shortcut.keys}</kbd></div>)}
            </section>
          ))}
        </div>
      </section>
    </div>
  )
}
