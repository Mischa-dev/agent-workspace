import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { App } from './app/App'
import './styles.css'
import './styles/tokens.css'
import './styles/base.css'
import './styles/primitives.css'
import './styles/shell.css'
import './styles/features.css'

const root = document.getElementById('root')

if (!root) {
  throw new Error('Missing root element')
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
