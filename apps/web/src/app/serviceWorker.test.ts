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
