import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import '@fontsource-variable/figtree/wght.css'
import '@fontsource/jetbrains-mono/latin-400.css'
import '@fontsource/jetbrains-mono/latin-500.css'
import { App } from './app/App'
import { registerServiceWorker } from './app/serviceWorker'
import { installErrorReporting } from './telemetry/reportErrors'
import './styles/tokens.css'
import './styles/base.css'
import './styles/components.css'
import './styles/shell.css'
import './styles/conversation.css'
import './styles/pages.css'
import './styles/work.css'
import './styles/kit.css'
import './styles/markdown.css'
import '@fontsource-variable/source-serif-4/opsz.css'
import './styles/directions.css'
import { applyDirection } from './theme/direction'

installErrorReporting()
applyDirection()

const root = document.getElementById('root')

if (!root) {
  throw new Error('Missing root element')
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)

registerServiceWorker()
