import type { QueryClient, QueryKey } from '@tanstack/react-query'

import type { MessageUpdate } from '../realtime/realtimeModel'
import { REMOVED_MESSAGE_CONTENT } from './api'
import { threadsQueryKey } from './threadModel'
import type { ConversationMessage, ConversationTimelineEntry } from './types'

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
  }
}

export function tombstoneEntry(entry: ConversationTimelineEntry): ConversationTimelineEntry {
  return entry.message ? { ...entry, message: tombstoneMessage(entry.message) } : entry
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
 * update says was removed. This runs during render, so removed text is gone
 * from the screen before any refetch finishes.
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
    if (current !== entry) changed = true
    return current
  })
  return changed ? next : entries
}

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as WireRecord) : null
}

/** A cached timeline page (or a thread's, which has the same entries) with one message blanked. */
export function tombstoneInTimelinePage(data: unknown, messageId: string): unknown {
  const page = asRecord(data)
  if (!page || !Array.isArray(page.entries)) return data
  let changed = false
  const entries = (page.entries as ConversationTimelineEntry[]).map((entry) => {
    if (entry?.message?.id !== messageId || entry.message.removed) return entry
    changed = true
    return tombstoneEntry(entry)
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
  queryClient.setQueriesData({ queryKey: ['pins'] }, (data: unknown) => withoutMessageItems(data, 'pins', messageId))
  queryClient.setQueriesData({ queryKey: ['search'] }, (data: unknown) => withoutMessageItems(data, 'hits', messageId))
  // Thread boards and sidebars list threads with an origin message. Only touch
  // caches that change, so the rest keep their freshness.
  const neutralized = (data: unknown) => {
    const next = neutralizeThreadNames(data, messageId)
    return next === data ? undefined : next
  }
  queryClient.setQueriesData({ queryKey: threadsQueryKey(conversationId) }, neutralized)
  queryClient.setQueriesData(
    { predicate: (query) => query.queryKey[0] === 'project' && query.queryKey[2] === 'conversation-sidebar' },
    neutralized,
  )
}

/**
 * Pins, search results, decisions and thread boards are cached outside a
 * project's queries, so a project refresh misses them. Forget the ones on no
 * screen (they'd show stale text on return) and refetch the rest.
 */
export function refreshMemoryCaches(queryClient: QueryClient) {
  for (const queryKey of [['pins'], ['search'], ['decisions'], ['threads']]) {
    queryClient.removeQueries({ queryKey, type: 'inactive' })
    void queryClient.invalidateQueries({ queryKey })
  }
}
