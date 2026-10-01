// Agent Workspace service worker. It makes the app installable and lets it open
// offline from the last app shell it loaded. It stores only the built app
// (index.html and the hashed files under /assets/), never API responses, so no
// conversation or account data is kept by it. Everything else goes straight to
// the network: API calls, sign-in callbacks, OAuth and MCP pages, health checks.

const SHELL_CACHE = 'aw-shell-v1'
const ASSET_CACHE = 'aw-assets-v1'
const SHELL_URL = '/index.html'
const MAX_ASSETS = 120
// Client-side routes that the server answers with index.html.
const APP_ROUTE = /^\/(?:|home|messages|projects|settings|search|sign-in|join)(?:\/|$)/

self.addEventListener('install', (event) => {
  event.waitUntil(cacheShell().catch(() => undefined).then(() => self.skipWaiting()))
})

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(
          names
            .filter((name) => name.startsWith('aw-') && name !== SHELL_CACHE && name !== ASSET_CACHE)
            .map((name) => caches.delete(name)),
        ),
      )
      .then(() => self.clients.claim()),
  )
})

self.addEventListener('fetch', (event) => {
  const request = event.request
  if (request.method !== 'GET') return
  const url = new URL(request.url)
  if (url.origin !== self.location.origin) return
  if (request.mode === 'navigate') {
    if (APP_ROUTE.test(url.pathname)) event.respondWith(openApp(event))
    return
  }
  if (url.pathname.startsWith('/assets/')) event.respondWith(asset(event))
})

// Network first, so a new release shows up at once; the saved shell when offline.
async function openApp(event) {
  try {
    const response = await fetch(event.request)
    if (response.ok && (response.headers.get('content-type') || '').startsWith('text/html')) {
      const copy = response.clone()
      event.waitUntil(save(SHELL_CACHE, SHELL_URL, copy))
    }
    return response
  } catch (error) {
    const shell = await caches.match(SHELL_URL, { cacheName: SHELL_CACHE }).catch(() => undefined)
    if (shell) return shell
    throw error
  }
}

// Asset names carry a content hash, so a saved copy never goes stale.
async function asset(event) {
  const cached = await caches.match(event.request, { cacheName: ASSET_CACHE }).catch(() => undefined)
  if (cached) return cached
  const response = await fetch(event.request)
  if (response.ok) event.waitUntil(save(ASSET_CACHE, event.request, response.clone()).then(trimAssets))
  return response
}

async function cacheShell() {
  const response = await fetch(SHELL_URL, { cache: 'no-cache' })
  if (!response.ok) return
  const html = await response.clone().text()
  await save(SHELL_CACHE, SHELL_URL, response)
  const assets = [...new Set(html.match(/\/assets\/[\w.-]+/g) || [])]
  const cache = await caches.open(ASSET_CACHE)
  await Promise.all(assets.map((path) => cache.add(path).catch(() => undefined)))
}

async function save(cacheName, key, response) {
  try {
    const cache = await caches.open(cacheName)
    await cache.put(key, response)
  } catch {
    // Storage can be full or unavailable (private browsing); the app still works online.
  }
}

async function trimAssets() {
  try {
    const cache = await caches.open(ASSET_CACHE)
    const keys = await cache.keys()
    await Promise.all(keys.slice(0, Math.max(0, keys.length - MAX_ASSETS)).map((key) => cache.delete(key)))
  } catch {
    // Trimming is housekeeping; skip it when storage is unavailable.
  }
}
