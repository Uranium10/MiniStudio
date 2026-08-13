// Missing-project-asset list and path relinking workflow.
import { FileQuestion, FolderSearch, X } from 'lucide-react'
import { useEngine } from '../hooks/useEngine'
import { chooseAudioFile } from '../io/projectFiles'
import { useProjectStore } from '../store/projectStore'

export function MissingAssetsDialog() {
  const missing = useProjectStore((state) => state.missingAssets)
  const setMissing = useProjectStore((state) => state.setMissingAssets)
  const replace = useProjectStore((state) => state.replaceAsset)
  const showToast = useProjectStore((state) => state.showToast)
  const engine = useEngine()
  if (missing.length === 0) return null

  const relink = async (id: string) => {
    try {
      const path = await chooseAudioFile()
      if (!path) return
      replace(id, await engine.loadAudioFile(path))
    } catch { showToast('대체 오디오 파일을 읽을 수 없습니다') }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section className="missing-dialog" role="dialog" aria-modal="true" aria-labelledby="missing-title">
        <header><FileQuestion size={19} /><div><strong id="missing-title">누락된 오디오 파일</strong><small>프로젝트의 클립을 다시 연결하세요.</small></div><button onClick={() => setMissing([])} title="닫기"><X size={16} /></button></header>
        <div className="missing-list">{missing.map((asset) => <div key={asset.id}><span>{asset.name}</span><button onClick={() => void relink(asset.id)}><FolderSearch size={13} />재지정</button></div>)}</div>
        <footer><span>{missing.length}개 파일을 찾을 수 없습니다.</span><button onClick={() => setMissing([])}>나중에</button></footer>
      </section>
    </div>
  )
}
