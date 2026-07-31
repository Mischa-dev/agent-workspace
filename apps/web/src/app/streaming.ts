export function coalesceDelta(
  current: Record<string, string>,
  messageId: string,
  delta: string,
) {
  return { ...current, [messageId]: `${current[messageId] ?? ''}${delta}` }
}

export function coalesceDeltaBatch(
  current: Record<string, string>,
  pending: Record<string, string>,
) {
  const next = { ...current }
  for (const [messageId, delta] of Object.entries(pending)) {
    next[messageId] = `${next[messageId] ?? ''}${delta}`
  }
  return next
}
