import { describe, expect, it, vi } from 'vitest'

import workerSource from '../../public/sw.js?raw'

type FetchEvent = {
  request: { url: string; method: string; mode: string }
  respondWith: ReturnType<typeof vi.fn>
  waitUntil: ReturnType<typeof vi.fn>
}

function loadWorker() {
  const listeners: Record<string, (event: FetchEvent) => void> = {}
  const self = {
    location: { origin: 'https://workspace.example' },
    addEventListener: (type: string, listener: (event: FetchEvent) => void) => {
      listeners[type] = listener
    },
  }
  const cache = { put: async () => undefined, keys: async () => [], delete: async () => true }
  const caches = { match: async () => undefined, open: async () => cache }
  const fetch = async () => new Response('<!doctype html>', { headers: { 'content-type': 'text/html' } })
  new Function('self', 'caches', 'fetch', workerSource)(self, caches, fetch)
  return (url: string, { method = 'GET', mode = 'cors' } = {}) => {
    const event: FetchEvent = { request: { url, method, mode }, respondWith: vi.fn(), waitUntil: vi.fn() }
    listeners.fetch?.(event)
    return event.respondWith.mock.calls.length > 0
  }
}

describe('service worker', () => {
  const handles = loadWorker()
  const origin = 'https://workspace.example'

  it('never answers API, sign-in, OAuth or other server requests', () => {
    expect(handles(`${origin}/api/v1/direct/overview`)).toBe(false)
    expect(handles(`${origin}/api/v1/realtime`)).toBe(false)
    expect(handles(`${origin}/api/v1/auth/login`, { method: 'POST' })).toBe(false)
    expect(handles(`${origin}/api/v1/auth/workos/start`, { mode: 'navigate' })).toBe(false)
    expect(handles(`${origin}/auth/workos/callback?code=abc`, { mode: 'navigate' })).toBe(false)
    expect(handles(`${origin}/oauth/authorize?client_id=abc`, { mode: 'navigate' })).toBe(false)
    expect(handles(`${origin}/connect/confirm`, { mode: 'navigate' })).toBe(false)
    expect(handles(`${origin}/mcp`, { mode: 'navigate' })).toBe(false)
    expect(handles(`${origin}/health/ready`)).toBe(false)
    expect(handles(`${origin}/homework`, { mode: 'navigate' })).toBe(false)
    expect(handles('https://elsewhere.example/assets/app.js')).toBe(false)
  })

  it('answers app pages and built assets', () => {
    expect(handles(`${origin}/`, { mode: 'navigate' })).toBe(true)
    expect(handles(`${origin}/home`, { mode: 'navigate' })).toBe(true)
    expect(handles(`${origin}/projects/p1/threads/t1`, { mode: 'navigate' })).toBe(true)
    expect(handles(`${origin}/join?token=abc`, { mode: 'navigate' })).toBe(true)
    expect(handles(`${origin}/assets/index-Cx1.js`)).toBe(true)
  })
})

type ShownOptions = { actions: { title: string }[]; data: { url: string; launch: string | null } }

describe('service worker push', () => {
  const origin = 'https://workspace.example'

  function loadPushWorker(fetch: (input: string, init?: RequestInit) => Promise<Response> = async () => new Response('')) {
    const listeners: Record<string, (event: unknown) => void> = {}
    const showNotification = vi.fn(async () => undefined)
    const openWindow = vi.fn(async () => null)
    const self = {
      location: { origin },
      addEventListener: (type: string, listener: (event: unknown) => void) => {
        listeners[type] = listener
      },
      registration: { showNotification },
      clients: { matchAll: async () => [], openWindow },
    }
    new Function('self', 'caches', 'fetch', workerSource)(self, {}, fetch)
    const dispatch = async (type: string, event: Record<string, unknown>) => {
      const waits: Promise<unknown>[] = []
      listeners[type]?.({ ...event, waitUntil: (promise: Promise<unknown>) => waits.push(promise) })
      await Promise.all(waits)
    }
    return { dispatch, showNotification, openWindow }
  }

  it('shows a capacity alert with its Launch action and opens the ready tasks', async () => {
    const worker = loadPushWorker()
    const payload = {
      id: 'n1',
      kind: 'capacity_reset_soon',
      title: 'Codex weekly limit resets in 2 h · 38% unused',
      body: '4 ready tasks across 2 projects can use it.',
      place: 'Usage',
      url: '/settings/account/usage?account=a1',
      tag: 'a1',
      actions: [
        { action: 'open', title: 'Open' },
        { action: 'launch', title: 'Launch', url: '/settings/account/usage?account=a1&ready=1#ready-tasks' },
      ],
    }
    await worker.dispatch('push', { data: { json: () => payload } })
    expect(worker.showNotification).toHaveBeenCalledTimes(1)
    const [title, options] = worker.showNotification.mock.calls[0] as unknown as [string, ShownOptions]
    expect(title).toBe('Codex weekly limit resets in 2 h · 38% unused')
    expect(options.actions.map((action) => action.title)).toEqual(['Open', 'Launch'])

    const close = vi.fn()
    await worker.dispatch('notificationclick', { action: 'launch', notification: { close, data: options.data } })
    expect(close).toHaveBeenCalled()
    expect(worker.openWindow).toHaveBeenCalledWith(`${origin}/settings/account/usage?account=a1&ready=1#ready-tasks`)
  })

  it('never follows a link off the app and still shows a malformed push', async () => {
    const worker = loadPushWorker()
    await worker.dispatch('push', {
      data: { json: () => ({ title: 'Hi', url: 'https://evil.example/', actions: [{ action: 'launch', url: '//evil.example' }] }) },
    })
    const [, options] = worker.showNotification.mock.calls[0] as unknown as [string, ShownOptions]
    expect(options.data.url).toBe('/home')
    expect(options.actions).toHaveLength(1)
    await worker.dispatch('push', { data: { json: () => { throw new Error('bad') } } })
    expect(worker.showNotification).toHaveBeenLastCalledWith('Agent Workspace', expect.any(Object))
  })

  const approval = {
    id: 'n2',
    kind: 'approval_requested',
    title: 'A run in #general needs your approval',
    body: 'Run npm install',
    url: '/projects/p1/threads/t1',
    tag: 'r1',
    actionToken: 'tok_123',
    actions: [
      { action: 'approve', title: 'Approve' },
      { action: 'deny', title: 'Deny' },
      { action: 'open', title: 'Open' },
    ],
  }

  it('approves from the notification with the push token, not the session', async () => {
    const fetch = vi.fn(async () => Response.json({ outcome: 'approved', href: '/projects/p1/threads/t1' }))
    const worker = loadPushWorker(fetch)
    await worker.dispatch('push', { data: { json: () => approval } })
    const [, options] = worker.showNotification.mock.calls[0] as unknown as [string, ShownOptions]
    expect(options.actions.map((action) => action.title)).toEqual(['Approve', 'Deny'])

    await worker.dispatch('notificationclick', { action: 'approve', notification: { close: vi.fn(), data: options.data } })
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('/api/v1/notification-actions')
    expect(init.credentials).toBe('omit')
    expect(JSON.parse(String(init.body))).toEqual({ token: 'tok_123', action: 'approve' })
    expect(worker.showNotification).toHaveBeenLastCalledWith('Approved', expect.objectContaining({ tag: 'r1' }))
    expect(worker.openWindow).not.toHaveBeenCalled()
  })

  it('says when an action fails and opens the app on tap', async () => {
    const fetch = vi.fn(async () => Response.json({ error: { message: 'Not found.' } }, { status: 404 }))
    const worker = loadPushWorker(fetch)
    await worker.dispatch('push', { data: { json: () => approval } })
    const [, options] = worker.showNotification.mock.calls[0] as unknown as [string, ShownOptions]
    await worker.dispatch('notificationclick', { action: 'deny', notification: { close: vi.fn(), data: options.data } })
    const [title, failed] = worker.showNotification.mock.calls.at(-1) as unknown as [string, ShownOptions & { body: string }]
    expect(title).toBe('Not denied')
    expect(failed.body).toContain('answered already')
    expect(failed.data.url).toBe('/projects/p1/threads/t1')
  })

  it('opens the composer for Reply where there is no inline text field', async () => {
    const fetch = vi.fn(async () => Response.json({ outcome: 'replied', href: '/x' }))
    const worker = loadPushWorker(fetch)
    const mention = { ...approval, kind: 'mention', url: '/projects/p1/channels/c1#message-m1', actions: [{ action: 'reply', title: 'Reply', type: 'text' }] }
    await worker.dispatch('push', { data: { json: () => mention } })
    const [, options] = worker.showNotification.mock.calls[0] as unknown as [string, ShownOptions & { actions: { type?: string }[] }]
    expect(options.actions.map((action) => action.title)).toEqual(['Reply', 'Open'])
    expect(options.actions[0].type).toBe('text')

    await worker.dispatch('notificationclick', { action: 'reply', notification: { close: vi.fn(), data: options.data } })
    expect(fetch).not.toHaveBeenCalled()
    expect(worker.openWindow).toHaveBeenCalledWith(`${origin}/projects/p1/channels/c1?compose=1#message-m1`)

    await worker.dispatch('notificationclick', { action: 'reply', reply: ' On it ', notification: { close: vi.fn(), data: options.data } })
    const [, init] = fetch.mock.calls[0] as unknown as [string, RequestInit]
    expect(JSON.parse(String(init.body))).toEqual({ token: 'tok_123', action: 'reply', text: 'On it' })
  })

  it('shows no token buttons without a token', async () => {
    const worker = loadPushWorker()
    await worker.dispatch('push', { data: { json: () => ({ ...approval, actionToken: undefined }) } })
    const [, options] = worker.showNotification.mock.calls[0] as unknown as [string, ShownOptions]
    expect(options.actions.map((action) => action.title)).toEqual(['Open'])
  })
})
