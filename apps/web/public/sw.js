// Agent Workspace service worker. It makes the app installable and lets it open
// offline from the last app shell it loaded. It stores only the built app
// (index.html and the hashed files under /assets/), never API responses, so no
// conversation or account data is kept by it. Everything else goes straight to
// the network: API calls, sign-in callbacks, OAuth and MCP pages, health checks.
// It also shows push notifications (the server sends only the notification's
// title, preview and in-app links) and opens the app when one is tapped. An
// approval request has Approve and Deny buttons, and a question or mention a
// Reply button; those send the push's own short-lived action token, never
// the session cookie, and the server checks the person's access when they act.
// Files another app shares (Share to Workspace) arrive as a POST to /share;
// they wait in a cache of their own only until the share page picks them up,
// which deletes them.

const SHELL_CACHE = 'aw-shell-v1'
const ASSET_CACHE = 'aw-assets-v1'
const SHARE_CACHE = 'aw-share-v1'
const MAX_SHARED_FILES = 10
const SHELL_URL = '/index.html'
const MAX_ASSETS = 120
// Client-side routes that the server answers with index.html.
const APP_ROUTE = /^\/(?:|home|inbox|my-work|runs|whiteboards|messages|projects|settings|search|share|new-task|sign-in|join)(?:\/|$)/

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
            .filter((name) => name.startsWith('aw-') && ![SHELL_CACHE, ASSET_CACHE, SHARE_CACHE].includes(name))
            .map((name) => caches.delete(name)),
        ),
      )
      .then(() => self.clients.claim()),
  )
})

self.addEventListener('fetch', (event) => {
  const request = event.request
  const url = new URL(request.url)
  if (url.origin !== self.location.origin) return
  if (request.method === 'POST' && url.pathname === '/share') {
    event.respondWith(receiveShare(request))
    return
  }
  if (request.method !== 'GET') return
  if (request.mode === 'navigate') {
    if (APP_ROUTE.test(url.pathname)) event.respondWith(openApp(event))
    return
  }
  if (url.pathname.startsWith('/assets/')) event.respondWith(asset(event))
})

// Push: {id, kind, title, body, place, url, tag, actions, actionToken}. Every
// push shows a notification, as browsers require; anything malformed shows a
// plain one.
self.addEventListener('push', (event) => {
  event.waitUntil(showPush(event))
})

const TOKEN_ACTIONS = ['approve', 'deny', 'reply']

self.addEventListener('notificationclick', (event) => {
  event.notification.close()
  const data = event.notification.data || {}
  if (TOKEN_ACTIONS.includes(event.action) && data.token) {
    const reply = typeof event.reply === 'string' ? event.reply.trim() : ''
    // Without an inline text field (most phones), Reply opens the composer.
    if (event.action === 'reply' && !reply) {
      event.waitUntil(focusApp(composePath(data.url || '/home')))
      return
    }
    event.waitUntil(act(data, event.action, reply))
    return
  }
  const target = (event.action === 'launch' && data.launch) || data.url || '/home'
  event.waitUntil(focusApp(target))
})

const DONE = { approve: 'Approved', deny: 'Denied', reply: 'Reply sent' }
const FAILED = { approve: 'Not approved', deny: 'Not denied', reply: 'Reply not sent' }

// Answer from the notification, then say what happened in a quiet one that
// replaces it. A failure says why and opens the app where it can be done.
async function act(data, action, reply) {
  const url = data.url || '/home'
  let message = null
  try {
    const response = await fetch('/api/v1/notification-actions', {
      method: 'POST',
      credentials: 'omit',
      headers: { accept: 'application/json', 'content-type': 'application/json' },
      body: JSON.stringify(action === 'reply' ? { token: data.token, action, text: reply } : { token: data.token, action }),
    })
    if (response.ok) {
      const result = await response.json().catch(() => ({}))
      const done = result && result.outcome === 'answered' ? 'Answer sent' : DONE[action]
      return self.registration.showNotification(done, {
        body: data.title || undefined,
        tag: data.tag || undefined,
        silent: true,
        icon: '/icons/icon-192.png',
        badge: '/icons/icon-192.png',
        data: { url: inAppPath(result && result.href) || url },
      })
    }
    const body = await response.json().catch(() => null)
    message = body && body.error && typeof body.error.message === 'string' ? body.error.message : null
    if (response.status === 404) message = 'It may have been answered already, or it expired.'
  } catch {
    message = 'You seem to be offline.'
  }
  const kept = action === 'reply' && reply ? ` Your reply: “${reply.slice(0, 160)}”` : ''
  return self.registration.showNotification(FAILED[action] || 'Not done', {
    body: `${message || 'Something went wrong.'} Tap to open it in the app.${kept}`,
    tag: data.tag || undefined,
    icon: '/icons/icon-192.png',
    badge: '/icons/icon-192.png',
    data: { url: action === 'reply' ? composePath(url) : url },
  })
}

// The same page with its composer focused.
function composePath(path) {
  const url = new URL(path, self.location.origin)
  url.searchParams.set('compose', '1')
  return url.pathname + url.search + url.hash
}

function inAppPath(value) {
  return typeof value === 'string' && value.startsWith('/') && !value.startsWith('//') ? value : null
}

function pushPayload(event) {
  try {
    const payload = event.data ? event.data.json() : null
    return payload && typeof payload === 'object' ? payload : {}
  } catch {
    return {}
  }
}

function showPush(event) {
  const payload = pushPayload(event)
  const text = (value) => (typeof value === 'string' && value ? value : null)
  const listed = Array.isArray(payload.actions) ? payload.actions.filter((action) => action && typeof action === 'object') : []
  const token = text(payload.actionToken)
  const actions = []
  if (token) {
    for (const item of listed) {
      if (!TOKEN_ACTIONS.includes(item.action)) continue
      const action = { action: item.action, title: text(item.title) || DONE[item.action] }
      if (item.type === 'text') {
        action.type = 'text'
        action.placeholder = text(item.placeholder) || 'Reply'
      }
      actions.push(action)
    }
  }
  const launch = listed.find((action) => action.action === 'launch' && inAppPath(action.url))
  if (actions.length < 2) actions.push({ action: 'open', title: 'Open' })
  if (launch && actions.length < 2) actions.push({ action: 'launch', title: text(launch.title) || 'Launch' })
  const tag = text(payload.tag)
  const title = text(payload.title) || 'Agent Workspace'
  return self.registration.showNotification(title, {
    body: [text(payload.place), text(payload.body)].filter(Boolean).join(' · ') || undefined,
    tag: tag || undefined,
    renotify: Boolean(tag),
    icon: '/icons/icon-192.png',
    badge: '/icons/icon-192.png',
    timestamp: Date.now(),
    actions,
    data: {
      id: text(payload.id),
      url: inAppPath(payload.url) || '/home',
      launch: launch ? inAppPath(launch.url) : null,
      token: actions.some((action) => TOKEN_ACTIONS.includes(action.action)) ? token : null,
      tag,
      title,
    },
  })
}

// A share with files: keep the files for the share page (it reads and deletes
// them), and open the page with the title, text and link in its address, as a
// share without files arrives.
async function receiveShare(request) {
  const target = new URL('/share', self.location.origin)
  try {
    const form = await request.formData()
    for (const name of ['title', 'text', 'url']) {
      const value = form.get(name)
      if (typeof value === 'string' && value) target.searchParams.set(name, value.slice(0, 20000))
    }
    const files = form.getAll('files').filter((file) => typeof file !== 'string').slice(0, MAX_SHARED_FILES)
    if (files.length) {
      const cache = await caches.open(SHARE_CACHE)
      for (const old of await cache.keys()) await cache.delete(old)
      await Promise.all(
        files.map((file, index) =>
          cache.put(
            `/__shared/${index}`,
            new Response(file, {
              headers: { 'content-type': file.type || 'application/octet-stream', 'x-file-name': encodeURIComponent(file.name || `file-${index + 1}`) },
            }),
          ),
        ),
      )
      target.searchParams.set('files', String(files.length))
    }
  } catch {
    // An unreadable share still opens the page, empty.
  }
  return Response.redirect(target.href, 303)
}

// Focus an open window of the app and take it there, else open one.
async function focusApp(path) {
  const url = new URL(path, self.location.origin).href
  const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true })
  const open = windows.find((client) => new URL(client.url).origin === self.location.origin)
  if (!open) return self.clients.openWindow(url)
  await open.focus()
  if (typeof open.navigate === 'function') {
    return open.navigate(url).catch(() => self.clients.openWindow(url))
  }
  return undefined
}

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
