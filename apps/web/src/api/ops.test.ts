import { afterEach, describe, expect, it, vi } from 'vitest'

import { describeError } from '../ui/errorText'
import { ApiError } from './errors'
import type { OpErrorOutcome } from './generated/operations'
import { callOp, newRequestId, OUTCOME_SENTENCES, opKey, opQuery } from './ops'

// The names and inputs are cast: the test is about the call, whichever
// operations the registry holds.
const call = callOp as unknown as (name: string, input: unknown, options?: Record<string, unknown>) => Promise<unknown>

function answer(status: number, body: unknown) {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('newRequestId', () => {
  it('is a UUIDv7 that sorts by time', () => {
    const early = newRequestId(1_700_000_000_000)
    const late = newRequestId(1_700_000_000_001)
    expect(early).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    expect(early < late).toBe(true)
  })
})

describe('callOp', () => {
  it('sends the same request id when it retries a write', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(answer(503, { outcome: 'temporarily_unavailable', message: 'Busy', retryCategory: 'retry' }))
      .mockResolvedValueOnce(answer(200, { outcome: 'ok', value: { id: 'd-1' }, replayed: false }))
    vi.stubGlobal('fetch', fetchMock)

    const value = await call('decision_record', { title: 'Ship it' }, { workspaceId: 'acme' })

    expect(value).toEqual({ id: 'd-1' })
    expect(fetchMock).toHaveBeenCalledTimes(2)
    const keys = fetchMock.mock.calls.map(([, init]) => new Headers(init.headers).get('idempotency-key'))
    expect(keys[0]).toMatch(/^[0-9a-f-]{36}$/)
    expect(keys[1]).toBe(keys[0])
  })

  it('sends the expected version in the input', async () => {
    const fetchMock = vi.fn().mockResolvedValue(answer(200, { outcome: 'ok', value: {} }))
    vi.stubGlobal('fetch', fetchMock)

    await call('decision_record', { title: 'Ship it' }, { expectedVersion: 4, requestId: 'r-1' })

    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({ title: 'Ship it', expectedVersion: 4 })
  })

  it('does not retry a refusal', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(answer(409, { outcome: 'conflict', message: 'Changed', retryCategory: 'reread', code: 'version' }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(call('decision_record', { title: 'x' }, { retries: 3 })).rejects.toBeInstanceOf(ApiError)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})

describe('error sentences', () => {
  const outcomes = Object.keys(OUTCOME_SENTENCES) as OpErrorOutcome[]

  it.each(outcomes)('turns %s into its sentence', async (outcome) => {
    const status = outcome === 'conflict' ? 409 : outcome === 'not_found' ? 404 : 422
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(answer(status, { outcome, message: '', retryCategory: 'fix_input' })))
    const error = await call('decision_record', { title: 'x' }, { retries: 0 }).catch((caught: unknown) => caught)
    expect(error).toBeInstanceOf(ApiError)
    const described = describeError(error)
    expect(described.message).toBe(OUTCOME_SENTENCES[outcome])
    expect(described.stale).toBe(outcome === 'conflict' || outcome === 'not_found')
  })

  it('keeps the server’s message where it says what to fix', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        answer(422, { outcome: 'invalid_request', field: 'title', message: 'Give it a title.', retryCategory: 'fix_input' }),
      ),
    )
    const error = await call('decision_record', { title: '' }).catch((caught: unknown) => caught)
    expect(describeError(error).message).toBe('Give it a title.')
  })

  it('leaves errors from older routes as they were', () => {
    const error = new ApiError({ status: 403, code: 'permission_denied', message: 'No.' })
    expect(describeError(error).message).toBe('You don’t have permission to do that here.')
  })
})

describe('opKey and opQuery', () => {
  it('keys a read by workspace, name and input', () => {
    const key = opKey('decision_list' as never, { spaceId: 'AWD' } as never, 'acme')
    expect(key).toEqual(['op', 'acme', 'decision_list', { spaceId: 'AWD' }])
    const query = opQuery('decision_list' as never, { spaceId: 'AWD' } as never, { workspaceId: 'acme' })
    expect(query.queryKey).toEqual(key)
  })
})
