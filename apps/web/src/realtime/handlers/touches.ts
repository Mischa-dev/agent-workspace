import type { InvalidateQueryFilters } from '@tanstack/react-query'

import { isThingKey, isThingReferencesKey, queryKeys } from '../../api/queryKeys'
import { threadsRootKey } from '../../conversations/threadModel'
import { field, uuid, type RealtimeEnvelope } from '../realtimeModel'

/**
 * One set of cached queries an event makes stale (W1.3). The connection
 * refetches each once per burst: two touches with the same id are one.
 */
export type Touch = { id: string; filters: InvalidateQueryFilters }

/** A conversation's kind as the cache knows it, or null where nothing of it is cached. */
export type ConversationShape = { kind: string; parentKind: string | null }

/** What a handler may read besides the event: what the cache already holds. */
export type HandlerContext = {
  conversation: (projectId: string, conversationId: string) => ConversationShape | null
}

/** What one event type makes stale. Only what the event names, never a whole project. */
export type Handler = (event: RealtimeEnvelope, context: HandlerContext) => Touch[]

/** Thing kinds whose id is a conversation's (see modules/channels.json and messages.json). */
const CONVERSATION_THINGS = ['thread', 'channel', 'conversation']

/** An id from the event's payload, in either spelling. */
export function payloadId(event: RealtimeEnvelope, camel: string, snake: string): string | null {
  return uuid(field(event.payload, camel, snake))
}

/** Everything cached about one conversation: its details, timelines, revisions. */
export const conversation = (projectId: string, conversationId: string): Touch => ({
  id: `conversation:${projectId}:${conversationId}`,
  filters: { queryKey: queryKeys.conversation(projectId, conversationId) },
})

/** A thread, channel or conversation embedded in a page as a card. */
export const conversationThing = (conversationId: string): Touch => ({
  id: `thing:conversation:${conversationId}`,
  filters: { predicate: (query) => CONVERSATION_THINGS.some((kind) => isThingKey(query.queryKey, kind, conversationId)) },
})

/** One thing embedded as a card or chip. */
export const thing = (kind: string, id: string): Touch => ({
  id: `thing:${kind}:${id}`,
  filters: { predicate: (query) => isThingKey(query.queryKey, kind, id) },
})

/** "Referenced in" lists: a message, comment or description may have added or dropped a link. */
export const references: Touch = { id: 'references', filters: { predicate: (query) => isThingReferencesKey(query.queryKey) } }

/** A project's channel and thread list, with unread marks. */
export const sidebar = (projectId: string): Touch => ({
  id: `sidebar:${projectId}`,
  filters: { queryKey: queryKeys.conversationSidebarRoot(projectId) },
})

/** A project's thread boards (names, reply counts, status). */
export const threads = (projectId: string): Touch => ({
  id: `threads:${projectId}`,
  filters: { queryKey: threadsRootKey(projectId) },
})

/** The rail's projects, with their attention counts. */
export const rail: Touch = { id: 'rail', filters: { queryKey: queryKeys.projects } }

/** One project's own details (its name, role, attention), not what's under it. */
export const project = (projectId: string): Touch => ({
  id: `project:${projectId}`,
  filters: { queryKey: queryKeys.project(projectId), exact: true },
})

/** DMs and groups, the Messages overview: they live in a space the rail doesn't list. */
export const direct: Touch = { id: 'direct', filters: { queryKey: queryKeys.direct } }

/** The inbox, which reads names and previews live. */
export const inbox: Touch = { id: 'inbox', filters: { queryKey: queryKeys.inbox } }

/** Search results, which quote messages. */
export const search: Touch = { id: 'search', filters: { queryKey: queryKeys.searchAll } }

/** Tasks across projects (My work). */
export const myWork: Touch = { id: 'my-work', filters: { queryKey: queryKeys.myWork } }
