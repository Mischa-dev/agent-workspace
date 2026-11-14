import { useState } from 'react'

import { Button, Dots, Icon, StatusChip, describeError } from '../ui'
import { describeReplyRequest, REPLY_ICONS } from './replyStatusModel'
import type { ReplyRequest } from './types'
import '../styles/conversation.css'

const SHOWS_REASON = new Set<ReplyRequest['status']>(['passed', 'released', 'cancelled'])
const CAN_RETRY = new Set<ReplyRequest['status']>(['expired', 'released', 'cancelled'])
const CAN_STOP = new Set<ReplyRequest['status']>(['pending', 'claimed'])

/**
 * One line per agent asked about a message, until it replies. A working or
 * waiting agent can be stopped; one that stopped, gave up, or timed out can be
 * asked again. Both buttons show they're busy and say why when they fail.
 */
export function ReplyStatusList({
  onRetry,
  onStop,
  requests,
}: {
  onRetry: (requestId: string) => unknown
  /** Without it, no Stop button (people who can't post here can't stop agents). */
  onStop?: (requestId: string) => Promise<unknown>
  requests: ReplyRequest[]
}) {
  const [busy, setBusy] = useState<{ requestId: string; action: 'stop' | 'retry' } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const visible = requests.filter((request) => request.status !== 'replied')
  if (!visible.length) return null
  const run = (requestId: string, action: 'stop' | 'retry', request: () => unknown, failure: string) => {
    setBusy({ requestId, action })
    setError(null)
    let started: unknown
    try {
      started = request()
    } catch (reason) {
      started = Promise.reject(reason)
    }
    Promise.resolve(started)
      .catch((reason: unknown) => setError(`${failure} ${describeError(reason).message}`))
      .finally(() => setBusy(null))
  }
  return (
    <ul aria-label="Replies" className="aw-replies">
      {visible.map((request) => (
        <li className={`aw-reply aw-reply--${request.status}`} key={request.id} role="status">
          {request.status === 'claimed' ? <Dots /> : <Icon className="aw-reply__icon" name={REPLY_ICONS[request.status] ?? 'check'} size={14} />}
          <span className="aw-reply__text">{describeReplyRequest(request)}</span>
          {request.reason && SHOWS_REASON.has(request.status) ? (
            <span className="aw-reply__reason">{request.status === 'cancelled' ? request.reason : `“${request.reason}”`}</span>
          ) : null}
          {request.required && request.status === 'pending' ? <StatusChip tone="agent">Needs a reply</StatusChip> : null}
          {onStop && CAN_STOP.has(request.status) ? (
            <Button
              aria-label={`Stop ${request.agent.displayName}`}
              busy={busy?.requestId === request.id && busy.action === 'stop'}
              disabled={busy !== null}
              icon="stop"
              onClick={() => run(request.id, 'stop', () => onStop(request.id), `${request.agent.displayName} wasn’t stopped.`)}
              size="sm"
              variant="ghost"
            >
              Stop
            </Button>
          ) : null}
          {CAN_RETRY.has(request.status) ? (
            <Button
              busy={busy?.requestId === request.id && busy.action === 'retry'}
              disabled={busy !== null}
              icon="retry"
              onClick={() => run(request.id, 'retry', () => onRetry(request.id), `${request.agent.displayName} wasn’t asked again.`)}
              size="sm"
              variant="ghost"
            >
              Ask again
            </Button>
          ) : null}
        </li>
      ))}
      {error ? (
        <li className="aw-reply aw-reply--error" role="alert">
          <Icon className="aw-reply__icon" name="circle-alert" size={14} />
          <span className="aw-reply__text">{error}</span>
        </li>
      ) : null}
    </ul>
  )
}
