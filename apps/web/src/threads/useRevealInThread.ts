import { useCallback, useEffect, useRef } from 'react'

import { revealMessage } from '../conversations/quotedMessage'

/**
 * For a thread open beside its conversation (`RevealInThread`): find a
 * message further up the thread by loading earlier pages until it's on
 * screen, then scroll to it. The lookup stops when the message shows, when
 * nothing older is left, when a page fails (its banner says so, and loading
 * again is up to you), when a page brings nothing new, or when the thread
 * changes.
 */
export function useRevealInThread({
  threadId,
  cursor,
  error,
  loading,
  loadOlder,
  olderCount,
}: {
  threadId: string
  cursor: string | null
  error: string | null
  loading: boolean
  loadOlder: () => Promise<void>
  olderCount: number
}) {
  // The message looked for, in which thread, and the cursor of the page last
  // asked for, so a page that changes nothing ends the lookup.
  const seeking = useRef<{ threadId: string; messageId: string; cursor: string } | null>(null)

  const reveal = useCallback(
    (messageId: string) => {
      if (revealMessage(messageId) || !cursor) {
        seeking.current = null
        return
      }
      seeking.current = { threadId, messageId, cursor }
      void loadOlder()
    },
    [cursor, loadOlder, threadId],
  )

  useEffect(() => {
    const pending = seeking.current
    if (!pending || loading) return
    if (pending.threadId !== threadId || revealMessage(pending.messageId) || !cursor || error || cursor === pending.cursor) {
      seeking.current = null
      return
    }
    seeking.current = { ...pending, cursor }
    void loadOlder()
  }, [cursor, error, loadOlder, loading, olderCount, threadId])

  return reveal
}
