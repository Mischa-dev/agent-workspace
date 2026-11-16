import { agentKeys } from '../../agents/keys'
import { queryKeys } from '../../api/queryKeys'
import { direct, inbox, payloadId, rail, type Handler } from './touches'

// Membership, notifications and account managers.

/**
 * Someone joined, left or changed role in a space. What anyone may see or do
 * there can change, so this is the one event that refetches the whole space
 * (like K5's `access` frame), and the lists of spaces.
 */
const membershipUpdated: Handler = (event) => [
  { id: `space:${event.projectId}`, filters: { queryKey: queryKeys.project(event.projectId) } },
  { id: 'projects', filters: { queryKey: queryKeys.projectLists } },
  direct,
]

/** The viewer's notifications changed: the inbox and the rail's attention counts. */
const notificationsUpdated: Handler = () => [inbox, rail]

/** Who manages an agent or app account changed: the workspace's settings and the space's agents. */
const managersChanged: Handler = (event) => {
  const workspaceId = payloadId(event, 'workspaceId', 'workspace_id')
  return [
    ...(workspaceId ? [{ id: `workspace:${workspaceId}`, filters: { queryKey: queryKeys.organization(workspaceId) } }] : []),
    { id: `agent-profiles:${event.projectId}`, filters: { queryKey: agentKeys.profiles(event.projectId) } },
    { id: `agents:${event.projectId}`, filters: { queryKey: agentKeys.presence(event.projectId) } },
  ]
}

export const peopleHandlers: Record<string, Handler> = {
  'project.membership.updated': membershipUpdated,
  'notifications.updated': notificationsUpdated,
  'account.managers.changed': managersChanged,
}
