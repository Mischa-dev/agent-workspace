import { useQuery } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  getConversationTimeline,
  getProjectConversationSidebar,
} from './api'

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

export function useProjectConversationSidebar(
  projectId: string,
  selectedChannelId = '',
  options: {
    includeArchived?: boolean
    expandedChannelId?: string
  } = {},
) {
  const mode = options.includeArchived
    ? 'archived'
    : options.expandedChannelId
      ? `expanded:${options.expandedChannelId}`
      : 'compact'
  return useQuery({
    queryKey: queryKeys.conversationSidebar(projectId, selectedChannelId, mode),
    queryFn: () =>
      getProjectConversationSidebar(
        projectId,
        selectedChannelId || undefined,
        options.includeArchived,
        options.expandedChannelId,
      ),
    enabled: Boolean(projectId),
    staleTime: 15_000,
    retry: false,
    // Moving between channels changes the key; keep showing this project's
    // last list while the new one loads instead of an empty sidebar.
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[1] === projectId && previousQuery.queryKey[4] === mode
        ? previous
        : undefined,
  })
}
