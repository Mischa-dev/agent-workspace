import type { QueryClient, QueryKey } from '@tanstack/react-query'

import { queryKeys } from '../../api/queryKeys'
import { refreshMemoryCaches } from '../../conversations/liveMessages'
import type { AccessSets } from './protocol'

const LISTS = new Set(['navigation', 'conversation-sidebar', 'channel-list'])
const inSpace = (key: QueryKey, spaceId: string) => key[0] === 'project' && key[1] === spaceId
const listsConversations = (key: QueryKey) => key[0] === 'project' && typeof key[2] === 'string' && LISTS.has(key[2])

/**
 * What a connection may hear changed (protocol 2's `access` frame): someone
 * joined or left a space, a private conversation or a workspace. What was
 * lost leaves the cache at once, so nothing it said stays on screen; the
 * rail and the lists that name spaces and conversations refetch. Nothing
 * else does: the events that follow say what changed.
 */
export function accessChanged(queryClient: QueryClient, change: { added: AccessSets; removed: AccessSets }) {
  const { added, removed } = change
  for (const spaceId of removed.spaces) queryClient.removeQueries({ predicate: (query) => inSpace(query.queryKey, spaceId) })
  for (const conversationId of removed.conversations) {
    queryClient.removeQueries({ predicate: (query) => query.queryKey.includes(conversationId) })
  }
  const any = (sets: AccessSets) => sets.workspaces.length + sets.spaces.length + sets.conversations.length > 0
  if (!any(added) && !any(removed)) return
  void queryClient.invalidateQueries({ queryKey: queryKeys.projects })
  void queryClient.invalidateQueries({ queryKey: queryKeys.direct })
  if (added.workspaces.length || removed.workspaces.length) void queryClient.invalidateQueries({ queryKey: queryKeys.session })
  if (added.conversations.length || removed.conversations.length) {
    void queryClient.invalidateQueries({ predicate: (query) => listsConversations(query.queryKey) })
  }
  // Search results, pins and decisions may quote what was lost.
  if (removed.conversations.length || removed.spaces.length) refreshMemoryCaches(queryClient)
}
