export function coalesceDelta(
  current: Record<string, string>,
  messageId: string,
  delta: string,
) {
  return { ...current, [messageId]: `${current[messageId] ?? ''}${delta}` }
}
