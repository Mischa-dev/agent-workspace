import { useContext, type MouseEvent } from 'react'
import { Reply } from 'lucide-react'
import { Link } from 'react-router-dom'

import { RevealInThread, readForwarded, revealMessage } from './quotedMessage'
import type { ReplyTarget } from './types'
import { Icon } from '../ui'

import './replies.css'

/**
 * The message a reply answers, read live for you: its author and its first
 * lines, above the reply. Choosing it jumps to the original: it scrolls there
 * when it's on the page, and otherwise follows the original's link, which
 * loads earlier messages until it's found (in a thread open beside its
 * conversation, earlier messages of the thread). Replies only quote their own
 * conversation, and the server still leaves out anything you can't see.
 */
export function ReplyQuote({ value }: { value: unknown }) {
  const revealInThread = useContext(RevealInThread)
  const quoted = readForwarded(value)
  if (!quoted) return null
  if (!quoted.available) {
    return (
      <div className="aw-replyquote is-unavailable">
        <Reply aria-hidden="true" className="aw-icon" size={14} />
        <span>Replying to a message you can’t see.</span>
      </div>
    )
  }
  const author = quoted.authorName ?? 'someone'
  const jump = (event: MouseEvent) => {
    if (revealMessage(quoted.messageId)) {
      event.preventDefault()
    } else if (revealInThread) {
      event.preventDefault()
      revealInThread(quoted.messageId)
    }
  }
  const body = (
    <>
      <Reply aria-hidden="true" className="aw-icon" size={14} />
      <span className="aw-replyquote__author">{author}</span>
      {quoted.removed ? (
        <span className="aw-replyquote__text is-removed">The message was removed.</span>
      ) : (
        <span className="aw-replyquote__text">{quoted.text}</span>
      )}
    </>
  )
  return quoted.href ? (
    <Link aria-label={`Replying to ${author}. Go to their message`} className="aw-replyquote" onClick={jump} to={quoted.href}>
      {body}
    </Link>
  ) : (
    <div className="aw-replyquote">{body}</div>
  )
}

/** The message you're replying to, above the message box, with a way to stop replying. */
export function ReplyingTo({ onCancel, target }: { onCancel: () => void; target: ReplyTarget }) {
  return (
    <div className="aw-replyingto">
      <button
        aria-label={`Replying to ${target.authorName}. Go to their message`}
        className="aw-replyingto__quote"
        onClick={() => revealMessage(target.messageId)}
        type="button"
      >
        <Reply aria-hidden="true" className="aw-icon" size={14} />
        <span className="aw-replyingto__label">
          Replying to <strong>{target.authorName}</strong>
        </span>
        <span className="aw-replyingto__text">{target.text}</span>
      </button>
      <button aria-label="Stop replying" className="aw-replyingto__close" onClick={onCancel} title="Stop replying (Esc)" type="button">
        <Icon name="x" />
      </button>
    </div>
  )
}
