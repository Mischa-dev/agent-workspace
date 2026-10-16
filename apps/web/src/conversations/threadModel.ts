import { useQuery, useQueryClient } from '@tanstack/react-query'

import { listThreads } from './api'

/** Suggested thread statuses. Any short status works too. */
export const THREAD_STATUSES = ['Open', 'In progress', 'Decided', 'Done', 'Dropped', 'Blocked']

const SETTLED = new Set(['decided', 'done', 'dropped'])

export function isSettled(status: string) {
  return SETTLED.has(status.toLowerCase())
}

export const threadsQueryKey = (projectId: string, conversationId: string) =>
  ['project', projectId, 'threads', conversationId] as const

/** Every cached thread board of one conversation, whichever project holds it. */
export const isThreadsKey = (key: readonly unknown[], conversationId: string) =>
  key[0] === 'project' && key[2] === 'threads' && key[3] === conversationId

export function useThreads(projectId: string, conversationId: string) {
  return useQuery({
    queryKey: threadsQueryKey(projectId, conversationId),
    queryFn: () => listThreads(conversationId),
    enabled: Boolean(projectId && conversationId),
    staleTime: 10_000,
  })
}

/** Refresh every view of a conversation's threads after one changes. */
export function useRefreshThreads() {
  const queryClient = useQueryClient()
  return (conversationId: string) => queryClient.invalidateQueries({ predicate: (query) => isThreadsKey(query.queryKey, conversationId) })
}
