import { useState, type FormEvent } from 'react'

import { callOp } from '../api/ops'
import { toastError } from '../app/mutations'
import { atPhrase } from '../later/when'
import {
  Banner,
  Button,
  ButtonLink,
  Menu,
  StatusChip,
  TextArea,
  countLabel,
  formatDateTime,
  formatFullDate,
  plainPreview,
  showToast,
} from '../ui'
import { BatchLaunch } from './BatchLaunch'
import { isExternalHref, snoozeMenu, type InboxNotification, type InboxRequest, type InboxView, type SnoozeTarget } from './api'
import { KIND_ICON } from './kinds'

/** What became of a question or approval someone already answered. */
/** What was answered, in one line. */
function answerText(answer: NonNullable<InboxRequest['answer']>) {
  return [answer.label, answer.text].filter(Boolean).join(' · ')
}

/** What became of a request that is no longer open. */
function settledText(request: InboxRequest) {
  if (request.status === 'answered' && request.answer) return `Answered: ${answerText(request.answer)}`
  if (request.status === 'expired') return 'Nobody answered in time.'
  if (request.status === 'superseded') return 'A newer request replaced this one.'
  return 'This request was withdrawn.'
}

/**
 * A request for you (a question, approval or proposal): pick one of its
 * answers or type your own. Once settled it shows what became of it; an
 * answer is final.
 */
function RequestCard({ onSend, request }: { onSend: (answer: { optionId?: string; text?: string }) => Promise<void>; request: InboxRequest }) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  if (request.status !== 'open') {
    return (
      <>
        <blockquote className="aw-inbox__quote aw-inbox__question">{request.title}</blockquote>
        <Banner tone={request.status === 'answered' ? 'success' : 'info'}>{settledText(request)}</Banner>
      </>
    )
  }
  const send = async (key: string, answer: { optionId?: string; text?: string }) => {
    setBusy(key)
    try {
      await onSend(answer)
      setText('')
    } finally {
      setBusy(null)
    }
  }
  const typed = text.trim()
  return (
    <form
      className="aw-inbox__answer"
      onSubmit={(event) => {
        event.preventDefault()
        if (typed) void send('text', { text: typed })
      }}
    >
      <blockquote className="aw-inbox__quote aw-inbox__question">{request.title}</blockquote>
      {request.options.length ? (
        <div className="aw-inbox__options" role="group" aria-label="Answers">
          {request.options.map((option) => (
            <Button
              busy={busy === option.id}
              disabled={busy !== null}
              key={option.id}
              onClick={() => void send(option.id, typed ? { optionId: option.id, text: typed } : { optionId: option.id })}
              variant={option.style === 'deny' ? 'danger' : option.id === request.options[0]?.id ? 'primary' : 'secondary'}
            >
              {option.label}
            </Button>
          ))}
        </div>
      ) : null}
      {request.allowText || !request.options.length ? (
        <>
      <TextArea
        hint={request.options.length ? 'Typed words go with the answer you pick, or on their own with Send.' : undefined}
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
      <div className="aw-inbox__answer-send">
        <Button busy={busy === 'text'} disabled={busy !== null || !typed} icon="send" type="submit" variant={request.options.length ? 'secondary' : 'primary'}>
          Send
        </Button>
      </div>
        </>
      ) : null}
    </form>
  )
}

/** Answers a mention, DM or thread reply where it was said, without leaving the inbox (C2.10). */
function ReplyInPlace({ conversationId, onSent, to }: { conversationId: string; onSent: () => void; to: string | null }) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const send = (event?: FormEvent<HTMLFormElement>) => {
    event?.preventDefault()
    const message = text.trim()
    if (!message || busy) return
    setBusy(true)
    void callOp('message_send', { conversationId, text: message })
      .then(() => {
        setText('')
        showToast({ tone: 'success', title: to ? `Replied to ${to}` : 'Replied' })
        onSent()
      })
      .catch((reason: unknown) => toastError('Your reply wasn’t sent', reason))
      .finally(() => setBusy(false))
  }
  return (
    <form className="aw-inbox__reply" onSubmit={send}>
      <TextArea
        id="inbox-reply"
        label="Reply"
        maxLength={4000}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          // Enter sends, Shift+Enter starts a new line, as in the composer.
          if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault()
            send()
          }
        }}
        placeholder={to ? `Reply to ${to}` : 'Reply'}
        rows={2}
        value={text}
      />
      <Button busy={busy} disabled={!text.trim()} size="sm" type="submit" variant="primary">
        Send
      </Button>
    </form>
  )
}

/** The preview pane: what happened, and every way to act on it. */
export function NotificationDetail({
  item,
  onAnswerRequest,
  onArchive,
  onBack,
  onOpen,
  onReplied,
  onSnooze,
  onToggleRead,
  onUnsnooze,
  view,
}: {
  item: InboxNotification
  onAnswerRequest: (answer: { optionId?: string; text?: string }) => Promise<void>
  onArchive: () => void
  onBack: () => void
  onOpen: () => void
  /** After a reply in place went out. */
  onReplied: () => void
  onSnooze: (until: SnoozeTarget) => void
  onToggleRead: () => void
  onUnsnooze: () => void
  view: InboxView
}) {
  const external = isExternalHref(item.href)
  const preview = plainPreview(item.preview)
  const runPage = item.subject.kind === 'agent_run' ? `/runs/${encodeURIComponent(item.subject.id)}` : null
  return (
    <article aria-labelledby="inbox-detail-title" className="aw-inbox__card">
      <Button className="aw-inbox__back" icon="arrow-left" onClick={onBack} variant="ghost">
        Inbox
      </Button>
      <p className="aw-inbox__card-meta">
        <StatusChip icon={KIND_ICON[item.kind] ?? 'info'}>{item.projectName}</StatusChip>
        <span>{item.subject.title}</span>
      </p>
      <h2 className="aw-inbox__card-title" id="inbox-detail-title">
        {item.title}
      </h2>
      <p className="aw-inbox__card-when">
        <time dateTime={item.at} title={formatFullDate(item.at)}>
          {formatDateTime(item.at)}
        </time>
        {item.count > 1 ? ` · ${countLabel(item.count, 'time')}` : ''}
        {item.snoozedUntil ? (
          <span title={formatFullDate(item.snoozedUntil)}> · Snoozed until {atPhrase(item.snoozedUntil)}</span>
        ) : item.untilActivity ? (
          <span> · Snoozed until something new happens</span>
        ) : null}
      </p>
      {item.request ? (
        <RequestCard key={item.request.id} onSend={onAnswerRequest} request={item.request} />
      ) : preview ? (
        <blockquote className="aw-inbox__quote">{preview}</blockquote>
      ) : null}
      {item.replyConversationId && !item.request && view !== 'archived' ? (
        <ReplyInPlace conversationId={item.replyConversationId} key={item.id} onSent={onReplied} to={item.actor?.name ?? null} />
      ) : null}
      <div className="aw-inbox__actions">
        {item.request && item.href === '/inbox' ? null : external ? (
          <Button icon="external" onClick={onOpen} variant="primary">
            Open on GitHub
          </Button>
        ) : (
          <ButtonLink to={item.href} variant="primary">
            Open
          </ButtonLink>
        )}
        {runPage ? (
          <ButtonLink icon="activity" to={runPage}>
            Open run
          </ButtonLink>
        ) : null}
        {item.batch ? (
          <BatchLaunch batch={item.batch} reviewHref={item.launchHref} />
        ) : item.launchHref ? (
          <ButtonLink icon="play" to={item.launchHref}>
            Launch
          </ButtonLink>
        ) : null}
        <Button icon={view === 'archived' ? 'undo' : 'archive'} onClick={onArchive}>
          {view === 'archived' ? 'Restore' : 'Archive'}
        </Button>
        {view === 'snoozed' ? (
          <Button icon="undo" onClick={onUnsnooze}>
            Unsnooze
          </Button>
        ) : view !== 'archived' ? (
          <Menu
            items={snoozeMenu(onSnooze)}
            label="Snooze until"
            trigger={
              <Button data-snooze="one" icon="clock" iconEnd="chevron-down">
                Snooze
              </Button>
            }
          />
        ) : null}
        <Button onClick={onToggleRead} variant="ghost">
          {item.read ? 'Mark unread' : 'Mark read'}
        </Button>
      </div>
    </article>
  )
}
