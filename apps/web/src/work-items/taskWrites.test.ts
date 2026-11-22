import { afterEach, describe, expect, it, vi } from 'vitest'

import { requestJson } from '../api/client'
import { ApiError } from '../api/errors'
import { callOp } from '../api/ops'
import { updateAcceptanceCriterion, updateWorkItem } from './api'
import { conflictCurrent } from './taskEdits'

vi.mock('../api/ops', async (importOriginal) => ({ ...(await importOriginal<typeof import('../api/ops')>()), callOp: vi.fn() }))
vi.mock('../api/client', () => ({ requestJson: vi.fn() }))

const TASK = { id: 'task-1', projectId: 'space-1', key: 'AW-1', title: 'Parse the flags', version: 5 }

afterEach(() => vi.clearAllMocks())

describe('task writes through operations', () => {
  it('sends every change to a criterion in one call', async () => {
    vi.mocked(callOp).mockResolvedValue({} as never)
    vi.mocked(requestJson).mockResolvedValue(TASK)
    await updateAcceptanceCriterion({ projectId: 'space-1', workItemId: 'task-1', criterionId: 'c1', expectedVersion: 4, text: 'Export works', completed: true, idempotencyKey: 'k' })
    expect(callOp).toHaveBeenCalledTimes(1)
    expect(vi.mocked(callOp).mock.calls[0][1]).toEqual({
      taskId: 'task-1',
      expectedVersion: 4,
      operations: [
        { op: 'edit', criterion: 'c1', text: 'Export works' },
        { op: 'check', criterion: 'c1' },
      ],
    })
  })

  it('refuses a stale edit with the whole task as it is now, not the few fields the server sends', async () => {
    // The server's projection: no project, few fields.
    const refusal = new ApiError({ status: 409, code: 'conflict', message: 'The task changed.', details: { reason: 'version', currentVersion: 5, current: { task: { id: 'task-1', version: 5, title: 'Parse the flags' } } } })
    expect(conflictCurrent(refusal)).toBeNull()
    vi.mocked(callOp).mockRejectedValue(refusal)
    vi.mocked(requestJson).mockResolvedValue(TASK)
    const error = await updateWorkItem({ projectId: 'space-1', workItemId: 'task-1', patch: { expectedVersion: 4, title: 'Mine' }, idempotencyKey: 'k' }).catch((reason: unknown) => reason)
    expect(conflictCurrent(error)).toMatchObject({ id: 'task-1', projectId: 'space-1', version: 5 })
  })
})
