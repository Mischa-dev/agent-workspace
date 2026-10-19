import { formatRelative, type IconName } from '../ui'
import type { ReplyRequest } from './types'

/** An agent seen this recently is online; its wait needs no time stamp. */
const RECENTLY_SEEN_MS = 5 * 60 * 1000

export const REPLY_ICONS: Partial<Record<ReplyRequest['status'], IconName>> = {
  pending: 'clock',
  passed: 'circle-minus',
  released: 'undo',
  expired: 'clock',
  cancelled: 'stop',
}

/** What a status line says about one agent and one request. */
export function describeReplyRequest(request: ReplyRequest, now = Date.now()) {
  const name = request.agent.displayName
  switch (request.status) {
    case 'claimed': {
      if (!request.progress) return `${name} is writing a reply…`
      const updated = request.progressAt ? formatRelative(request.progressAt) : ''
      return `${name} is working · ${request.progress}${updated ? ` · updated ${updated}` : ''}`
    }
    case 'pending': {
      if (request.followsRequestId) return `${name} will pick this up after what it’s working on`
      if (!request.agentLastSeenAt) return `Waiting for ${name} to connect`
      const seen = Date.parse(request.agentLastSeenAt)
      if (Number.isNaN(seen) || now - seen < RECENTLY_SEEN_MS) return `Waiting for ${name} to reply`
      return `Waiting for ${name} · last seen ${formatRelative(request.agentLastSeenAt)}`
    }
    case 'passed':
      return `${name} passed`
    case 'released':
      return `${name} handed this back`
    case 'expired':
      return `${name} didn’t answer in time`
    case 'cancelled':
      return `${name} was stopped`
    default:
      return `${name} replied`
  }
}

/** Requests keyed by the message they're about. */
export function groupReplyRequests(requests: ReplyRequest[]) {
  const byMessage = new Map<string, ReplyRequest[]>()
  for (const request of requests) {
    byMessage.set(request.messageId, [...(byMessage.get(request.messageId) ?? []), request])
  }
  return byMessage
}
