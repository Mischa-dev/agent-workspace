import type { MouseEvent } from 'react'
import { Link } from 'react-router-dom'

import { AvatarStack, Icon, StatusChip, actorKind, countLabel, formatFullDate, formatRelative } from '../ui'
import { ThreadStatusChip } from './threadBoard'
import type { ThreadChipFacts } from './threadChipModel'

/** The run or reply state worth a chip; quiet when there's nothing to act on. */
function attentionChip(attention: ThreadChipFacts['attention']) {
  switch (attention) {
    case 'response_required':
      return (
        <StatusChip icon="circle-help" tone="warning">
          Needs you
        </StatusChip>
      )
    case 'active':
      return (
        <StatusChip dots tone="agent">
          Working
        </StatusChip>
      )
    case 'failed':
      return (
        <StatusChip icon="triangle-alert" tone="danger">
          Run failed
        </StatusChip>
      )
    default:
      return null
  }
}

/**
 * The line under a message that opened a thread: its name, how many replies
 * and when the last one came, with a chip only when something needs you, is
 * new, or the thread has settled.
 */
export function ThreadChip({
  current = false,
  href,
  onOpen,
  thread,
}: {
  current?: boolean
  /** The thread as a page; a plain click still calls onOpen, so it opens beside the conversation. */
  href?: string
  onOpen: () => void
  thread: ThreadChipFacts
}) {
  const replies = thread.messageCount === undefined ? null : thread.messageCount === 0 ? 'No replies yet' : countLabel(thread.messageCount, 'reply', 'replies')
  const when = thread.messageCount && thread.lastActivityAt ? formatRelative(thread.lastActivityAt) : ''
  const status = thread.status && thread.status.toLowerCase() !== 'open' ? thread.status : null
  const repliers = thread.messageCount ? (thread.participants ?? []) : []
  const face = (
    <>
      {repliers.length ? (
        <AvatarStack
          people={repliers.map((actor) => ({ id: actor.id, name: actor.displayName, kind: actorKind(actor.kind) }))}
          size={18}
        />
      ) : (
        <Icon className="aw-threadchip__icon" name="messages" size={14} />
      )}
      <span className="aw-threadchip__name">{thread.name}</span>
      {replies ? <span className="aw-threadchip__count">{replies}</span> : null}
      {when ? (
        <span className="aw-threadchip__when" title={formatFullDate(thread.lastActivityAt)}>
          Last reply {when}
        </span>
      ) : null}
      {attentionChip(thread.attention)}
      {thread.unread && thread.attention !== 'response_required' ? (
        <span aria-label="New replies" className="aw-threadchip__new" role="img">
          <span aria-hidden="true">New</span>
        </span>
      ) : null}
      {status ? <ThreadStatusChip status={status} /> : null}
      <Icon className="aw-threadchip__chev" name="chevron-right" size={14} />
      {repliers.length ? (
        <span className="visually-hidden">, replies from {repliers.map((actor) => actor.displayName).join(', ')}</span>
      ) : null}
    </>
  )
  const title = thread.outcome ? `${thread.name}: ${thread.outcome}` : undefined
  if (href) {
    return (
      <Link
        aria-current={current ? 'true' : undefined}
        className="aw-threadchip"
        onClick={(event: MouseEvent<HTMLAnchorElement>) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return
          event.preventDefault()
          onOpen()
        }}
        title={title}
        to={href}
      >
        {face}
        <span className="visually-hidden">, open thread</span>
      </Link>
    )
  }
  return (
    <button aria-current={current ? 'true' : undefined} className="aw-threadchip" onClick={onOpen} title={title} type="button">
      {face}
      <span className="visually-hidden">, open thread</span>
    </button>
  )
}
