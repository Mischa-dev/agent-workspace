import { isSafeMessageText } from '../conversations/messageContentModel'

export function coalesceDelta(
  current: Record<string, string>,
  messageId: string,
  delta: string,
) {
  const candidate = `${current[messageId] ?? ''}${delta}`
  return isSafeMessageText(candidate)
    ? { ...current, [messageId]: candidate }
    : current
}

export function coalesceDeltaBatch(
  current: Record<string, string>,
  pending: Record<string, string>,
) {
  const next = { ...current }
  for (const [messageId, delta] of Object.entries(pending)) {
    const candidate = `${next[messageId] ?? ''}${delta}`
    if (isSafeMessageText(candidate)) next[messageId] = candidate
  }
  return next
}
