import { here, legacyPathOf } from '../app/href'
import { useState, type FormEvent } from 'react'

import { callOp } from '../api/ops'
import { toastError } from '../app/mutations'
import { atPhrase } from '../later/when'
import {
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
import { RequestCard } from '../pieces/RequestCard'
import { BatchLaunch } from './BatchLaunch'
import { isExternalHref, snoozeMenu, type InboxNotification, type InboxView, type SnoozeTarget } from './api'
import { KIND_ICON } from './kinds'

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
  onAnswered,
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
  /** After a request card here was answered. */
  onAnswered: () => void
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
  const runPage = item.subject.kind === 'agent_run' ? here(`/runs/${encodeURIComponent(item.subject.id)}`) : null
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
        <RequestCard key={item.request.id} onAnswered={onAnswered} request={item.request} variant="inline" />
      ) : preview ? (
        <blockquote className="aw-inbox__quote">{preview}</blockquote>
      ) : null}
      {item.replyConversationId && !item.request && view !== 'archived' ? (
        <ReplyInPlace conversationId={item.replyConversationId} key={item.id} onSent={onReplied} to={item.actor?.name ?? null} />
      ) : null}
      <div className="aw-inbox__actions">
        {item.request && legacyPathOf(item.href) === '/inbox' ? null : external ? (
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
