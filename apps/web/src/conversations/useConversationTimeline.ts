import { useQuery } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { getConversationTimeline } from './api'

/** A conversation's latest messages. Apart from ./hooks so the shell doesn't load the timeline code. */
export function useConversationTimeline(
  projectId: string,
  conversationId: string,
  includeArchived = false,
) {
  return useQuery({
    queryKey: queryKeys.timeline(projectId, conversationId, includeArchived),
    queryFn: () =>
      getConversationTimeline(
        conversationId,
        undefined,
        50,
        includeArchived,
      ),
    enabled: Boolean(projectId && conversationId),
    retry: false,
  })
}
