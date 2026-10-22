// The trailer page. It loads the app's own fonts and stylesheets in the app's
// order, so every token, theme and kit component looks exactly as it does in
// the product, then draws one frame of the trailer for any moment in time.
import '@fontsource-variable/figtree/wght.css'
import '@fontsource/jetbrains-mono/latin-400.css'
import '@fontsource/jetbrains-mono/latin-500.css'
import '../../src/styles/tokens.css'
import '../../src/styles/base.css'
import '../../src/styles/components.css'
import '../../src/styles/shell.css'
import '../../src/styles/conversation.css'
import '../../src/styles/pages.css'
import '../../src/styles/work.css'
import '../../src/styles/kit.css'
import '../../src/styles/markdown.css'
import '../../src/conversations/chat.css'
import '../../src/conversations/messageMarkdown.css'
import '../../src/work-items/board.css'
import '../../src/work-items/work.css'
import '../../src/files/files.css'
import '../../src/inbox/inbox.css'
import '../../src/runs/runs.css'
import '../../src/launch/launch.css'
import '../../src/later/later.css'
import '../../src/views/views.css'
import '../../src/agents/agents.css'
import '../../src/automations/automations.css'
import '../../src/whiteboards/whiteboards.css'
import '../../src/projects/home.css'
import '../../src/projects/overview.css'
import '../../src/ai/ai.css'
import './trailer.css'

import { useState } from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { MemoryRouter } from 'react-router-dom'

import excalifont from '@excalifont?url'

import { museMock } from './data'
import { LOGO_URLS } from './logos'
import { ExportMock } from './Mock'
import { initMotion } from './motion'
import { DURATION, Trailer } from './Trailer'

declare global {
  interface Window {
    __trailer?: {
      duration: number
      seek: (seconds: number) => void
      ready: Promise<unknown>
    }
  }
}

// The whiteboards' handwriting: Excalifont's Latin face, from the Excalidraw package (vite.config.ts).
document.fonts.add(new FontFace('Excalifont', `url(${excalifont}) format('woff2')`, { display: 'block' }))

const params = new URLSearchParams(window.location.search)
let setTime: (seconds: number) => void = () => undefined

function Root() {
  const [t, set] = useState(() => Number(params.get('t') ?? 0))
  setTime = set
  // The pictures messages attach are drawn here too, and captured once (asset.mjs).
  if (params.get('asset') === 'export-mock') return <ExportMock />
  return (
    <MemoryRouter>
      <Trailer t={t} />
    </MemoryRouter>
  )
}

/** CSS animations in the kit (working dots, spinners, breathing rings) follow the trailer's clock, not the wall clock. */
function syncAnimations(seconds: number) {
  for (const animation of document.getAnimations()) {
    animation.pause()
    animation.currentTime = seconds * 1000
  }
}

// Every face the trailer uses, loaded before the first frame so no frame ever falls back.
const FACES = [
  '400 16px "Figtree Variable"',
  '500 16px "Figtree Variable"',
  '600 16px "Figtree Variable"',
  '700 16px "Figtree Variable"',
  '400 16px "JetBrains Mono"',
  '500 16px "JetBrains Mono"',
  '400 16px Excalifont',
]

// Pictures messages attach, and the agents' logos, decoded up front so they're there the first frame they're shown.
const PICTURES = [...[museMock.id].map((id) => `/api/v1/files/${id}/thumbnail`), ...LOGO_URLS]

async function preload(url: string) {
  const image = new Image()
  image.src = url
  await image.decode().catch(() => undefined)
}

async function start() {
  await Promise.all([...FACES.map((face) => document.fonts.load(face)), ...(params.has('asset') ? [] : PICTURES.map(preload))])
  await document.fonts.ready
  initMotion()
  const root = document.getElementById('root')
  if (!root) throw new Error('Missing root element')
  createRoot(root).render(<Root />)
  // Let React mount once before the first seek.
  await new Promise((resolve) => requestAnimationFrame(() => resolve(null)))
  const seek = (seconds: number) => {
    flushSync(() => setTime(seconds))
    syncAnimations(seconds)
  }
  window.__trailer = { duration: DURATION, seek, ready: Promise.resolve(true) }
  seek(Number(params.get('t') ?? 0))
  if (params.has('play')) {
    const began = performance.now() - Number(params.get('t') ?? 0) * 1000
    const tick = () => {
      const seconds = ((performance.now() - began) / 1000) % DURATION
      seek(seconds)
      requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }
  document.documentElement.dataset.ready = 'true'
}

void start()
