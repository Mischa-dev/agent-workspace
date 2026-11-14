import { afterEach, describe, expect, it, vi } from 'vitest'

import { apiUrl, origins, requestJson } from '../api/client'
import { uploadFile } from '../files/api'
import { load as loadClientConfig } from './clientConfig'

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

afterEach(() => {
  vi.unstubAllGlobals()
  Object.assign(origins, { api: undefined, live: undefined, csrf: undefined })
})

describe('client config (H1.41)', () => {
  it('keeps to its own origin when the server that served it answers', async () => {
    const fetchMock = vi.fn().mockResolvedValue(json({ apiOrigin: null, liveOrigin: null, version: '1' }))
    vi.stubGlobal('fetch', fetchMock)

    const config = await loadClientConfig()

    expect(config.apiOrigin).toBeNull()
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(apiUrl('/api/v1/projects')).toBe('/api/v1/projects')
    expect(origins.live).toBeUndefined()
  })

  it('keeps to its own origin with no answer at all', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('offline')))
    expect((await loadClientConfig()).apiOrigin).toBeNull()
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('<!doctype html>', { headers: { 'content-type': 'text/html' } })))
    expect((await loadClientConfig()).apiOrigin).toBeNull()
  })

  it('on a static host, follows the pointer and talks to the API across origins', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(json({ apiOrigin: 'https://api.example.com/' }))
      .mockResolvedValueOnce(json({ apiOrigin: 'https://api.example.com', liveOrigin: 'https://live.example.com' }))
      .mockResolvedValueOnce(json({ csrfToken: 'token-1' }))
      .mockResolvedValueOnce(json({ ok: true }))
    vi.stubGlobal('fetch', fetchMock)

    const config = await loadClientConfig()
    expect(config.apiOrigin).toBe('https://api.example.com')
    expect(fetchMock.mock.calls[1][0]).toBe('https://api.example.com/client-config')
    expect(origins.live).toBe('https://live.example.com')

    await requestJson('/api/v1/projects', { method: 'POST', json: { name: 'P' } })
    expect(fetchMock.mock.calls[2][0]).toBe('https://api.example.com/api/v1/auth/csrf')
    const [url, init] = fetchMock.mock.calls[3]
    expect(url).toBe('https://api.example.com/api/v1/projects')
    expect(init.credentials).toBe('include')
    expect(new Headers(init.headers).get('x-csrf-token')).toBe('token-1')
  })

  it('reads the current request token before each write', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(json({ apiOrigin: 'https://api.example.com' }))
      .mockResolvedValueOnce(json({ apiOrigin: 'https://api.example.com' }))
      .mockResolvedValueOnce(json({ csrfToken: 'first' }))
      .mockResolvedValueOnce(json({ ok: true }))
      .mockResolvedValueOnce(json({ csrfToken: 'after-sign-in' }))
      .mockResolvedValueOnce(json({ ok: true }))
    vi.stubGlobal('fetch', fetchMock)
    await loadClientConfig()

    await requestJson('/api/v1/projects', { method: 'POST', json: {} })
    await requestJson('/api/v1/projects', { method: 'POST', json: {} })
    expect(new Headers(fetchMock.mock.calls[3][1].headers).get('x-csrf-token')).toBe('first')
    expect(new Headers(fetchMock.mock.calls[5][1].headers).get('x-csrf-token')).toBe('after-sign-in')
  })

  it('sends uploads to the API origin with its request token', async () => {
    const sent: { url?: string; headers: Record<string, string>; credentials?: boolean } = { headers: {} }
    class FakeRequest {
      status = 201
      responseText = JSON.stringify({ id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b', name: 'a.txt' })
      withCredentials = false
      upload = {}
      onload?: () => void
      open(_method: string, url: string) {
        sent.url = url
      }
      setRequestHeader(name: string, value: string) {
        sent.headers[name] = value
      }
      getResponseHeader() {
        return null
      }
      send() {
        sent.credentials = this.withCredentials
        this.onload?.()
      }
    }
    vi.stubGlobal('XMLHttpRequest', FakeRequest)
    Object.assign(origins, { api: 'https://api.example.com', csrf: async () => 'upload-token' })

    await uploadFile({ kind: 'conversation', conversationId: 'c1' })(
      new File(['x'], 'a.txt', { type: 'text/plain' }),
      () => {},
      new AbortController().signal,
    )
    expect(sent.url).toBe('https://api.example.com/api/v1/conversations/c1/files?name=a.txt')
    expect(sent.headers['x-csrf-token']).toBe('upload-token')
    expect(sent.credentials).toBe(true)
  })
})
