import { formatRelative, type IconName } from '../ui'
import type { ReplyRequest } from './types'

function timeAgo(iso: string) {
  return formatRelative(iso) || 'a while ago'
}

export const REPLY_ICONS: Partial<Record<ReplyRequest['status'], IconName>> = {
  pending: 'clock',
  passed: 'circle-minus',
  released: 'undo',
  expired: 'clock',
  cancelled: 'stop',
}

/** What a status line says about one agent and one request. */
export function describeReplyRequest(request: ReplyRequest) {
  const name = request.agent.displayName
  switch (request.status) {
    case 'claimed':
      return request.progress
        ? `${name} is working · ${request.progress}${request.progressAt ? ` · updated ${timeAgo(request.progressAt)}` : ''}`
        : `${name} is working on it…`
    case 'pending':
      if (request.followsRequestId) return `${name} will pick this up after what it’s working on`
      return request.agentLastSeenAt
        ? `Waiting for ${name} to pick this up · last checked in ${timeAgo(request.agentLastSeenAt)}`
        : `Waiting for ${name} to connect`
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
