import type { Pin } from './api'

/** Where a pin opens: its page, its decision, or the message in this conversation. */
export function pinHref(projectId: string, item: Pin) {
  return item.kind === 'page'
    ? `/projects/${encodeURIComponent(projectId)}/pages/${encodeURIComponent(item.id)}`
    : item.kind === 'decision'
      ? `/projects/${encodeURIComponent(projectId)}/decisions#${encodeURIComponent(item.id)}`
      : `#message-${item.id}`
}
