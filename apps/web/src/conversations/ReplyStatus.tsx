import { useState } from 'react'

import { isApiError } from '../api/errors'
import { Button, Dots, Icon, StatusChip } from '../ui'
import { describeReplyRequest, REPLY_ICONS } from './replyStatusModel'
import type { ReplyRequest } from './types'

const SHOWS_REASON = new Set<ReplyRequest['status']>(['passed', 'released', 'cancelled'])
const CAN_RETRY = new Set<ReplyRequest['status']>(['expired', 'released', 'cancelled'])
const CAN_STOP = new Set<ReplyRequest['status']>(['pending', 'claimed'])

/**
 * One line per agent asked about a message, until it replies. A working or
 * waiting agent can be stopped; one that stopped, gave up, or timed out can be
 * asked again.
 */
export function ReplyStatusList({
  onRetry,
  onStop,
  requests,
}: {
  onRetry: (requestId: string) => void
  /** Without it, no Stop button (people who can't post here can't stop agents). */
  onStop?: (requestId: string) => Promise<unknown>
  requests: ReplyRequest[]
}) {
  const [stopping, setStopping] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const visible = requests.filter((request) => request.status !== 'replied')
  if (!visible.length) return null
  const stop = (requestId: string) => {
    if (!onStop) return
    setStopping(requestId)
    setError(null)
    onStop(requestId)
      .catch((reason: unknown) => {
        setError(isApiError(reason) ? reason.message : 'The agent wasn’t stopped. Try again.')
      })
      .finally(() => setStopping(null))
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
          {request.required && request.status === 'pending' ? <StatusChip tone="agent">Reply required</StatusChip> : null}
          {onStop && CAN_STOP.has(request.status) ? (
            <Button
              aria-label={`Stop ${request.agent.displayName}`}
              busy={stopping === request.id}
              disabled={stopping !== null}
              icon="stop"
              onClick={() => stop(request.id)}
              size="sm"
              variant="ghost"
            >
              Stop
            </Button>
          ) : null}
          {CAN_RETRY.has(request.status) ? (
            <Button icon="retry" onClick={() => onRetry(request.id)} size="sm" variant="ghost">
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
