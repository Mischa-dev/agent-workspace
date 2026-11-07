import { cpSync, createReadStream, existsSync, readdirSync, readFileSync, realpathSync, statSync } from 'node:fs'
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

// AWD-75: the module manifests (`modules/*.json`, which the server reads too)
// as small build-time projections, so each part of the app bundles only the
// ownership it asks for: `virtual:modules/slots/<kind>` is that kind's
// [pattern, module] claims, in module and then manifest order;
// `virtual:modules/switchable` the modules that can be off, and
// `virtual:modules/names` each module's name. One module per kind, so the
// first page load doesn't carry the palette's or Home's claims.
const SLOT_KINDS = ['pages', 'parts', 'sidebar', 'home', 'settings', 'palette', 'shortcuts', 'slash', 'setup'] as const
const manifestsDir = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'modules')

type Manifest = { name?: string; switchable?: boolean; slots?: Record<string, string[] | undefined> }

function readManifests(): Array<{ id: string; file: string; manifest: Manifest }> {
  return readdirSync(manifestsDir)
    .filter((file) => file.endsWith('.json') && file !== 'presets.json')
    .sort()
    .map((file) => ({
      id: file.slice(0, -'.json'.length),
      file: join(manifestsDir, file),
      manifest: JSON.parse(readFileSync(join(manifestsDir, file), 'utf8')) as Manifest,
    }))
}

function moduleProjections(): Plugin {
  const prefix = 'virtual:modules/'
  return {
    name: 'module-projections',
    resolveId: (id) => (id.startsWith(prefix) ? `\0${id}` : null),
    load(id) {
      if (!id.startsWith(`\0${prefix}`)) return null
      const what = id.slice(prefix.length + 1)
      const manifests = readManifests()
      for (const { file } of manifests) this.addWatchFile(file)
      if (what === 'switchable') {
        return `export default ${JSON.stringify(manifests.filter(({ manifest }) => manifest.switchable === true).map(({ id }) => id))}`
      }
      if (what === 'names') {
        return `export default ${JSON.stringify(Object.fromEntries(manifests.map(({ id, manifest }) => [id, manifest.name ?? id])))}`
      }
      const kind = what.startsWith('slots/') ? what.slice('slots/'.length) : ''
      if (!(SLOT_KINDS as readonly string[]).includes(kind)) this.error(`unknown module projection ${id}`)
      // Each module's patterns once under its id, in manifest order, spread
      // back into the same [pattern, module] pairs in the same order: the
      // first page carries the page and shortcut claims, so this keeps them
      // short.
      const grouped = manifests
        .map(({ id, manifest }) => [id, manifest.slots?.[kind] ?? []] as const)
        .filter(([, patterns]) => patterns.length > 0)
      return `export default ${JSON.stringify(grouped)}.flatMap(([m, ps]) => ps.map((p) => [p, m]))`
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
  plugins: [react(), excalidrawAssets(), moduleProjections()],
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
