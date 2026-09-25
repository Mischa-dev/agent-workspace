import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import '@fontsource-variable/figtree/wght.css'
import '@fontsource/jetbrains-mono/latin-400.css'
import '@fontsource/jetbrains-mono/latin-500.css'
import { App } from './app/App'
// Pre-redesign styles for pages not yet moved over (project home, work items, settings).
import './styles.css'
import './styles/features.css'
import './styles/tokens.css'
import './styles/base.css'
import './styles/components.css'
import './styles/shell.css'
import './styles/conversation.css'
import './styles/pages.css'

const root = document.getElementById('root')

if (!root) {
  throw new Error('Missing root element')
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
