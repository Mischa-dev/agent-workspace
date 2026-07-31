import { parseMessageContent } from './messageContentModel'
import type { ConversationActor } from './types'

type MessageContentProps = {
  actors: ConversationActor[]
  content: unknown
  fallbackPlainText: string
}

export function MessageContent({
  actors,
  content,
  fallbackPlainText,
}: MessageContentProps) {
  const parsed = parseMessageContent(content)
  if (!parsed) {
    return <span>{fallbackPlainText}</span>
  }

  const actorsById = new Map(actors.map((actor) => [actor.id, actor]))
  return (
    <span className="message-content">
      {parsed.nodes.map((node, index) => {
        if (node.type === 'text') {
          return <span key={`text-${index}`}>{node.text}</span>
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
