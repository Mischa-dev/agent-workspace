import type { SidebarThread, ThreadSummary } from './types'

/** What the chip under a message knows about its thread, from the board, the sidebar, or both. */
export type ThreadChipFacts = {
  id: string
  name: string
  messageCount?: number
  lastActivityAt?: string | null
  status?: string | null
  outcome?: string | null
  unread?: boolean
  attention?: SidebarThread['attention']
}

/** A thread board row and a sidebar entry, as one set of chip facts. */
export function threadChipFacts(summary: ThreadSummary | undefined, sidebar?: SidebarThread): ThreadChipFacts | null {
  const id = summary?.id ?? sidebar?.id
  if (!id) return null
  return {
    id,
    name: summary?.name ?? sidebar?.name ?? 'Thread',
    messageCount: summary?.messageCount,
    lastActivityAt: summary?.lastActivityAt,
    status: summary?.status,
    outcome: summary?.outcome,
    unread: sidebar ? sidebar.attention === 'unread' || sidebar.highestSequence > sidebar.highestSeenSequence : summary?.unread,
    attention: sidebar?.attention,
  }
}
