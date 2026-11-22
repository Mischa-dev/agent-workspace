import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { callOp } from '../api/ops'
import { inboxKeys, noteWorkspaces, postUpdate } from './api'

vi.mock('../api/ops', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/ops')>()),
  callOp: vi.fn(),
}))

const sent = () =>
  vi.mocked(callOp).mock.calls.map(([, input, options]) => ({ workspace: options?.workspaceId, ids: (input as { ids?: string[] }).ids }))

afterEach(() => vi.mocked(callOp).mockReset())

describe('an inbox change across workspaces', () => {
  it('goes only to the workspace its notifications live in; unknown ones go everywhere', async () => {
    vi.mocked(callOp).mockImplementation(async (_name, input) => ({ updated: (input as { ids: string[] }).ids, unreadCount: 1 }) as never)
    const client = new QueryClient()
    client.setQueryData(queryKeys.projects, { projects: [{ id: 'space-a', organizationId: 'alpha' }] })
    client.setQueryData(inboxKeys.list('inbox', 'all'), {
      pages: [{ notifications: [{ id: 'n-alpha', projectId: 'space-a' }, { id: 'n-dm', projectId: 'direct' }] }],
      pageParams: [null],
    })
    await postUpdate({ action: 'archive', ids: ['n-alpha', 'n-dm'] }, ['alpha', 'beta'], noteWorkspaces(client))
    expect(sent()).toEqual([
      { workspace: 'alpha', ids: ['n-alpha'] },
      { workspace: 'alpha', ids: ['n-dm'] },
      { workspace: 'beta', ids: ['n-dm'] },
    ])
  })

  it('keeps what one workspace did when another refuses', async () => {
    const refusal = new Error('Held')
    vi.mocked(callOp)
      .mockResolvedValueOnce({ updated: ['n1'], unreadCount: 2 } as never)
      .mockRejectedValueOnce(refusal)
    const result = await postUpdate({ action: 'read', all: true }, ['alpha', 'beta'])
    expect(result.updated).toEqual(['n1'])
    expect(result.refused).toBe(refusal)
  })

  it('fails when no workspace takes it', async () => {
    const refusal = new Error('Held')
    vi.mocked(callOp).mockRejectedValue(refusal)
    await expect(postUpdate({ action: 'read', all: true }, ['alpha', 'beta'])).rejects.toBe(refusal)
  })
  it('keeps the parts of a long change that landed before one was refused', async () => {
    const refusal = new Error('Held')
    const ids = Array.from({ length: 250 }, (_, index) => `n${index}`)
    vi.mocked(callOp)
      .mockImplementationOnce(async (_name, input) => ({ updated: (input as { ids: string[] }).ids, unreadCount: 0 }) as never)
      .mockRejectedValueOnce(refusal)
    const result = await postUpdate({ action: 'archive', ids }, ['alpha'])
    expect(result.updated).toEqual(ids.slice(0, 200))
    expect(result.refused).toBe(refusal)
  })

  it('notes notifications a space overview cached as a single page', () => {
    const client = new QueryClient()
    client.setQueryData(queryKeys.projects, { projects: [{ id: 'space-a', organizationId: 'alpha' }] })
    client.setQueryData(inboxKeys.project('space-a'), { notifications: [{ id: 'n-alpha', projectId: 'space-a' }] })
    expect(noteWorkspaces(client).get('n-alpha')).toBe('alpha')
  })
})
