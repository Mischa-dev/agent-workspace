import { memoryKeys } from '../../memory/keys'
import { laterKeys } from '../../later/keys'
import type { RealtimeEnvelope } from '../realtimeModel'
import {
  conversation,
  conversationThing,
  direct,
  inbox,
  payloadId,
  rail,
  references,
  search,
  sidebar,
  sidebarSections,
  threads,
  type Handler,
  type HandlerContext,
  type Touch,
} from './touches'

// Chat's events (conversation.*). v1 events name the conversation but not
// its kind or parent, so where that matters the handler asks the cache: a
// conversation nothing has cached may be anywhere, and gets both.

const PRIVATE_KINDS = new Set(['dm', 'group'])

/** Whether the conversation is a thread, or unknown. */
function mayBeThread(event: RealtimeEnvelope, context: HandlerContext, kind?: string | null) {
  if (kind) return kind === 'thread'
  const shape = event.conversationId ? context.conversation(event.projectId, event.conversationId) : null
  return !shape || shape.kind === 'thread'
}

/** Whether the conversation shows on the Messages overview (a DM, a group, or a thread in one), or unknown. */
function mayBeDirect(event: RealtimeEnvelope, context: HandlerContext, kind?: string | null) {
  if (kind && kind !== 'thread') return PRIVATE_KINDS.has(kind)
  const shape = event.conversationId ? context.conversation(event.projectId, event.conversationId) : null
  if (!shape) return true
  return PRIVATE_KINDS.has(shape.kind) || (shape.kind === 'thread' && (!shape.parentKind || PRIVATE_KINDS.has(shape.parentKind)))
}

function when(condition: boolean, ...touches: Touch[]) {
  return condition ? touches : []
}

/** A new channel, thread, DM or group: lists that show it. */
const created: Handler = (event, context) => {
  const id = event.conversationId
  if (!id) return []
  const kind = typeof event.payload.kind === 'string' ? event.payload.kind : null
  return [
    sidebar(event.projectId),
    sidebarSections,
    rail,
    conversationThing(id),
    ...when(mayBeThread(event, context, kind), threads(event.projectId)),
    ...when(mayBeDirect(event, context, kind), direct),
  ]
}

/** Renamed, a thread's status, members or details: the conversation and every list naming it. */
const updated: Handler = (event, context) => {
  const id = event.conversationId
  if (!id) return []
  return [
    conversation(event.projectId, id),
    sidebar(event.projectId),
    sidebarSections,
    conversationThing(id),
    inbox,
    ...when(mayBeThread(event, context), threads(event.projectId)),
    ...when(mayBeDirect(event, context), direct),
  ]
}

const archived: Handler = (event, context) => [...updated(event, context), rail]

/** Who's been asked to reply: shown in the conversation's own timeline. */
const replyRequests: Handler = (event) => (event.conversationId ? [conversation(event.projectId, event.conversationId)] : [])

/** A new message: its timeline, unread marks, and a thread's reply count. */
const messageCreated: Handler = (event, context) => {
  const id = event.conversationId
  if (!id) return []
  return [
    conversation(event.projectId, id),
    sidebar(event.projectId),
    sidebarSections,
    rail,
    conversationThing(id),
    references,
    ...when(mayBeThread(event, context), threads(event.projectId)),
    ...when(mayBeDirect(event, context), direct),
  ]
}

/**
 * An edited or removed message: everything that quotes it. (A removal is
 * also blanked synchronously before this, in liveMessages.)
 */
const messageUpdated: Handler = (event, context) => {
  const id = event.conversationId
  if (!id) return []
  return [
    conversation(event.projectId, id),
    // Thread names come from the message they started from.
    sidebar(event.projectId),
    sidebarSections,
    threads(event.projectId),
    { id: `pins:${event.projectId}:${id}`, filters: { queryKey: memoryKeys.pins(event.projectId, id) } },
    { id: `decisions:${event.projectId}`, filters: { queryKey: memoryKeys.decisions(event.projectId) } },
    { id: 'later', filters: { queryKey: laterKeys.root } },
    search,
    inbox,
    references,
    conversationThing(id),
    ...when(mayBeDirect(event, context), direct),
  ]
}

/** Reactions show in that conversation's timelines and nowhere else. */
const reactions: Handler = (event) => (event.conversationId ? [conversation(event.projectId, event.conversationId)] : [])

/** The viewer read up to somewhere: unread marks. */
const readState: Handler = (event, context) => [
  sidebar(event.projectId),
    sidebarSections,
  rail,
  ...when(mayBeThread(event, context), threads(event.projectId)),
  ...when(mayBeDirect(event, context), direct),
]

/**
 * The viewer left or was removed from a group. What they held of it is
 * dropped synchronously first (liveMessages.dropRemovedConversation); the
 * lists that named it refetch.
 */
const memberRemoved: Handler = (event) => {
  const id = payloadId(event, 'conversationId', 'conversation_id')
  return [rail, direct, sidebar(event.projectId),
    sidebarSections, ...(id ? [conversationThing(id)] : [])]
}

/**
 * Someone joined a group or private channel: its members, and for the new
 * member the lists it now shows in.
 */
const memberAdded: Handler = (event, context) => [...updated(event, context), rail]

export const conversationHandlers: Record<string, Handler> = {
  'conversation.created': created,
  'conversation.updated': updated,
  'conversation.archived': archived,
  'conversation.reply_requests.updated': replyRequests,
  'conversation.message.created': messageCreated,
  'conversation.message.updated': messageUpdated,
  'conversation.message.reactions.updated': reactions,
  'conversation.read_state.updated': readState,
  'conversation.member.added': memberAdded,
  'conversation.member.removed': memberRemoved,
}
