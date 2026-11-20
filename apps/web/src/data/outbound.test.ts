import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import { callOp } from '../api/ops'
import { setLocalDraftOwner } from '../app/localDrafts'
import { clearOutbound, discardOutbound, flushOutbound, outboundEntries, queueOp, resetOutboundForTests, retryOutbound } from './outbound'

vi.mock('../api/ops', async (original) => ({ ...(await original<typeof import('../api/ops')>()), callOp: vi.fn() }))

const offline = () => new TypeError('Failed to fetch')
const refusal = () => new ApiError({ status: 403, code: 'permission_denied', message: 'You can’t change this task.', details: null, requestId: null })

const message = (text: string) => ({ conversationId: 'c1', text })

beforeEach(() => {
  resetOutboundForTests()
  setLocalDraftOwner('actor-a')
})

afterEach(() => {
  setLocalDraftOwner(null)
  vi.mocked(callOp).mockReset()
})

describe('the outbound queue (W1.30)', () => {
  it('sends at once when it can, keeping nothing', async () => {
    vi.mocked(callOp).mockResolvedValue({ ok: true } as never)
    const answer = await queueOp('message_send', message('Hello'), { thing: 'conversation:c1', label: 'Message' })
    expect(answer).toEqual({ ok: true })
    expect(outboundEntries()).toEqual([])
  })

  it('keeps writes made offline and sends them in order on reconnect, with the same request ids', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    expect(await queueOp('task_update', { taskId: 't1', title: 'One' } as never, { thing: 'task:t1', label: 'Task edit' })).toBeNull()
    expect(await queueOp('message_send', message('Two'), { thing: 'conversation:c1', label: 'Message' })).toBeNull()
    expect(await queueOp('task_update', { taskId: 't1', title: 'Three' } as never, { thing: 'task:t1', label: 'Task edit' })).toBeNull()
    const kept = outboundEntries().map((entry) => entry.id)
    expect(kept).toHaveLength(3)

    vi.mocked(callOp).mockReset()
    vi.mocked(callOp).mockResolvedValue({} as never)
    const result = await flushOutbound()

    expect(result.sent.map((entry) => entry.id)).toEqual(kept)
    expect(vi.mocked(callOp).mock.calls.map(([, , options]) => options?.requestId)).toEqual(kept)
    expect(outboundEntries()).toEqual([])
  })

  it('holds a later write to a thing behind an earlier one still waiting', async () => {
    vi.mocked(callOp).mockRejectedValueOnce(offline())
    await queueOp('task_update', { taskId: 't1', title: 'First' } as never, { thing: 'task:t1', label: 'Task edit' })
    // Back online, but the first edit hasn't gone yet: the second waits behind it.
    vi.mocked(callOp).mockResolvedValue({} as never)
    expect(await queueOp('task_update', { taskId: 't1', title: 'Second' } as never, { thing: 'task:t1', label: 'Task edit' })).toBeNull()
    expect(vi.mocked(callOp)).toHaveBeenCalledTimes(1)
    await flushOutbound()
    expect(vi.mocked(callOp).mock.calls.slice(1).map(([, input]) => (input as { title: string }).title)).toEqual(['First', 'Second'])
  })

  it('sends a later edit made offline on top of the version the earlier one reached', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('task_update', { taskId: 't1', expectedVersion: 4, title: 'One' } as never, { thing: 'task:t1', label: 'Task edit' })
    await queueOp('task_update', { taskId: 't1', expectedVersion: 4, priority: 'high' } as never, { thing: 'task:t1', label: 'Task edit' })

    vi.mocked(callOp).mockReset()
    vi.mocked(callOp).mockResolvedValueOnce({ version: 5 } as never).mockResolvedValue({ version: 6 } as never)
    await flushOutbound()

    expect(vi.mocked(callOp).mock.calls.map(([, input]) => (input as { expectedVersion: number }).expectedVersion)).toEqual([4, 5])
  })

  it('keeps the version an earlier edit reached when the connection drops before the next', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('task_update', { taskId: 't1', expectedVersion: 4, title: 'One' } as never, { thing: 'task:t1', label: 'Task edit' })
    await queueOp('task_update', { taskId: 't1', expectedVersion: 4, priority: 'high' } as never, { thing: 'task:t1', label: 'Task edit' })

    vi.mocked(callOp).mockReset()
    vi.mocked(callOp).mockResolvedValueOnce({ version: 5 } as never).mockRejectedValueOnce(offline())
    await flushOutbound()
    expect(outboundEntries()).toHaveLength(1)

    vi.mocked(callOp).mockReset()
    vi.mocked(callOp).mockResolvedValue({ version: 6 } as never)
    await flushOutbound()
    expect(vi.mocked(callOp).mock.calls.map(([, input]) => (input as { expectedVersion: number }).expectedVersion)).toEqual([5])
  })

  it('touches nothing kept while nobody is known to be signed in', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('task_update', { taskId: 't1', expectedVersion: 4, title: 'One' } as never, { thing: 'task:t1', label: 'Task edit' })
    setLocalDraftOwner(null)
    await flushOutbound()
    setLocalDraftOwner('actor-a')
    expect(outboundEntries()).toHaveLength(1)
  })

  it('keeps a refusal with its reason, holds that thing’s later writes, and sends the rest', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('task_update', { taskId: 't1', title: 'Refused' } as never, { thing: 'task:t1', label: 'Task edit' })
    await queueOp('task_update', { taskId: 't1', title: 'After' } as never, { thing: 'task:t1', label: 'Task edit' })
    await queueOp('message_send', message('Fine'), { thing: 'conversation:c1', label: 'Message' })

    vi.mocked(callOp).mockReset()
    vi.mocked(callOp).mockRejectedValueOnce(refusal()).mockResolvedValue({} as never)
    const result = await flushOutbound()

    expect(result.refused).toHaveLength(1)
    expect(result.sent.map((entry) => entry.label)).toEqual(['Message'])
    const left = outboundEntries()
    expect(left.map((entry) => (entry.input as { title: string }).title)).toEqual(['Refused', 'After'])
    expect(left[0].refused).toBeTruthy()
    expect(left[1].refused).toBeUndefined()

    // Dropping the refused one lets the next go.
    await discardOutbound(left[0].id)
    await flushOutbound()
    expect(outboundEntries()).toEqual([])
  })

  it('tries a refused write again on request', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('message_send', message('Again'), { thing: 'conversation:c1', label: 'Message' })
    vi.mocked(callOp).mockReset()
    vi.mocked(callOp).mockRejectedValueOnce(refusal())
    await flushOutbound()
    const [entry] = outboundEntries()
    vi.mocked(callOp).mockResolvedValue({} as never)
    await retryOutbound(entry.id)
    expect(outboundEntries()).toEqual([])
    expect(vi.mocked(callOp).mock.calls.at(-1)?.[2]?.requestId).toBe(entry.id)
  })

  it('stops at a lost connection and keeps the rest', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('message_send', message('One'), { thing: 'conversation:c1', label: 'Message' })
    await queueOp('message_send', message('Two'), { thing: 'conversation:c2', label: 'Message' })
    const result = await flushOutbound()
    expect(result).toEqual({ sent: [], refused: [] })
    expect(outboundEntries()).toHaveLength(2)
  })

  it('forgets everything on sign-out', async () => {
    vi.mocked(callOp).mockRejectedValue(offline())
    await queueOp('message_send', message('Mine'), { thing: 'conversation:c1', label: 'Message' })
    await clearOutbound()
    expect(outboundEntries()).toEqual([])
  })

  it('refuses at once, keeping nothing, when the server says no while online', async () => {
    vi.mocked(callOp).mockRejectedValue(refusal())
    await expect(queueOp('message_send', message('No'), { label: 'Message' })).rejects.toBeInstanceOf(ApiError)
    expect(outboundEntries()).toEqual([])
  })
})
