import { getProjectConversationSidebar, markConversationSeen } from '../conversations/sidebarApi'
import type { SidebarChannel } from '../conversations/types'
import { getDirectOverview } from '../direct/api'

/**
 * Mark every conversation you can see read: each Project's channels and
 * threads, and your DMs and groups. Each mark goes through the same
 * per-conversation endpoint the pages use, so access is checked as usual.
 * Returns how many conversations changed.
 */

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

/** Runs `work` over `items`, a few at a time. */
async function inBatches<T>(items: T[], size: number, work: (item: T) => Promise<unknown>) {
  const results: PromiseSettledResult<unknown>[] = []
  for (let start = 0; start < items.length; start += size) {
    results.push(...(await Promise.allSettled(items.slice(start, start + size).map(work))))
  }
  return results
}

async function unreadInProject(projectId: string): Promise<Unread[]> {
  const sidebar = await getProjectConversationSidebar(projectId)
  const found = unreadInSidebar(sidebar.channels)
  // The compact list hides quiet threads; ask for the rest channel by channel.
  const crowded = sidebar.channels.filter((channel) => channel.hasMoreThreads)
  const expanded = await inBatches(crowded, 3, async (channel) => {
    const full = await getProjectConversationSidebar(projectId, undefined, false, channel.id)
    return unreadInSidebar(full.channels.filter((candidate) => candidate.id === channel.id))
  })
  for (const result of expanded) {
    if (result.status === 'fulfilled') found.push(...(result.value as Unread[]))
  }
  return found
}

export async function markAllRead(projectIds: string[]): Promise<{ marked: number; failed: number }> {
  const [direct, ...projects] = await Promise.allSettled([getDirectOverview(), ...projectIds.map(unreadInProject)])
  const byId = new Map<string, Unread>()
  if (direct.status === 'fulfilled') {
    for (const conversation of direct.value.conversations) {
      if (conversation.unread) byId.set(conversation.id, { conversationId: conversation.id, highestSequence: conversation.highestSequence })
    }
  }
  let failed = direct.status === 'rejected' ? 1 : 0
  for (const result of projects) {
    if (result.status === 'rejected') failed += 1
    else for (const unread of result.value) byId.set(unread.conversationId, unread)
  }
  const marks = await inBatches([...byId.values()], 6, (unread) =>
    markConversationSeen({ conversationId: unread.conversationId, highestSeenSequence: unread.highestSequence }),
  )
  const markFailures = marks.filter((result) => result.status === 'rejected').length
  return { marked: marks.length - markFailures, failed: failed + markFailures }
}
