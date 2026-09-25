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
          return <span key={`text-${index}`}>{node.text}</span>
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
