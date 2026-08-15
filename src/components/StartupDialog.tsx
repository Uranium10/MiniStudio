import { FolderOpen, History, Music2, Plus } from 'lucide-react'
import { useState } from 'react'
import { useEngine } from '../hooks/useEngine'
import { discardRecoverySnapshot, readRecoverySnapshot } from '../io/autosave'
import { hydrateProjectAudio } from '../io/projectFiles'
import { openProjectFile } from '../store/commands'
import { useProjectStore } from '../store/projectStore'

export function StartupDialog() {
  const [open, setOpen] = useState(true)
  const [loading, setLoading] = useState(false)
  const [recovery] = useState(readRecoverySnapshot)
  const engine = useEngine()
  if (!open) return null

  const finish = () => {
    setOpen(false)
    window.dispatchEvent(new Event('ministudio:session-started'))
  }
  const createBlank = () => {
    discardRecoverySnapshot()
    useProjectStore.getState().newProject()
    finish()
  }
  const load = async () => {
    setLoading(true)
    try {
      if (await openProjectFile(engine)) {
        discardRecoverySnapshot()
        finish()
      }
    } finally { setLoading(false) }
  }
  const recover = async () => {
    if (!recovery) return
    setLoading(true)
    try {
      const hydrated = await hydrateProjectAudio(engine, recovery.project)
      const store = useProjectStore.getState()
      store.setProject(hydrated.project)
      store.setMissingAssets(hydrated.missing)
      finish()
    } finally { setLoading(false) }
  }

  return <div className="modal-backdrop startup-backdrop"><section className="startup-dialog" role="dialog" aria-modal="true" aria-labelledby="startup-title">
    <header><span><Music2 size={25} /></span><div><small>MINISTUDIO</small><strong id="startup-title">무엇을 시작할까요?</strong></div></header>
    <main>
      {recovery && <button className="startup-blank startup-recovery" disabled={loading} onClick={() => void recover()}><i><History size={22} /></i><span><strong>복구된 프로젝트</strong><small>{recovery.project.meta.name} · {new Date(recovery.savedAt).toLocaleString('ko-KR')}</small></span></button>}
      <button className="startup-blank" disabled={loading} onClick={createBlank}><i><Plus size={22} /></i><span><strong>빈 프로젝트</strong><small>새로운 세션을 처음부터 시작합니다.</small></span></button>
    </main>
    <footer><button disabled={loading} onClick={() => void load()}><FolderOpen size={15} />{loading ? '불러오는 중…' : '불러오기'}</button></footer>
  </section></div>
}
