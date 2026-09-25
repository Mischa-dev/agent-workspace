import { Button, Dots, Icon, StatusChip, type IconName } from '../ui'
import type { ReplyRequest } from './types'

function timeAgo(iso: string) {
  const seconds = Math.max(0, (Date.now() - Date.parse(iso)) / 1000)
  if (!Number.isFinite(seconds)) return 'a while ago'
  if (seconds < 60) return 'just now'
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`
  if (seconds < 86_400) return `${Math.round(seconds / 3600)} hr ago`
  return `${Math.round(seconds / 86_400)} days ago`
}

const ICONS: Partial<Record<ReplyRequest['status'], IconName>> = {
  pending: 'clock',
  passed: 'circle-minus',
  released: 'undo',
  expired: 'clock',
}

function describe(request: ReplyRequest) {
  const name = request.agent.displayName
  switch (request.status) {
    case 'claimed':
      return `${name} is writing a reply…`
    case 'pending':
      return request.agentLastSeenAt
        ? `Waiting for ${name} to pick this up · last checked in ${timeAgo(request.agentLastSeenAt)}`
        : `Waiting for ${name} to connect`
    case 'passed':
      return `${name} passed`
    case 'released':
      return `${name} handed this back`
    case 'expired':
      return `${name} didn’t answer in time`
    default:
      return `${name} replied`
  }
}

/** One line per agent asked about a message, until it replies. */
export function ReplyStatusList({
  onRetry,
  requests,
}: {
  onRetry: (requestId: string) => void
  requests: ReplyRequest[]
}) {
  const visible = requests.filter((request) => request.status !== 'replied')
  if (!visible.length) return null
  return (
    <ul aria-label="Replies" className="aw-replies">
      {visible.map((request) => (
        <li className={`aw-reply aw-reply--${request.status}`} key={request.id} role="status">
          {request.status === 'claimed' ? <Dots /> : <Icon className="aw-reply__icon" name={ICONS[request.status] ?? 'check'} size={14} />}
          <span className="aw-reply__text">{describe(request)}</span>
          {request.reason && (request.status === 'passed' || request.status === 'released') ? (
            <span className="aw-reply__reason">“{request.reason}”</span>
          ) : null}
          {request.required && request.status === 'pending' ? <StatusChip tone="agent">Reply required</StatusChip> : null}
          {request.status === 'expired' || request.status === 'released' ? (
            <Button icon="retry" onClick={() => onRetry(request.id)} size="sm" variant="ghost">
              Ask again
            </Button>
          ) : null}
        </li>
      ))}
    </ul>
  )
}
