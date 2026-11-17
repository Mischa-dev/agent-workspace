import { callOp, newRequestId } from '../api/ops'
import type { SidebarChannel } from '../conversations/types'

type Unread = { conversationId: string; highestSequence: number }

/** The channels and threads in a sidebar that have something you haven't seen. */
export function unreadInSidebar(channels: SidebarChannel[]): Unread[] {
  const found: Unread[] = []
  for (const channel of channels) {
    if (channel.highestSequence > channel.highestSeenSequence) {
      found.push({ conversationId: channel.id, highestSequence: channel.highestSequence })
    }
    for (const thread of channel.threads) {
      if (thread.highestSequence > thread.highestSeenSequence) {
        found.push({ conversationId: thread.id, highestSequence: thread.highestSequence })
      }
    }
  }
  return found
}

/**
 * Mark every conversation you can read in these workspaces read, through
 * the `conversations_mark_read` operation (C2.2): the server picks what you
 * may read and reads it the same way opening each one does. A workspace
 * with more than one call marks is asked again.
 */
export async function markAllRead(workspaceIds: string[]): Promise<{ marked: number; failed: number }> {
  let marked = 0
  let failed = 0
  for (const workspaceId of new Set(workspaceIds)) {
    try {
      for (let round = 0; round < 20; round += 1) {
        const output = await callOp('conversations_mark_read', {}, { workspaceId, requestId: newRequestId() })
        marked += output.marked
        if (!output.more) break
      }
    } catch {
      failed += 1
    }
  }
  return { marked, failed }
}
