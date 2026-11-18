import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { legacyPathOf } from '../app/href'

/** The pages in the Home space: what's yours across every project. */
export const homeSpacePaths = ['/home', '/inbox', '/my-work', '/initiatives', '/later', '/runs', '/whiteboards'] as const

export function inHomeSpace(address: string) {
  const pathname = legacyPathOf(address)
  // All projects is reached from Home; a project's own pages are its space.
  if (pathname === '/projects') return true
  return homeSpacePaths.some((path) => pathname === path || pathname.startsWith(`${path}/`))
}

/** Under the inbox's own key, so anything that refreshes the inbox refreshes this too. */
export const inboxSummaryKey = ['inbox', 'summary'] as const

type InboxSummary = { unreadCount?: unknown; needsYouConversations?: unknown }

function useInboxSummary() {
  return useQuery({
    queryKey: inboxSummaryKey,
    queryFn: () => requestJson<InboxSummary>('/api/v1/inbox/summary'),
    staleTime: 30_000,
  })
}

function countOf(summary: ReturnType<typeof useInboxSummary>, field: keyof InboxSummary) {
  const count = summary.data?.[field]
  return typeof count === 'number' && count > 0 ? count : summary.isSuccess ? 0 : null
}

/**
 * The inbox's own unread count, for the Inbox row. It counts what the Inbox
 * page counts: notifications from every project, DMs and reminders
 * included, so the row and the page always agree. Null until it loads.
 */
export function useInboxUnread(): number | null {
  return countOf(useInboxSummary(), 'unreadCount')
}

/**
 * What needs you, for the tab title, the app badge and the Home tile (C2.4):
 * the places with something that asks something of you, each counted once,
 * as the server counts them. Null until it loads.
 */
export function useNeedsYouBadge(): number | null {
  return countOf(useInboxSummary(), 'needsYouConversations')
}
