import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { createServer } from 'node:http'
import path from 'node:path'

import { outputRoot, ports, serverOrigin, staticOrigin, webRoot } from '../environment'

// The built app served as plain files (W1.38): index.html for any other path,
// and a client-config file pointing at the API on its own origin
// (docs/self-host/static-web-app.md). CI passes the build it just made
// (E2E_STATIC_DIST); otherwise this builds one. Used by the static-host spec
// and the speed test, which needs the real build.

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.woff2': 'font/woff2',
  '.webmanifest': 'application/manifest+json',
}

/** Builds the app if no build was passed in, serves it at `staticOrigin`, and returns how to stop. */
export async function startStaticHost(): Promise<() => Promise<void>> {
  let dist = process.env.E2E_STATIC_DIST ? path.resolve(webRoot, process.env.E2E_STATIC_DIST) : ''
  if (!dist) {
    dist = path.join(outputRoot, 'static-dist')
    const build = spawnSync(process.execPath, ['./node_modules/vite/bin/vite.js', 'build', '--outDir', dist, '--emptyOutDir'], {
      cwd: webRoot,
      encoding: 'utf8',
      timeout: 220_000,
    })
    if (build.status !== 0) throw new Error(`vite build failed:\n${build.stdout}\n${build.stderr}`)
  }
  const config = JSON.stringify({ apiOrigin: serverOrigin })
  const host = createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url ?? '/', staticOrigin).pathname)
    if (pathname === '/client-config') {
      response.writeHead(200, { 'content-type': 'application/json' }).end(config)
      return
    }
    const file = path.join(dist, path.normalize(pathname).replace(/^([/\\])+/, ''))
    const found = file.startsWith(dist) && existsSync(file) && statSync(file).isFile() ? file : path.join(dist, 'index.html')
    // Hashed assets are kept by the browser, as a real host has them kept.
    const cache = found.includes(`${path.sep}assets${path.sep}`) ? 'public, max-age=31536000, immutable' : 'no-cache'
    response
      .writeHead(200, { 'content-type': TYPES[path.extname(found)] ?? 'application/octet-stream', 'cache-control': cache })
      .end(readFileSync(found))
  })
  await new Promise<void>((resolve) => host.listen(ports.static, '127.0.0.1', resolve))
  return () => new Promise<void>((resolve) => host.close(() => resolve()))
}
