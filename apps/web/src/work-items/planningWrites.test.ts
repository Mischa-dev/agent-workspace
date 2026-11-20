import { afterEach, describe, expect, it, vi } from 'vitest'

import { callOp } from '../api/ops'
import { updateMilestone } from './planningApi'

vi.mock('../api/ops', () => ({ callOp: vi.fn() }))

afterEach(() => vi.clearAllMocks())

describe('milestone writes through operations', () => {
  it('puts a milestone in an initiative and takes it out again, as the operation names it', async () => {
    vi.mocked(callOp).mockResolvedValue({ milestone: { id: 'm1', name: 'Beta', version: 3 } } as never)
    await updateMilestone({ projectId: 'space-1', milestoneId: 'm1', expectedVersion: 2, initiativeId: 'i1', idempotencyKey: 'k1' }).catch(() => undefined)
    await updateMilestone({ projectId: 'space-1', milestoneId: 'm1', expectedVersion: 3, initiativeId: null, idempotencyKey: 'k2' }).catch(() => undefined)
    const sent = vi.mocked(callOp).mock.calls.map((call) => call[1] as Record<string, unknown>)
    expect(sent[0]).toMatchObject({ initiative: 'i1' })
    expect(sent[1]).toMatchObject({ initiative: '' })
    expect(sent.every((input) => !('initiativeId' in input))).toBe(true)
  })
})
