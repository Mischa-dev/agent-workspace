import { Forward } from 'lucide-react'
import { Link } from 'react-router-dom'

import { Markdown } from '../ui/markdown/Markdown'
import { isSafeMessageText } from './messageContentModel'
import { formatDateTime, formatFullDate } from '../ui/time'
import type { ForwardedMessage } from './types'

function text(value: unknown) {
  return typeof value === 'string' ? value : null
}

/** The wire's forward, checked: only what's available is kept, and only in-app links. */
function readForwarded(value: unknown): ForwardedMessage | null {
  const forwarded = value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
  if (typeof forwarded.messageId !== 'string') return null
  const available = forwarded.available === true
  const href = text(forwarded.href)
  const quote = text(forwarded.text)
  return {
    messageId: forwarded.messageId,
    available,
    removed: available && forwarded.removed === true,
    authorName: available ? text(forwarded.authorName) : null,
    place: available ? text(forwarded.place) : null,
    text: available && quote !== null && isSafeMessageText(quote) ? quote.slice(0, 2_000) : null,
    createdAt: available ? text(forwarded.createdAt) : null,
    href: available && href?.startsWith('/') && !href.startsWith('//') ? href : null,
  }
}

/**
 * The message a forward quotes, read live for you: its author, where it was
 * said and its text (as Markdown, cut at four lines), with a link back. Someone who can't see the original
 * sees only that a message was forwarded.
 */
export function ForwardedQuote({ value }: { value: unknown }) {
  const forwarded = readForwarded(value)
  if (!forwarded) return null
  if (!forwarded.available) {
    return (
      <div className="aw-forward is-unavailable">
        <Forward aria-hidden="true" className="aw-icon" size={14} />
        <span>Forwarded from a conversation you can’t see.</span>
      </div>
    )
  }
  const meta = (
    <>
      <Forward aria-hidden="true" className="aw-icon" size={14} />
      <span className="aw-forward__author">{forwarded.authorName}</span>
      {forwarded.place ? <span className="aw-forward__place">in {forwarded.place}</span> : null}
      {forwarded.createdAt ? (
        <time className="aw-forward__time" dateTime={forwarded.createdAt} title={formatFullDate(forwarded.createdAt)}>
          {formatDateTime(forwarded.createdAt)}
        </time>
      ) : null}
    </>
  )
  return (
    <blockquote className="aw-forward">
      {forwarded.href ? (
        <Link aria-label={`Open the original message from ${forwarded.authorName ?? 'someone'}`} className="aw-forward__meta" to={forwarded.href}>
          {meta}
        </Link>
      ) : (
        <div className="aw-forward__meta">{meta}</div>
      )}
      {forwarded.removed ? (
        <p className="aw-forward__removed">The original was removed.</p>
      ) : (
        <Markdown className="aw-forward__text" source={forwarded.text ?? ''} />
      )}
    </blockquote>
  )
}
