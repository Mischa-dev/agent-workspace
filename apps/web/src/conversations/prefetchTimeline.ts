// Speed (C2.29): a conversation's latest messages are fetched when a
// pointer rests on its sidebar row or the row takes focus, so opening it
// shows them at once. The timeline code loads only then, so the shell
// stays small; a timeline already cached and fresh isn't fetched again.
import { useQueryClient } from '@tanstack/react-query'
import { useCallback } from 'react'

import { queryKeys } from '../api/queryKeys'

/** How long a prefetched timeline counts as fresh. */
const FRESH_MS = 15_000

export function usePrefetchTimeline() {
  const queryClient = useQueryClient()
  return useCallback(
    (projectId: string, conversationId: string) => {
      if (!projectId || !conversationId) return
      const queryKey = queryKeys.timeline(projectId, conversationId, false)
      const state = queryClient.getQueryState(queryKey)
      if (state?.fetchStatus === 'fetching' || (state?.dataUpdatedAt && Date.now() - state.dataUpdatedAt < FRESH_MS)) return
      void import('./api')
        .then(({ getConversationTimeline }) =>
          queryClient.prefetchQuery({
            queryKey,
            queryFn: () => getConversationTimeline(conversationId, undefined, 50, false),
            staleTime: FRESH_MS,
            retry: false,
          }),
        )
        .catch(() => undefined)
    },
    [queryClient],
  )
}
