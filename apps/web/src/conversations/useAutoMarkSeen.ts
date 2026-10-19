import { useEffect, useRef, useState } from 'react'

import { markConversationSeen } from './api'

/** The newest sequence a page has shown for one conversation. */
export interface ShownSequence {
  conversationId: string
  sequence: number
}

export interface AutoMarkInput {
  conversationId: string
  highestSequence: number
  highestSeenSequence: number
  /** The page's own conditions, such as "the newest entry is loaded". */
  ready: boolean
}

/**
 * Whether to mark a conversation seen, and what the page has shown since.
 *
 * A page marks seen only when it shows something new: the first time it
 * shows a conversation, or when the newest sequence grows past what it has
 * shown. A read mark moved back while the page is open (Mark unread here, in
 * another tab, or through MCP) therefore stays until a new message arrives or
 * the conversation is opened again, instead of being undone at once.
 */
export function decideMarkSeen(
  shown: ShownSequence | null,
  input: AutoMarkInput,
): { mark: number | null; shown: ShownSequence | null } {
  if (!input.ready) return { mark: null, shown }
  if (
    shown !== null &&
    shown.conversationId === input.conversationId &&
    input.highestSequence <= shown.sequence
  ) {
    return { mark: null, shown }
  }
  return {
    mark: input.highestSequence > input.highestSeenSequence ? input.highestSequence : null,
    shown: { conversationId: input.conversationId, sequence: input.highestSequence },
  }
}

/** Whether the tab is on screen. A conversation left open in a hidden tab isn't being read. */
function useTabVisible() {
  const read = () => typeof document === 'undefined' || document.visibilityState !== 'hidden'
  const [visible, setVisible] = useState(read)
  useEffect(() => {
    const update = () => setVisible(read())
    document.addEventListener('visibilitychange', update)
    return () => document.removeEventListener('visibilitychange', update)
  }, [])
  return visible
}

/**
 * Mark the conversation a page shows as seen, following `decideMarkSeen`,
 * only while its tab is on screen: messages that arrive in a hidden tab stay
 * unread until you come back to it. `onMarked` runs after a mark is saved,
 * for example to refresh a sidebar.
 */
export function useAutoMarkSeen(input: AutoMarkInput, onMarked: () => unknown) {
  const shown = useRef<ShownSequence | null>(null)
  const visible = useTabVisible()
  const { conversationId, highestSequence, highestSeenSequence } = input
  const ready = input.ready && visible
  useEffect(() => {
    const previous = shown.current
    const decision = decideMarkSeen(previous, {
      conversationId,
      highestSequence,
      highestSeenSequence,
      ready,
    })
    shown.current = decision.shown
    if (decision.mark === null) return
    void markConversationSeen({ conversationId, highestSeenSequence: decision.mark })
      .then(() => onMarked())
      .catch(() => {
        // Try again the next time the page renders this conversation.
        if (shown.current?.conversationId === conversationId) shown.current = previous
      })
  }, [conversationId, highestSequence, highestSeenSequence, onMarked, ready])
}
