import { useQuery, type QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import type { InboxCategory, InboxView } from './api'

// What the rail badge and the realtime connection need. The rest of the inbox
// (api.ts) loads with the page, so it stays out of the initial bundle.

export const inboxKeys = {
  root: ['inbox'] as const,
  summary: ['inbox', 'summary'] as const,
  list: (view: InboxView, category: InboxCategory) => ['inbox', 'list', view, category] as const,
}

/** Which realtime events can change what the inbox shows. */
export function affectsInbox(type: string) {
  return (
    type === 'notifications.updated' ||
    type === 'conversation.message.updated' ||
    type === 'conversation.member.removed' ||
    type === 'project.membership.updated'
  )
}

export function invalidateInbox(queryClient: QueryClient) {
  void queryClient.invalidateQueries({ queryKey: inboxKeys.root })
}

export function useInboxSummary(enabled = true) {
  return useQuery({
    queryKey: inboxKeys.summary,
    queryFn: async () => {
      const value: unknown = await requestJson<unknown>('/api/v1/inbox/summary')
      const unreadCount = value && typeof value === 'object' ? (value as { unreadCount?: unknown }).unreadCount : null
      return { unreadCount: typeof unreadCount === 'number' ? unreadCount : 0 }
    },
    enabled,
    staleTime: 15_000,
  })
}
