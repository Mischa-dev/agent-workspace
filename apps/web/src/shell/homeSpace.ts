import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'

/** Under the inbox's own key, so anything that refreshes the inbox refreshes this too. */
export const inboxSummaryKey = ['inbox', 'summary'] as const

/**
 * The inbox's own unread count, for the Home and Inbox badges. It counts what
 * the Inbox page counts: notifications from every project, DMs and reminders
 * included, so the badges and the page always agree. Null until it loads.
 */
export function useInboxUnread(): number | null {
  const summary = useQuery({
    queryKey: inboxSummaryKey,
    queryFn: () => requestJson<{ unreadCount?: unknown }>('/api/v1/inbox/summary'),
    staleTime: 30_000,
  })
  const count = summary.data?.unreadCount
  return typeof count === 'number' && count > 0 ? count : summary.isSuccess ? 0 : null
}
