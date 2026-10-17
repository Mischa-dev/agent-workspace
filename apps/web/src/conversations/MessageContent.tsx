import { Link } from 'react-router-dom'

import { parseMessageContent } from './messageContentModel'
import type { ConversationActor, ConversationWorkItemReference } from './types'

type MessageContentProps = {
  actors: ConversationActor[]
  content: unknown
  fallbackPlainText: string
  workItems?: ConversationWorkItemReference[]
  /** Mentions of this actor are marked as mentions of you. */
  viewerId?: string
}

function workItemState(statusCategory: string) {
  if (statusCategory === 'done' || statusCategory === 'completed' || statusCategory === 'canceled') return 'done'
  if (statusCategory === 'in_progress' || statusCategory === 'started' || statusCategory === 'active') return 'doing'
  return 'todo'
}

/** `@everyone` or `@here` as a word, which notify everyone who can see the conversation. */
const EVERYONE_MENTION = /(?<![\p{L}\p{N}_@])@(?:everyone|here)(?![\p{L}\p{N}_])/giu

/** Text with `@everyone` and `@here` marked like mentions. */
function TextWithEveryone({ text }: { text: string }) {
  const parts: Array<string | { mention: string }> = []
  let last = 0
  for (const match of text.matchAll(EVERYONE_MENTION)) {
    const start = match.index
    if (start > last) parts.push(text.slice(last, start))
    parts.push({ mention: match[0] })
    last = start + match[0].length
  }
  if (parts.length === 0) return <>{text}</>
  if (last < text.length) parts.push(text.slice(last))
  return (
    <>
      {parts.map((part, index) =>
        typeof part === 'string' ? (
          part
        ) : (
          <span className="aw-mention is-everyone" key={index} title="Notifies everyone who can see this conversation">
            {part.mention}
          </span>
        ),
      )}
    </>
  )
}

/** Message text with people, agents and work items as chips. Text is always rendered as text. */
export function MessageContent({ actors, content, fallbackPlainText, workItems = [], viewerId }: MessageContentProps) {
  const parsed = parseMessageContent(content)
  if (!parsed) {
    return <span className="aw-content">{fallbackPlainText}</span>
  }

  const actorsById = new Map(actors.map((actor) => [actor.id, actor]))
  const workItemsById = new Map(workItems.map((item) => [item.id, item]))
  return (
    <span className="aw-content">
      {parsed.nodes.map((node, index) => {
        if (node.type === 'text') {
          return (
            <span key={`text-${index}`}>
              <TextWithEveryone text={node.text} />
            </span>
          )
        }

        if (node.type === 'work_item_ref') {
          const item = workItemsById.get(node.workItemId)
          if (!item) {
            return (
              <span aria-label="Unavailable Work Item reference" className="aw-workref is-unavailable" key={`work-item-${index}`}>
                Unavailable work item
              </span>
            )
          }
          return (
            <Link
              aria-label={`${item.key}: ${item.title}`}
              className="aw-workref"
              data-archived={Boolean(item.archivedAt)}
              key={`work-item-${index}`}
              title={item.title}
              to={item.href}
            >
              <span aria-hidden="true" className={`aw-workref__status is-${workItemState(item.statusCategory)}`} />
              <span className="aw-workref__key">{item.key}</span>
              <span className="aw-workref__title">{item.title}</span>
            </Link>
          )
        }

        const actor = actorsById.get(node.actorId)
        if (!actor) {
          return (
            <span aria-label="Unavailable Actor mention" className="aw-mention is-unavailable" key={`mention-${index}`}>
              @unavailable
            </span>
          )
        }

        return (
          <span
            aria-label={`${actor.displayName}, @${actor.handle}`}
            className={actor.id === viewerId ? 'aw-mention is-you' : 'aw-mention'}
            data-actor-id={actor.id}
            key={`mention-${index}`}
            title={`@${actor.handle}`}
          >
            @{actor.displayName}
          </span>
        )
      })}
    </span>
  )
}
