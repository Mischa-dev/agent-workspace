import { useMemo, useState } from 'react'

import type { UnreadSummary } from './Timeline'
import type { ConversationTimelineEntry } from './types'

/**
 * Where unread starts in a conversation, for the New line and the "new
 * messages" bar. Channels, DMs, groups and threads all use it.
 *
 * It's worked out once per conversation, from the first answer fetched after
 * the page opened: a cached copy from your last visit says you'd read it all.
 * Then it stays put while the conversation is marked read, so the line doesn't
 * move under you, until you mark something unread here yourself.
 */
export function useFirstUnread({
  conversationId,
  entries,
  fresh,
  hasOlder,
  page,
  viewerId,
}: {
  conversationId: string
  /** Every entry shown, oldest first. */
  entries: ConversationTimelineEntry[]
  /** The page was fetched since the conversation opened, not only read from the cache. */
  fresh: boolean
  /** There are earlier messages than the ones loaded. */
  hasOlder: boolean
  /** The newest page, once it's known to be this conversation's. */
  page: { highestSequence: number; readState: { highestSeenSequence: number } } | null | undefined
  viewerId: string
}) {
  const [state, setState] = useState<{ conversationId: string; sequence: number | null }>({ conversationId: '', sequence: null })
  if (fresh && page && state.conversationId !== conversationId) {
    const seen = page.readState.highestSeenSequence
    setState({ conversationId, sequence: page.highestSequence > seen ? seen + 1 : null })
  }
  const sequence = state.conversationId === conversationId ? state.sequence : null

  const unread = useMemo<UnreadSummary | null>(() => {
    if (sequence == null || entries.length === 0) return null
    const count = entries.filter(
      (entry) => entry.sequence >= sequence && entry.message && !entry.message.removed && entry.message.author.id !== viewerId,
    ).length
    const loadedFrom = entries[0].sequence
    const first = entries.find((entry) => entry.sequence >= sequence)
    return {
      count,
      more: hasOlder && loadedFrom > sequence,
      since: first && loadedFrom <= sequence ? first.createdAt : null,
    }
  }, [entries, hasOlder, sequence, viewerId])

  return {
    firstUnreadSequence: sequence,
    unread,
    /** Mark unread from here: the New line moves to this message. */
    markUnreadFrom: (from: number) => setState({ conversationId, sequence: from }),
  }
}
