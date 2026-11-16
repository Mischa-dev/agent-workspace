import type { QueryClient } from '@tanstack/react-query'

import { queryKeys } from '../../api/queryKeys'
import type { RealtimeEnvelope } from '../realtimeModel'
import { conversationHandlers } from './conversations'
import { peopleHandlers } from './people'
import { runHandlers } from './runs'
import { taskHandlers } from './tasks'
import type { ConversationShape, Handler, HandlerContext, Touch } from './touches'

export type { Touch } from './touches'

/**
 * What each durable event makes stale, by type (W1.3). Each module's file
 * names the keys its events touch; nothing refetches a whole project except
 * a membership change, which can change what anyone may see there.
 */
export const HANDLERS: Readonly<Record<string, Handler>> = {
  ...conversationHandlers,
  ...runHandlers,
  ...taskHandlers,
  ...peopleHandlers,
}

/**
 * Events whose own module refreshes exactly what they change: documents
 * (liveDocuments.ts, loaded with the first) and module switches
 * (moduleSwitches.ts).
 */
export const SELF_HANDLED: ReadonlySet<string> = new Set([
  'whiteboard.updated',
  'page.updated',
  'page.discussion.updated',
  'collection.updated',
  'sheet.updated',
  'project.modules.updated',
])

/** The queries an event makes stale. An event type the client doesn't know touches nothing. */
export function touchesFor(event: RealtimeEnvelope, context: HandlerContext): Touch[] {
  return HANDLERS[event.type]?.(event, context) ?? []
}

/**
 * After a (re)connect anything may have been missed, so everything the
 * signed-in person sees refetches: the one broad refresh.
 */
export const RECONNECT: readonly Touch[] = [
  { id: 'reconnect:projects', filters: { queryKey: queryKeys.projects } },
  { id: 'reconnect:direct', filters: { queryKey: queryKeys.direct } },
  { id: 'reconnect:spaces', filters: { predicate: (query) => query.queryKey[0] === queryKeys.project('')[0] } },
  { id: 'reconnect:inbox', filters: { queryKey: queryKeys.inbox } },
]

/** What handlers may read from the cache: a conversation's kind, from any of its cached timelines. */
export function cacheContext(queryClient: QueryClient): HandlerContext {
  return {
    conversation: (projectId, conversationId) => {
      for (const [, data] of queryClient.getQueriesData<{ conversation?: Partial<ConversationShape> }>({
        queryKey: queryKeys.conversation(projectId, conversationId),
      })) {
        const kind = data?.conversation?.kind
        if (typeof kind === 'string') return { kind, parentKind: data?.conversation?.parentKind ?? null }
      }
      return null
    },
  }
}
