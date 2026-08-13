// React context exposing only the engine contract to UI code.
/* oxlint-disable react/only-export-components */
import { createContext, useContext, useEffect, useRef, type ReactNode } from 'react'
import { createEngine, describeEngineError, type IAudioEngine } from '../engine'
import { useProjectStore } from '../store/projectStore'

const EngineContext = createContext<IAudioEngine | null>(null)

export function EngineProvider({ children }: { children: ReactNode }) {
  const engineRef = useRef<IAudioEngine>()
  if (!engineRef.current) engineRef.current = createEngine()

  useEffect(() => {
    const engine = engineRef.current
    void engine?.init().catch((error) => {
      useProjectStore.getState().showToast(`오디오 엔진을 시작하지 못했습니다: ${describeEngineError(error)}`)
    })
    return () => { void engine?.dispose() }
  }, [])

  return <EngineContext.Provider value={engineRef.current}>{children}</EngineContext.Provider>
}

export function useEngine(): IAudioEngine {
  const engine = useContext(EngineContext)
  if (!engine) throw new Error('useEngine must be used inside EngineProvider')
  return engine
}
