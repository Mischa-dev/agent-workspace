import type { QueryClient } from '@tanstack/react-query'

import { queryKeys } from '../../api/queryKeys'
import type { ConversationShape, HandlerContext, Touch } from './touches'

export type { Touch } from './touches'

// The table of what each event makes stale is in ./table, loaded when the
// connection opens rather than with the first paint: invalidations wait a
// moment anyway (useRealtimeConnection.ts).

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
  'dashboard.updated',
  'sheet.updated',
  'sheet.ranges_changed',
  'project.modules.updated',
])

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
