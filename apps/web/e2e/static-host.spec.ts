import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { createServer, type Server } from 'node:http'
import path from 'node:path'

import { expect, test } from '@playwright/test'

import { outputRoot, ports, seeded, seededValue, serverOrigin, staticOrigin, webRoot } from './environment'

// W1.38 (R24.1): the built app, served as plain files from one origin, signs
// in and works against the API on another. The static host knows nothing of
// the app: it serves files, index.html for any other path, and a
// client-config file pointing at the API (docs/self-host/static-web-app.md).
// CI passes the build it just made (E2E_STATIC_DIST); otherwise this builds one.

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

let host: Server

test.beforeAll(async () => {
  test.setTimeout(240_000)
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
  host = createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url ?? '/', staticOrigin).pathname)
    if (pathname === '/client-config') {
      response.writeHead(200, { 'content-type': 'application/json' }).end(config)
      return
    }
    const file = path.join(dist, path.normalize(pathname).replace(/^([/\\])+/, ''))
    const found = file.startsWith(dist) && existsSync(file) && statSync(file).isFile() ? file : path.join(dist, 'index.html')
    response.writeHead(200, { 'content-type': TYPES[path.extname(found)] ?? 'application/octet-stream' }).end(readFileSync(found))
  })
  await new Promise<void>((resolve) => host.listen(ports.static, '127.0.0.1', resolve))
})

test.afterAll(async () => {
  await new Promise((resolve) => host?.close(resolve))
})

test('the built app from a static host signs in and talks to the API on another origin', async ({ page }) => {
  const toApi: string[] = []
  page.on('request', (request) => {
    if (request.url().startsWith(`${serverOrigin}/api/`)) toApi.push(request.url())
  })
  await page.goto(`${staticOrigin}/sign-in`)
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()
  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()

  await expect(page).toHaveURL(new RegExp(`^${staticOrigin}/(?:w/[^/]+/)?home$`))
  await expect(
    page.getByRole('navigation', { name: 'Spaces' }).getByRole('link', { name: seeded.projectName }),
  ).toBeVisible()
  // Every call went to the API's origin, not the static host.
  expect(toApi.some((url) => url.includes('/api/v1/auth/login'))).toBe(true)
})
