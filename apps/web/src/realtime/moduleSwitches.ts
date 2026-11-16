import type { QueryClient } from '@tanstack/react-query'

import { layoutKeys } from '../experience/layout'
import { uuid, type RealtimeEnvelope } from './realtimeModel'

/**
 * `project.modules.updated`: a space's own module switches changed (M1.2).
 * Refetches exactly what shows them: the project list (each space's
 * modules off), that space's layout (its sidebar and commands), and the
 * workspace's Settings › Modules. Answers whether the event was one.
 */
export function modulesChanged(queryClient: QueryClient, envelope: RealtimeEnvelope): boolean {
  if (envelope.type !== 'project.modules.updated') return false
  void queryClient.invalidateQueries({ queryKey: ['projects'] })
  void queryClient.invalidateQueries({ queryKey: layoutKeys.project(envelope.projectId), exact: true })
  const workspaceId = uuid(envelope.payload?.workspaceId)
  if (workspaceId) {
    void queryClient.invalidateQueries({ queryKey: ['organization', workspaceId, 'modules'], exact: true })
  }
  return true
}
