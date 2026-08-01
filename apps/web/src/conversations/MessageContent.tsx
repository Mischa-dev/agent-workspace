import { parseMessageContent } from './messageContentModel'
import { Link } from 'react-router-dom'

import type {
  ConversationActor,
  ConversationWorkItemReference,
} from './types'

type MessageContentProps = {
  actors: ConversationActor[]
  content: unknown
  fallbackPlainText: string
  workItems?: ConversationWorkItemReference[]
}

export function MessageContent({
  actors,
  content,
  fallbackPlainText,
  workItems = [],
}: MessageContentProps) {
  const parsed = parseMessageContent(content)
  if (!parsed) {
    return <span>{fallbackPlainText}</span>
  }

  const actorsById = new Map(actors.map((actor) => [actor.id, actor]))
  const workItemsById = new Map(workItems.map((item) => [item.id, item]))
  return (
    <span className="message-content">
      {parsed.nodes.map((node, index) => {
        if (node.type === 'text') {
          return <span key={`text-${index}`}>{node.text}</span>
        }

        if (node.type === 'work_item_ref') {
          const item = workItemsById.get(node.workItemId)
          if (!item) {
            return (
              <span
                aria-label="Unavailable Work Item reference"
                className="work-item-reference work-item-reference-unavailable"
                key={`work-item-${index}`}
              >
                Unavailable Work Item
              </span>
            )
          }
          return (
            <Link
              aria-label={`${item.key}: ${item.title}`}
              className="work-item-reference"
              data-archived={Boolean(item.archivedAt)}
              key={`work-item-${index}`}
              to={item.href}
            >
              {item.key}
            </Link>
          )
        }

        const actor = actorsById.get(node.actorId)
        if (!actor) {
          return (
            <span
              aria-label="Unavailable Actor mention"
              className="message-mention message-mention-unavailable"
              key={`mention-${index}`}
            >
              @unavailable
            </span>
          )
        }

        return (
          <span
            aria-label={`${actor.displayName}, @${actor.handle}`}
            className="message-mention"
            data-actor-id={actor.id}
            key={`mention-${index}`}
          >
            @{actor.handle}
          </span>
        )
      })}
    </span>
  )
}
