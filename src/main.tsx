// React entry point and engine-context bootstrap.
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import { PluginEditorShell } from './components/PluginEditorShell'
import { EngineProvider } from './hooks/useEngine'
import './index.css'

const root = document.getElementById('root')

if (!root) throw new Error('MiniStudio root element is missing')

const isPluginShell = new URLSearchParams(window.location.search).get('pluginShell') === '1'

createRoot(root).render(
  <StrictMode>
    {isPluginShell ? <PluginEditorShell /> : <EngineProvider><App /></EngineProvider>}
  </StrictMode>,
)
