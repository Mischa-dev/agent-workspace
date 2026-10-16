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

  function loadPushWorker() {
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
    new Function('self', 'caches', 'fetch', workerSource)(self, {}, async () => new Response(''))
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
})
