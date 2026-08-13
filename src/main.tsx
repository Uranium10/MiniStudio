// React entry point and engine-context bootstrap.
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import { EngineProvider } from './hooks/useEngine'
import './index.css'

const root = document.getElementById('root')

if (!root) throw new Error('MiniDAW root element is missing')

createRoot(root).render(
  <StrictMode>
    <EngineProvider>
      <App />
    </EngineProvider>
  </StrictMode>,
)
