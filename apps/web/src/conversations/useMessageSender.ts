import { useState } from 'react'
import { useQueryClient, type QueryKey } from '@tanstack/react-query'

import { sendConversationMessage, sentEntryFrom } from './api'
import type { ComposerSubmission } from './composerModel'
import type { ConversationKind, ConversationTimelineEntry } from './types'

type CachedTimeline = { entries: ConversationTimelineEntry[]; highestSequence: number }

/** A cached timeline page with one more entry, unless it already has it. */
export function withSentEntry<T extends CachedTimeline>(page: T, entry: ConversationTimelineEntry): T {
  if (page.entries.some((candidate) => candidate.sequence === entry.sequence)) return page
  return {
    ...page,
    entries: [...page.entries, entry].sort((left, right) => left.sequence - right.sequence),
    highestSequence: Math.max(page.highestSequence, entry.sequence),
  }
}

/**
 * Sending in one conversation: each message shows dimmed while it's on its
 * way, then the entry the server returns goes straight into the timeline, so
 * it appears formatted and only once, before any refetch.
 */
export function useMessageSender({
  conversationId,
  kind,
  onSent,
  queryKey,
}: {
  conversationId: string
  kind: ConversationKind
  /** After the message is in, for example to refresh a sidebar. */
  onSent?: () => void
  /** The cached newest page this message belongs at the end of. */
  queryKey: QueryKey
}) {
  const queryClient = useQueryClient()
  // Kept per conversation, so a send still going in one never shows in the next.
  const [pending, setPending] = useState<{ conversationId: string; items: ComposerSubmission[] }>({ conversationId, items: [] })
  const [sent, setSent] = useState(0)

  const send = async (submission: ComposerSubmission) => {
    const target = conversationId
    setSent((count) => count + 1)
    setPending((current) => ({
      conversationId: target,
      items: [...(current.conversationId === target ? current.items : []), submission],
    }))
    try {
      const response = await sendConversationMessage({ conversationId: target, kind, ...submission })
      const entry = sentEntryFrom(response, target)
      if (entry) {
        queryClient.setQueryData<CachedTimeline>(queryKey, (page) => (page ? withSentEntry(page, entry) : page))
        // Read marks, reply requests and link previews follow from the server.
        void queryClient.invalidateQueries({ queryKey, exact: true })
      } else {
        // No entry came back: keep the dimmed copy until the timeline has it.
        await queryClient.invalidateQueries({ queryKey, exact: true })
      }
      onSent?.()
    } finally {
      setPending((current) =>
        current.conversationId === target
          ? { conversationId: target, items: current.items.filter((item) => item.idempotencyKey !== submission.idempotencyKey) }
          : current,
      )
    }
  }

  return {
    /** Messages on their way here, oldest first. */
    pending: pending.conversationId === conversationId ? pending.items : [],
    send,
    /** Goes up with each send, for keeping the timeline on your newest message. */
    sent,
  }
}
