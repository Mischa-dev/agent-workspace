import type { OutboxTarget, QueuedSend } from '../conversations/composerModel'
import { isConnectionError, localDraftOwner, localKeys, readLocal, unsentHeld, writeLocal } from './localDrafts'

const UNSENT = 'unsent:'

export type OutboxResult = {
  sent: (QueuedSend & { target: OutboxTarget })[]
  refused: (QueuedSend & { target: OutboxTarget })[]
}

function update(key: string, change: (items: QueuedSend[]) => QueuedSend[]) {
  const next = change(readLocal<QueuedSend[]>(key) ?? [])
  writeLocal<QueuedSend[]>(key, next.length ? next : null)
}

/**
 * Sends what the signed-in person wrote while offline in boxes that aren't
 * on screen, oldest first in each, with the key each was first sent with,
 * so a send that did reach the server isn't posted twice. A box on screen
 * sends its own. A send the server refuses stays queued, marked, and goes
 * back into its box when that opens. Stops when the connection is gone again,
 * or when someone else signs in.
 */
export async function flushOutbox(send: (target: OutboxTarget, item: QueuedSend) => Promise<unknown>): Promise<OutboxResult> {
  const result: OutboxResult = { sent: [], refused: [] }
  const owner = localDraftOwner()
  if (!owner) return result
  for (const key of localKeys(UNSENT)) {
    const draftKey = key.slice(UNSENT.length)
    for (const item of readLocal<QueuedSend[]>(key) ?? []) {
      const { target } = item
      if (!target || item.refused) continue
      if (localDraftOwner() !== owner) return result
      if (unsentHeld(draftKey)) break
      try {
        await send(target, item)
      } catch (reason) {
        if (isConnectionError(reason) || localDraftOwner() !== owner) return result
        update(key, (items) => items.map((entry) => (entry.idempotencyKey === item.idempotencyKey ? { ...entry, refused: true } : entry)))
        result.refused.push({ ...item, target })
        continue
      }
      if (localDraftOwner() !== owner) return result
      update(key, (items) => items.filter((entry) => entry.idempotencyKey !== item.idempotencyKey))
      result.sent.push({ ...item, target })
    }
  }
  return result
}
