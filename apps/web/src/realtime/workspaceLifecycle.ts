import type { QueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { uuid, type RealtimeEnvelope } from './realtimeModel'

/**
 * `workspace.lifecycle.changed`: a workspace was suspended, resumed,
 * scheduled for deletion or kept (H1.24). Refetches exactly what shows the
 * state: the session (each workspace's lifecycle) and the workspace's
 * Settings › General. Answers whether the event was one.
 */
export function workspaceLifecycleChanged(queryClient: QueryClient, envelope: RealtimeEnvelope): boolean {
  if (envelope.type !== 'workspace.lifecycle.changed') return false
  void queryClient.invalidateQueries({ queryKey: queryKeys.session, exact: true })
  const workspaceId = uuid(envelope.payload?.workspaceId)
  if (workspaceId) {
    void queryClient.invalidateQueries({ queryKey: queryKeys.workspace(workspaceId), exact: true })
  }
  return true
}
