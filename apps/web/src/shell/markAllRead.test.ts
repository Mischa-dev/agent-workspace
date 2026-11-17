import { beforeEach, expect, it, vi } from 'vitest'

import { callOp } from '../api/ops'
import { markAllRead } from './markAllRead'

vi.mock('../api/ops', () => ({ callOp: vi.fn(), newRequestId: () => 'request' }))

beforeEach(() => vi.resetAllMocks())

it('asks each workspace until it has nothing more, and counts a workspace that fails', async () => {
  vi.mocked(callOp)
    .mockResolvedValueOnce({ marked: 500, more: true } as never)
    .mockResolvedValueOnce({ marked: 3, more: false } as never)
    .mockRejectedValueOnce(new Error('Offline'))

  expect(await markAllRead(['one', 'one', 'two'])).toEqual({ marked: 503, failed: 1 })
  expect(callOp).toHaveBeenCalledTimes(3)
  expect(callOp).toHaveBeenCalledWith('conversations_mark_read', {}, { workspaceId: 'one', requestId: 'request' })
})
