import { useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { useInstantMutation } from '../app/mutations'
import { archiveConversation, conversationVersionFrom, restoreConversation } from './api'
import type { ProjectConversationSidebar } from './types'

/**
 * Archive a channel or thread, leave it, and offer Undo, which restores it
 * and comes back. Failures are toasts; the page stays as it was.
 */
export function useArchiveConversation({
  conversationId,
  kind,
  label,
  leaveTo,
  projectId,
}: {
  conversationId: string
  kind: 'channel' | 'thread'
  /** How it's named in the toast: "#design" or a thread's name. */
  label: string
  /** Where to go once it's archived. */
  leaveTo: string
  projectId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  const here = `${location.pathname}${location.search}`
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
    void queryClient.invalidateQueries({ queryKey: ['project', projectId, 'conversation', conversationId] })
    void queryClient.invalidateQueries({ queryKey: ['project', projectId, 'threads'] })
  }
  return useInstantMutation({
    mutationFn: (expectedVersion: number) => archiveConversation({ conversationId, kind, expectedVersion }),
    errorTitle: `Couldn’t archive ${label}`,
    onSuccess: () => {
      if (kind === 'channel') {
        // Drop the archived channel from every cached sidebar now, so the
        // list stays put (minus this channel) while it refetches.
        queryClient.setQueriesData<ProjectConversationSidebar>(
          { queryKey: queryKeys.conversationSidebarRoot(projectId) },
          (data) => data && { ...data, channels: data.channels.filter((channel) => channel.id !== conversationId) },
        )
      }
      refresh()
      navigate(leaveTo, { replace: true })
    },
    success: (response, expectedVersion) => ({
      id: `archive:${conversationId}`,
      tone: 'success',
      title: `Archived ${label}`,
      description: kind === 'channel' ? 'Its history stays in Archived conversations.' : undefined,
      onUndo: async () => {
        await restoreConversation({ conversationId, kind, expectedVersion: conversationVersionFrom(response, expectedVersion + 1) })
        refresh()
        navigate(here)
      },
    }),
  })
}
