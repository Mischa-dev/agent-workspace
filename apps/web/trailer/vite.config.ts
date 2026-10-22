import { existsSync, realpathSync } from 'node:fs'
import { createRequire } from 'node:module'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import react from '@vitejs/plugin-react'
import { defineConfig, type Plugin } from 'vite'

// The trailer is its own page, built from the app's design system: the same
// tokens, stylesheets, fonts and kit components the app uses. Whiteboard
// sketches draw the way the app's whiteboards do, with Excalidraw's own
// rough.js and its Excalifont hand, both read from the installed package.
const excalidraw = realpathSync(fileURLToPath(new URL('../node_modules/@excalidraw/excalidraw', import.meta.url)))
const rough = createRequire(join(excalidraw, 'package.json')).resolve('roughjs/bundled/rough.esm.js')
// Excalidraw names its font files by hash, so an upgrade can rename this one.
const EXCALIFONT_LATIN = 'Excalifont-Regular-a88b72a24fb54c9f94e3b5fdaa7481c9.woff2'
const excalifont = join(excalidraw, 'dist/prod/fonts/Excalifont', EXCALIFONT_LATIN)
if (!existsSync(excalifont)) {
  throw new Error(`${EXCALIFONT_LATIN} isn't in this Excalidraw: set EXCALIFONT_LATIN in trailer/vite.config.ts to its Latin Excalifont file`)
}

// Every import of the app's identity module (Avatar and friends) gets the
// trailer's version instead, which gives each demo agent its maker's logo.
const identity = fileURLToPath(new URL('../src/ui/identity.tsx', import.meta.url))
const trailerIdentity = fileURLToPath(new URL('./src/identity.tsx', import.meta.url))
function agentMarks(): Plugin {
  return {
    name: 'trailer-agent-marks',
    enforce: 'pre',
    async resolveId(source, importer, options) {
      if (!importer || importer === trailerIdentity) return null
      const resolved = await this.resolve(source, importer, { ...options, skipSelf: true })
      return resolved?.id === identity ? trailerIdentity : null
    },
  }
}

export default defineConfig({
  root: __dirname,
  base: './',
  plugins: [agentMarks(), react()],
  resolve: {
    alias: [
      { find: /^roughjs$/, replacement: rough },
      { find: /^@excalifont(?=\?|$)/, replacement: excalifont },
    ],
  },
  build: { outDir: 'dist', emptyOutDir: true, assetsInlineLimit: 0 },
  server: { host: '127.0.0.1', port: 5199, fs: { allow: ['..', '../..', '../../..'] } },
})
