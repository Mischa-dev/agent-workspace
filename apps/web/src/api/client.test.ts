import { afterEach, describe, expect, it, vi } from 'vitest'

import { requestJson } from './client'

afterEach(() => {
  vi.unstubAllGlobals()
  document.cookie = 'agent_workspace_csrf=; Max-Age=0; Path=/'
})

describe('API client security boundary', () => {
  it('uses same-origin credentials and adds CSRF only to unsafe methods', async () => {
    document.cookie = 'agent_workspace_csrf=csrf-value; Path=/'
    const fetchMock = vi
      .fn()
      .mockImplementation(async () =>
        new Response(JSON.stringify({ ok: true }), {
          headers: { 'content-type': 'application/json' },
        }),
      )
    vi.stubGlobal('fetch', fetchMock)

    await requestJson('/api/v1/projects')
    await requestJson('/api/v1/projects', {
      method: 'POST',
      idempotencyKey: 'idempotent-1',
      json: { name: 'Project' },
    })

    expect(fetchMock.mock.calls[0][1].credentials).toBe('same-origin')
    expect(
      new Headers(fetchMock.mock.calls[0][1].headers).has('x-csrf-token'),
    ).toBe(false)
    const mutationHeaders = new Headers(fetchMock.mock.calls[1][1].headers)
    expect(mutationHeaders.get('x-csrf-token')).toBe('csrf-value')
    expect(mutationHeaders.get('idempotency-key')).toBe('idempotent-1')
  })

  it('returns stable structured errors and request IDs', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            error: { code: 'project_archived', message: 'Project archived.' },
          }),
          {
            status: 409,
            headers: {
              'content-type': 'application/json',
              'x-request-id': 'request-1',
            },
          },
        ),
      ),
    )

    await expect(requestJson('/api/v1/projects/id')).rejects.toMatchObject({
        code: 'project_archived',
        message: 'Project archived.',
        requestId: 'request-1',
        status: 409,
    })
  })

  it('ignores a malformed CSRF cookie instead of crashing before fetch', async () => {
    document.cookie = 'agent_workspace_csrf=%not-a-valid-escape; Path=/'
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), {
        headers: { 'content-type': 'application/json' },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await expect(
      requestJson('/api/v1/projects', { method: 'POST', json: {} }),
    ).resolves.toEqual({ ok: true })

    const headers = new Headers(fetchMock.mock.calls[0][1].headers)
    expect(headers.has('x-csrf-token')).toBe(false)
  })

  it.each([
    ['non-JSON success', new Response('<html>proxy error</html>')],
    [
      'malformed JSON success',
      new Response('{broken', {
        headers: { 'content-type': 'application/json' },
      }),
    ],
  ])('rejects a %s response instead of returning a mistyped null', async (_, response) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response))

    await expect(requestJson('/api/v1/projects')).rejects.toMatchObject({
      code: 'invalid_response',
      message: 'The server returned an invalid response.',
      status: 200,
    })
  })
})
