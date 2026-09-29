import { useQuery, useQueryClient } from '@tanstack/react-query'

import { listThreads } from './api'

/** Suggested thread statuses. Any short status works too. */
export const THREAD_STATUSES = ['Open', 'In progress', 'Decided', 'Done', 'Dropped', 'Blocked']

const SETTLED = new Set(['decided', 'done', 'dropped'])

export function isSettled(status: string) {
  return SETTLED.has(status.toLowerCase())
}

export const threadsQueryKey = (conversationId: string) => ['threads', conversationId] as const

export function useThreads(conversationId: string) {
  return useQuery({
    queryKey: threadsQueryKey(conversationId),
    queryFn: () => listThreads(conversationId),
    enabled: Boolean(conversationId),
    staleTime: 10_000,
  })
}

/** Refresh every view of a conversation's threads after one changes. */
export function useRefreshThreads() {
  const queryClient = useQueryClient()
  return (conversationId: string) => queryClient.invalidateQueries({ queryKey: threadsQueryKey(conversationId) })
}
