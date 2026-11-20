import { useRef, useState, type ReactNode } from 'react'

import { callOp, newRequestId } from '../api/ops'
import type { RequestRespondOutput } from '../api/generated/operations'
import { toastError } from '../app/toastError'
import { Banner, Button, StatusChip, TextArea } from '../ui'
import { settledText, type RequestAnswer, type RequestCardData } from './requestModel'
import './request.css'

export type { RequestAnswer, RequestCardData } from './requestModel'

const isApprove = (option: { id: string; style?: string | null }) => (option.style ?? option.id) === 'approve'
const isDeny = (option: { id: string; style?: string | null }) => (option.style ?? option.id) === 'deny'

/**
 * A request for an answer (W1.24): pick one of its answers or type your own.
 * One piece for the inbox, a thread, a run's page and a task. It answers
 * through `request_respond` with one request id per answer (a retry is
 * answered once), then `onAnswered` hears the request as it is now. Once
 * settled it says what became of it; an answer is final.
 *
 * `heading` names who asks (an agent's name and mark); `canAnswer` is false
 * for someone the request doesn't ask, who sees it waiting.
 */
export function RequestCard({
  request,
  heading,
  canAnswer = true,
  onAnswered,
  variant = 'card',
  waitingNote = 'This waits on whoever it asks; they answer it here or from their inbox.',
}: {
  request: RequestCardData
  heading?: ReactNode
  canAnswer?: boolean
  onAnswered?: (request: RequestRespondOutput) => void
  /** `card` stands on its own (a thread, a run); `inline` sits inside something that already frames it (an inbox item). */
  variant?: 'card' | 'inline'
  waitingNote?: string
}) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  // One request id per answer, kept across a retry of the same answer.
  const pending = useRef<{ key: string; id: string } | null>(null)
  const open = request.status === 'open'
  const question = request.kind !== 'approval'
  const typed = text.trim()
  const titleId = `request-${request.id}`

  const send = async (key: string, answer: RequestAnswer) => {
    if (busy) return
    if (pending.current?.key !== key) pending.current = { key, id: newRequestId() }
    setBusy(key)
    try {
      const answered = await callOp('request_respond', { requestId: request.id, ...answer }, { requestId: pending.current.id })
      pending.current = null
      setText('')
      onAnswered?.(answered)
    } catch (reason) {
      toastError('Your answer wasn’t sent', reason)
    } finally {
      setBusy(null)
    }
  }

  const body = open ? (
    canAnswer ? (
      <form
        className="aw-request__form"
        data-part="request-form"
        onSubmit={(event) => {
          event.preventDefault()
          if (typed) void send('text', { text: typed })
        }}
      >
        {request.options.length ? (
          <div aria-label={question ? 'Suggested answers' : 'Answers'} className="aw-request__options" data-part="request-options" role="group">
            {/* An offered answer is an answer: it sends straight away, with any words typed. */}
            {request.options.map((option, index) => (
              <Button
                busy={busy === option.id}
                disabled={busy !== null}
                key={option.id}
                onClick={() => void send(option.id, typed ? { optionId: option.id, text: typed } : { optionId: option.id })}
                type="button"
                variant={isDeny(option) ? 'danger' : isApprove(option) || index === 0 ? 'primary' : 'secondary'}
              >
                {option.label}
              </Button>
            ))}
          </div>
        ) : null}
        {request.allowText || !request.options.length ? (
          <>
            <TextArea
              disabled={busy !== null}
              hint={request.options.length ? 'Typed words go with the answer you pick, or on their own with Send.' : undefined}
              id={`request-answer-${request.id}`}
              label={request.options.length ? 'Or answer in your own words' : 'Your answer'}
              maxLength={4000}
              onChange={(event) => setText(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && typed) {
                  event.preventDefault()
                  void send('text', { text: typed })
                }
              }}
              rows={3}
              value={text}
            />
            <div className="aw-request__send">
              <Button busy={busy === 'text'} disabled={busy !== null || !typed} icon="send" type="submit" variant={request.options.length ? 'secondary' : 'primary'}>
                Send
              </Button>
            </div>
          </>
        ) : null}
      </form>
    ) : (
      <p className="aw-request__note">{waitingNote}</p>
    )
  ) : (
    <Banner tone={request.status === 'answered' ? 'success' : 'info'}>{settledText(request)}</Banner>
  )

  if (variant === 'inline') {
    return (
      <div className="aw-request aw-request--inline" data-part="request-card" data-status={request.status}>
        <blockquote className="aw-request__question" id={titleId}>
          {request.title}
        </blockquote>
        {request.detail ? <p className="aw-request__detail">{request.detail}</p> : null}
        {body}
      </div>
    )
  }
  return (
    <section aria-labelledby={titleId} className="aw-card aw-request" data-part="request-card" data-status={request.status}>
      <header className="aw-card__head">
        {heading ? <div className="aw-card__titles">{heading}</div> : null}
        {open ? (
          <StatusChip icon={question ? 'circle-help' : 'shield-check'} tone="warning">
            Needs you
          </StatusChip>
        ) : null}
      </header>
      <p className="aw-request__question" id={titleId}>
        {request.title}
      </p>
      {request.detail ? <p className="aw-request__detail">{request.detail}</p> : null}
      {body}
    </section>
  )
}
