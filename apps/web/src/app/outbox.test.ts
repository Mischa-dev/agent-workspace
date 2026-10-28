import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import type { OutboxTarget, QueuedSend } from '../conversations/composerModel'
import { holdUnsent, readLocal, setLocalDraftOwner, writeLocal } from './localDrafts'
import { flushOutbox } from './outbox'

afterEach(() => {
  setLocalDraftOwner(null)
  window.localStorage.clear()
})

const target = (conversationId: string): OutboxTarget => ({
  kind: 'message',
  conversationId,
  conversationKind: 'channel',
  href: `/projects/p/channels/${conversationId}`,
})

const queued = (key: string, text: string, to?: OutboxTarget): QueuedSend => ({
  content: { version: 1, nodes: [{ type: 'text', text }] },
  idempotencyKey: key,
  text,
  target: to,
})

describe('the outbox', () => {
  it('sends what was written offline in boxes that are closed, each with its first key, once', async () => {
    setLocalDraftOwner('actor-a')
    writeLocal('unsent:c1', [queued('k1', 'First', target('c1')), queued('k2', 'Second', target('c1'))])
    writeLocal('unsent:c2', [queued('k3', 'Elsewhere', target('c2'))])
    const send = vi.fn<(target: OutboxTarget, item: QueuedSend) => Promise<undefined>>(async () => undefined)
    const release = holdUnsent('c2')

    const result = await flushOutbox(send)

    // The open box sends its own; the closed one's go out in order.
    expect(send.mock.calls.map(([, item]) => item.idempotencyKey)).toEqual(['k1', 'k2'])
    expect(result.sent.map((item) => item.text)).toEqual(['First', 'Second'])
    expect(readLocal('unsent:c1')).toBeNull()
    expect(readLocal<QueuedSend[]>('unsent:c2')).toHaveLength(1)
    release()
    await flushOutbox(send)
    expect(send).toHaveBeenCalledTimes(3)
    expect(readLocal('unsent:c2')).toBeNull()
  })

  it('stops while offline, and leaves a refused send for its box', async () => {
    setLocalDraftOwner('actor-a')
    writeLocal('unsent:c1', [queued('k1', 'Refused', target('c1')), queued('k2', 'Waits', target('c1'))])
    const refused = vi.fn(async (_: OutboxTarget, item: QueuedSend) => {
      if (item.idempotencyKey === 'k1') throw new ApiError({ status: 422, code: 'validation_failed', message: 'No' })
      throw new TypeError('Failed to fetch')
    })

    const result = await flushOutbox(refused)

    expect(result.refused.map((item) => item.idempotencyKey)).toEqual(['k1'])
    expect(result.sent).toEqual([])
    expect(readLocal<QueuedSend[]>('unsent:c1')?.map((item) => [item.idempotencyKey, item.refused ?? false])).toEqual([
      ['k1', true],
      ['k2', false],
    ])
    // A refused one isn't tried again until its box opens.
    const send = vi.fn<(target: OutboxTarget, item: QueuedSend) => Promise<undefined>>(async () => undefined)
    await flushOutbox(send)
    expect(send.mock.calls.map(([, item]) => item.idempotencyKey)).toEqual(['k2'])
  })

  it('never sends one person’s queue as someone else, or a send with nowhere to go', async () => {
    setLocalDraftOwner('actor-a')
    writeLocal('unsent:c1', [queued('k1', 'From A', target('c1'))])
    writeLocal('unsent:c2', [queued('k2', 'An old send')])
    setLocalDraftOwner(null)
    setLocalDraftOwner('actor-b')
    const send = vi.fn(async () => undefined)
    expect(await flushOutbox(send)).toEqual({ sent: [], refused: [] })
    setLocalDraftOwner('actor-a')
    await flushOutbox(send)
    expect(send).not.toHaveBeenCalled()
  })
})
