import type { Pin } from './api'
import { spaceHref } from '../app/href'

/** Where a pin opens: its page, its decision, or the message in this conversation. */
export function pinHref(projectId: string, item: Pin) {
  return item.kind === 'page'
    ? spaceHref(projectId, `/pages/${encodeURIComponent(item.id)}`)
    : item.kind === 'decision'
      ? spaceHref(projectId, `/decisions#${encodeURIComponent(item.id)}`)
      : `#message-${item.id}`
}
