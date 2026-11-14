import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../errors'
import { callOp } from './client'

// The names and inputs are cast: the test is about the call, whichever
// operations the registry holds.
const call = callOp as unknown as (
  name: string,
  input: unknown,
  options?: { workspaceId?: string; requestId?: string; preview?: boolean },
) => Promise<unknown>

function answer(status: number, body: unknown) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('callOp', () => {
  it('posts the input to the workspace route with the request id and returns the value', async () => {
    const fetchMock = vi.fn().mockResolvedValue(answer(200, { outcome: 'ok', value: { id: 'd-1' }, replayed: false }))
    vi.stubGlobal('fetch', fetchMock)

    const value = await call('decision_record', { title: 'Ship it' }, { workspaceId: 'acme team', requestId: 'r-1' })

    expect(value).toEqual({ id: 'd-1' })
    const [path, init] = fetchMock.mock.calls[0]
    expect(path).toBe('/api/v2/w/acme%20team/ops/decision_record')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body)).toEqual({ title: 'Ship it' })
    expect(new Headers(init.headers).get('idempotency-key')).toBe('r-1')
  })

  it('uses the route without a workspace when none is named', async () => {
    const fetchMock = vi.fn().mockResolvedValue(answer(200, { outcome: 'ok', value: null }))
    vi.stubGlobal('fetch', fetchMock)

    await call('later_list', {}, { preview: true })

    expect(fetchMock.mock.calls[0][0]).toBe('/api/v2/ops/later_list?preview=true')
  })

  it('throws a conflict envelope as an ApiError with its details', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        answer(409, {
          outcome: 'conflict',
          code: 'version',
          message: 'Someone changed this first.',
          retryCategory: 'reread',
          details: { currentVersion: 8 },
        }),
      ),
    )

    const error = await call('task_update', {}, { workspaceId: 'w' }).catch((caught: unknown) => caught)

    expect(error).toBeInstanceOf(ApiError)
    expect(error).toMatchObject({
      status: 409,
      code: 'conflict',
      message: 'Someone changed this first.',
      details: { currentVersion: 8, reason: 'version' },
    })
  })
})
