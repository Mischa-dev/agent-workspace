import type { QueryClient, QueryKey } from '@tanstack/react-query'

import type { MessageUpdate } from '../realtime/realtimeModel'
import { REMOVED_MESSAGE_CONTENT } from './sidebarApi'
import { isThreadsKey } from './threadModel'
import type { ConversationMessage, ConversationTimelineEntry, MessageReaction } from './types'

/** How many edited or removed messages the client remembers between refetches. */
export const MAX_MESSAGE_UPDATES = 200

/** The latest edit or removal seen live for each message, by message id. */
export type MessageUpdates = Readonly<Record<string, MessageUpdate>>

export const NO_MESSAGE_UPDATES: MessageUpdates = Object.freeze({})

/**
 * Remember a live update, keeping the most recent `limit` messages. A removal
 * is never forgotten in favour of an older edit, and a repeat changes nothing.
 */
export function rememberMessageUpdate(
  current: MessageUpdates,
  update: MessageUpdate,
  limit = MAX_MESSAGE_UPDATES,
): MessageUpdates {
  const previous = current[update.messageId]
  const merged: MessageUpdate = previous
    ? {
        ...update,
        revision: Math.max(previous.revision, update.revision),
        removed: previous.removed || update.removed,
      }
    : update
  if (previous && previous.revision === merged.revision && previous.removed === merged.removed) {
    return current
  }
  const next: Record<string, MessageUpdate> = { ...current }
  // Re-insert so the most recently changed message is the last to be dropped.
  delete next[update.messageId]
  next[update.messageId] = merged
  const keys = Object.keys(next)
  for (const key of keys.slice(0, Math.max(0, keys.length - limit))) delete next[key]
  return next
}

/** The message as everyone sees it once it's removed: no text, no mentions, no references. */
export function tombstoneMessage(message: ConversationMessage): ConversationMessage {
  return {
    ...message,
    content: REMOVED_MESSAGE_CONTENT,
    plainText: '',
    mentionedActors: [],
    referencedWorkItems: [],
    removed: message.removed ?? { removedAt: '', removedBy: null, via: null },
    reactions: [],
  }
}

export function tombstoneEntry(entry: ConversationTimelineEntry): ConversationTimelineEntry {
  return entry.message ? { ...entry, message: tombstoneMessage(entry.message) } : entry
}

/** The message a reply quotes (AWD-70), or null. */
export function quotedMessageId(message: ConversationMessage): string | null {
  const replyTo = asRecord(message.replyTo)
  return typeof replyTo?.messageId === 'string' ? replyTo.messageId : null
}

/** A reply whose quoted message was removed: the quote says so and keeps none of its text. */
export function withRemovedQuote(message: ConversationMessage): ConversationMessage {
  const replyTo = asRecord(message.replyTo)
  if (!replyTo || (replyTo.removed === true && replyTo.text === null)) return message
  return { ...message, replyTo: { ...replyTo, removed: true, text: null } }
}

/** True when a live update says the message `message` quotes was removed, and its quote still shows it. */
export function quotesRemovedLive(message: ConversationMessage, updates: MessageUpdates, conversationId: string) {
  const quoted = message.removed ? null : quotedMessageId(message)
  const update = quoted ? updates[quoted] : undefined
  return Boolean(update?.removed && update.conversationId === conversationId && withRemovedQuote(message) !== message)
}

/** A message whose reactions changed live, and how many changes were seen. */
export type ReactionUpdate = { conversationId: string; messageId: string; seen: number }

/** Reaction changes seen live, by message id (the latest MAX_MESSAGE_UPDATES messages). */
export type ReactionUpdates = Readonly<Record<string, ReactionUpdate>>

export const NO_REACTION_UPDATES: ReactionUpdates = Object.freeze({})

/** Count one more live reaction change on a message, keeping the most recent `limit` messages. */
export function rememberReactionUpdate(
  current: ReactionUpdates,
  change: { conversationId: string; messageId: string },
  limit = MAX_MESSAGE_UPDATES,
): ReactionUpdates {
  const seen = (current[change.messageId]?.seen ?? 0) + 1
  const next: Record<string, ReactionUpdate> = { ...current }
  delete next[change.messageId]
  next[change.messageId] = { conversationId: change.conversationId, messageId: change.messageId, seen }
  const keys = Object.keys(next)
  for (const key of keys.slice(0, Math.max(0, keys.length - limit))) delete next[key]
  return next
}

/**
 * Entries with one message's reactions replaced. Only reactions change, so a
 * stale copy can never bring back removed text or undo an edit, and a removed
 * message keeps none.
 */
export function withMessageReactions(
  entries: ConversationTimelineEntry[],
  messageId: string,
  reactions: MessageReaction[],
): ConversationTimelineEntry[] {
  let changed = false
  const next = entries.map((entry) => {
    const message = entry.message
    if (message?.id !== messageId || message.removed) return entry
    changed = true
    return { ...entry, message: { ...message, reactions } }
  })
  return changed ? next : entries
}

/** True when `candidate` is a later state of the same message than `current`. */
export function isNewerMessage(candidate: ConversationMessage, current: ConversationMessage) {
  if (candidate.id !== current.id) return false
  // A confirmed removal (who removed it) replaces a removal only seen live.
  if (current.removed) return Boolean(candidate.removed?.removedBy && !current.removed.removedBy)
  if (candidate.removed) return true
  return candidate.revision > current.revision
}

/** True when a live update says the server has something newer than `message`. */
export function needsRefresh(message: ConversationMessage, update: MessageUpdate | undefined) {
  if (!update || update.messageId !== message.id) return false
  if (message.removed) return false
  return update.removed || update.revision > message.revision
}

/** Entries from several pages, one per sequence (later lists win), oldest first. */
export function mergeTimelineEntries(...lists: ConversationTimelineEntry[][]) {
  const bySequence = new Map<number, ConversationTimelineEntry>()
  for (const list of lists) for (const entry of list) bySequence.set(entry.sequence, entry)
  return [...bySequence.values()].sort((left, right) => left.sequence - right.sequence)
}

/**
 * Patch entries with fetched replacements, then blank every message a live
 * update says was removed, and every reply's quote of one. This runs during
 * render, so removed text is gone from the screen before any refetch finishes.
 */
export function applyLiveMessages(
  entries: ConversationTimelineEntry[],
  updates: MessageUpdates,
  replacements: Readonly<Record<string, ConversationTimelineEntry>>,
  conversationId: string,
) {
  let changed = false
  const next = entries.map((entry) => {
    const message = entry.message
    if (!message) return entry
    let current = entry
    const replacement = replacements[message.id]
    if (replacement?.message && isNewerMessage(replacement.message, message)) {
      current = { ...replacement, id: entry.id, sequence: entry.sequence }
    }
    const update = updates[message.id]
    if (update?.removed && update.conversationId === conversationId && !current.message?.removed) {
      current = tombstoneEntry(current)
    }
    // A reply quoting a removed message stops showing its text at once too.
    if (current.message && quotesRemovedLive(current.message, updates, conversationId)) {
      current = { ...current, message: withRemovedQuote(current.message) }
    }
    if (current !== entry) changed = true
    return current
  })
  return changed ? next : entries
}

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as WireRecord) : null
}

/**
 * A cached timeline page (or a thread's, which has the same entries) with one
 * message blanked, and every reply's quote of it.
 */
export function tombstoneInTimelinePage(data: unknown, messageId: string): unknown {
  const page = asRecord(data)
  if (!page || !Array.isArray(page.entries)) return data
  let changed = false
  const entries = (page.entries as ConversationTimelineEntry[]).map((entry) => {
    const message = entry?.message
    if (!message || message.removed) return entry
    if (message.id === messageId) {
      changed = true
      return tombstoneEntry(entry)
    }
    if (quotedMessageId(message) !== messageId) return entry
    const blanked = withRemovedQuote(message)
    if (blanked === message) return entry
    changed = true
    return { ...entry, message: blanked }
  })
  return changed ? { ...page, entries } : data
}

/** A cached message history with every version's text dropped. */
export function redactRevisions(data: unknown): unknown {
  const history = asRecord(data)
  if (!history || !Array.isArray(history.revisions)) return data
  return {
    ...history,
    removed: true,
    revisions: (history.revisions as WireRecord[]).map((revision) => ({
      ...revision,
      content: null,
      plainText: null,
      redacted: true,
    })),
  }
}

/** A cached list (pins or search hits) without the entries pointing at a message. */
export function withoutMessageItems(data: unknown, field: 'pins' | 'hits', messageId: string): unknown {
  const root = asRecord(data)
  if (!root || !Array.isArray(root[field])) return data
  const items = root[field] as WireRecord[]
  const kept = items.filter((item) => !(item?.kind === 'message' && item.id === messageId))
  return kept.length === items.length ? data : { ...root, [field]: kept }
}

/** What the server renames a thread to when its name came from a removed message. */
export const NEUTRAL_THREAD_NAME = 'Thread'

function neutralizeThreadList(list: unknown, messageId: string): unknown {
  if (!Array.isArray(list)) return list
  let changed = false
  const next = (list as WireRecord[]).map((thread) => {
    if (thread?.originMessageId !== messageId || thread.name === NEUTRAL_THREAD_NAME) return thread
    changed = true
    return { ...thread, name: NEUTRAL_THREAD_NAME }
  })
  return changed ? next : list
}

/**
 * Cached threads (a thread board's list, or a sidebar's channels) with every
 * thread started from a removed message showing the neutral name. Its name may
 * repeat the message's first line; the refetch that follows brings back names
 * someone chose themselves.
 */
export function neutralizeThreadNames(data: unknown, messageId: string): unknown {
  if (Array.isArray(data)) return neutralizeThreadList(data, messageId)
  const root = asRecord(data)
  if (!root || !Array.isArray(root.channels)) return data
  let changed = false
  const channels = (root.channels as WireRecord[]).map((channel) => {
    const threads = neutralizeThreadList(channel?.threads, messageId)
    if (threads === channel?.threads) return channel
    changed = true
    return { ...channel, threads }
  })
  return changed ? { ...root, channels } : data
}

function isConversationKey(key: QueryKey, conversationId: string, rest: string) {
  return key[0] === 'project' && key[2] === 'conversation' && key[3] === conversationId && key[4] === rest
}

/** Show one message's reactions in every cached timeline page of its conversation. */
export function setCachedReactions(
  queryClient: QueryClient,
  change: { conversationId: string; messageId: string; reactions: MessageReaction[] },
) {
  queryClient.setQueriesData(
    { predicate: (query) => isConversationKey(query.queryKey, change.conversationId, 'timeline') },
    (data: unknown) => {
      const page = asRecord(data)
      if (!page || !Array.isArray(page.entries)) return data
      const entries = page.entries as ConversationTimelineEntry[]
      const next = withMessageReactions(entries, change.messageId, change.reactions)
      return next === entries ? data : { ...page, entries: next }
    },
  )
}

/** Refetch a conversation's cached queries (its timelines), and nothing else. */
export function invalidateConversation(queryClient: QueryClient, projectId: string, conversationId: string) {
  void queryClient.invalidateQueries({
    predicate: (query) =>
      query.queryKey[0] === 'project' &&
      query.queryKey[1] === projectId &&
      query.queryKey[2] === 'conversation' &&
      query.queryKey[3] === conversationId,
  })
}

/**
 * Blank a removed message in every cache that can hold its text, synchronously:
 * timelines, its history, pins, search results, and the names of threads
 * started from it. Callers then invalidate.
 */
export function dropRemovedMessage(
  queryClient: QueryClient,
  removal: { conversationId: string; messageId: string },
) {
  const { conversationId, messageId } = removal
  queryClient.setQueriesData(
    { predicate: (query) => isConversationKey(query.queryKey, conversationId, 'timeline') },
    (data: unknown) => tombstoneInTimelinePage(data, messageId),
  )
  queryClient.setQueriesData(
    {
      predicate: (query) =>
        isConversationKey(query.queryKey, conversationId, 'message') && query.queryKey[5] === messageId,
    },
    redactRevisions,
  )
  queryClient.setQueriesData({ predicate: (query) => isMemoryKey(query.queryKey, 'pins') }, (data: unknown) =>
    withoutMessageItems(data, 'pins', messageId),
  )
  queryClient.setQueriesData({ queryKey: ['search'] }, (data: unknown) => withoutMessageItems(data, 'hits', messageId))
  // Thread boards and sidebars list threads with an origin message. Only touch
  // caches that change, so the rest keep their freshness.
  const neutralized = (data: unknown) => {
    const next = neutralizeThreadNames(data, messageId)
    return next === data ? undefined : next
  }
  queryClient.setQueriesData({ predicate: (query) => isThreadsKey(query.queryKey, conversationId) }, neutralized)
  queryClient.setQueriesData(
    { predicate: (query) => query.queryKey[0] === 'project' && query.queryKey[2] === 'conversation-sidebar' },
    neutralized,
  )
}

/**
 * The viewer was removed from a group (or left it): nothing they hold of it
 * may stay on screen. Every query in its space is reset, because the group's
 * threads have timelines keyed by their own IDs; active ones refetch, and an
 * open page for the group gets a 404 and leaves. Queries naming the group
 * outside the space, and pins, search and decisions, go too.
 */
export function dropRemovedConversation(
  queryClient: QueryClient,
  projectId: string,
  conversationId: string,
) {
  queryClient.removeQueries({ predicate: (query) => query.queryKey.includes(conversationId) })
  void queryClient.resetQueries({
    predicate: (query) => query.queryKey[0] === 'project' && query.queryKey[1] === projectId,
  })
  refreshMemoryCaches(queryClient)
}

function isMemoryKey(key: QueryKey, kind: 'pins' | 'decisions') {
  return key[0] === 'project' && key[2] === 'memory' && key[3] === kind
}

function quotesMessages(key: QueryKey) {
  return (
    key[0] === 'search' ||
    isMemoryKey(key, 'pins') ||
    isMemoryKey(key, 'decisions') ||
    (key[0] === 'project' && key[2] === 'threads')
  )
}

/**
 * Pins, decisions, thread boards and search results quote messages, and a
 * refresh of one project misses the others' copies. Forget the ones on no
 * screen (they'd show stale text on return) and refetch the rest.
 */
export function refreshMemoryCaches(queryClient: QueryClient) {
  const predicate = (query: { queryKey: QueryKey }) => quotesMessages(query.queryKey)
  queryClient.removeQueries({ predicate, type: 'inactive' })
  void queryClient.invalidateQueries({ predicate })
}
