import { cpSync, createReadStream, existsSync, readFileSync, realpathSync, statSync } from 'node:fs'
import { dirname, join, normalize, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

import react from '@vitejs/plugin-react'
import type { Plugin } from 'vite'
import { configDefaults, defineConfig } from 'vitest/config'

// The e2e smoke tests (e2e/) point the proxy at their own throwaway server.
const apiTarget = process.env.AGENT_WORKSPACE_API_PROXY_TARGET ?? 'http://127.0.0.1:3100'

// Whiteboards draw with Excalidraw, which loads its fonts at run time. They're
// served from the app itself (font-src 'self'), at /excalidraw-assets/fonts.
const excalidrawFonts = realpathSync(
  join(dirname(fileURLToPath(import.meta.url)), 'node_modules/@excalidraw/excalidraw/dist/prod/fonts'),
)

function excalidrawAssets(): Plugin {
  return {
    name: 'excalidraw-assets',
    configureServer(server) {
      server.middlewares.use('/excalidraw-assets/fonts', (request, response, next) => {
        const path = normalize(join(excalidrawFonts, decodeURIComponent((request.url ?? '').split('?')[0])))
        if (!path.startsWith(excalidrawFonts + sep) || !existsSync(path) || !statSync(path).isFile()) return next()
        response.setHeader('content-type', 'font/woff2')
        createReadStream(path).pipe(response)
      })
    },
    // Excalidraw falls back to a public CDN for any font, which the policy
    // blocks and which would tell a third party who opens boards. Point the
    // fallback at the same copy.
    transform(code, id) {
      if (!id.includes('@excalidraw/excalidraw/dist/') || !code.includes('ASSETS_FALLBACK_URL')) return null
      const replaced = code.replace(
        /"ASSETS_FALLBACK_URL",`https:\/\/esm\.sh\/.*?\/dist\/prod\/`/,
        '"ASSETS_FALLBACK_URL",new URL("/excalidraw-assets/",window.location.origin).toString()',
      )
      if (replaced.includes('esm.sh')) throw new Error(`Excalidraw's font fallback changed shape in ${id}`)
      return { code: replaced, map: null }
    },
    writeBundle(options) {
      if (options.dir) cpSync(excalidrawFonts, join(options.dir, 'excalidraw-assets/fonts'), { recursive: true })
    },
  }
}

// Excalidraw and what it pulls in (diagram parsers, maths, locales) load only
// on a whiteboard. Their chunks go under assets/whiteboard/ so the web budget
// can hold them to their own limit; see scripts/web-budget-lib.mjs.
const appPackages = new Set(
  Object.keys(
    (JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'package.json'), 'utf8')) as {
      dependencies: Record<string, string>
    }).dependencies,
  ).filter((name) => name !== '@excalidraw/excalidraw'),
)

function packageOf(id: string) {
  const at = id.lastIndexOf('node_modules/')
  if (at < 0) return null
  const [first, second] = id.slice(at + 'node_modules/'.length).split('/')
  return first.startsWith('@') ? `${first}/${second}` : first
}

function isWhiteboardChunk(moduleIds: readonly string[]) {
  const ids = moduleIds.map((id) => id.split(sep).join('/')).filter((id) => !id.startsWith('\0'))
  if (ids.some((id) => id.includes('/src/whiteboards/BoardCanvas') || packageOf(id) === '@excalidraw/excalidraw')) {
    return true
  }
  return ids.length > 0 && ids.every((id) => {
    const name = packageOf(id)
    return name !== null && !appPackages.has(name)
  })
}

export default defineConfig({
  plugins: [react(), excalidrawAssets()],
  build: {
    rollupOptions: {
      output: {
        chunkFileNames: (chunk) =>
          isWhiteboardChunk(chunk.moduleIds) ? 'assets/whiteboard/[name]-[hash].js' : 'assets/[name]-[hash].js',
        assetFileNames: (asset) =>
          (asset.names ?? []).some((name) => name.startsWith('BoardCanvas'))
            ? 'assets/whiteboard/[name]-[hash][extname]'
            : 'assets/[name]-[hash][extname]',
      },
    },
  },
  server: {
    host: '127.0.0.1',
    port: 5173,
    proxy: {
      '/api': {
        target: apiTarget,
        ws: true,
      },
      '/health': apiTarget,
      // Single sign-on comes back here from WorkOS, and the server finishes it.
      '/auth/workos': apiTarget,
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
    // Playwright owns e2e/*.spec.ts; vitest would try to run them otherwise.
    exclude: [...configDefaults.exclude, 'e2e/**'],
    // The full-App tests take 4 to 5 seconds on an idle machine, so vitest's
    // 5 second default fails them whenever the CI host is busy.
    testTimeout: 20_000,
  },
})
