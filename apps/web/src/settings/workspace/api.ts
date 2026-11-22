import { requestJson } from '../../api/client'
import { callOp, ranOrThrow } from '../../api/ops'
import type { OrganizationRole } from '../../auth/session'

export type Workspace = {
  id: string
  name: string
  slug: string
  version: number
  createdAt: string
  role: OrganizationRole
  memberCount: number
  /** Missing from servers before people and agents were counted apart. */
  peopleCount?: number
  agentCount?: number
  projectCount: number
  /** Where the workspace is in its life; missing from older servers. */
  lifecycle?: WorkspaceLifecycle
  /** When a deleting workspace goes for good. */
  deleteAfter?: string | null
  capabilities: {
    rename: boolean
    manageMembers: boolean
    transferOwnership: boolean
    leave: boolean
    viewAuditLog: boolean
    /** Holds "Delete the workspace": may schedule its deletion or keep it. */
    delete?: boolean
  }
}

export type WorkspaceLifecycle = 'active' | 'importing' | 'suspended' | 'deleting'

/** Schedule the workspace's deletion in 14 days; `confirm` is its address, typed out. */
export const deleteWorkspace = (workspaceId: string, confirm: string) =>
  callOp('workspace_delete', { confirm }, { workspaceId })

/** Keep a workspace that was going to be deleted. */
export const keepWorkspace = (workspaceId: string) => callOp('workspace_delete_cancel', {}, { workspaceId })

export type WorkspaceMember = {
  actorId: string
  kind: 'human' | 'external_agent' | 'integration'
  displayName: string
  handle: string
  loginName: string | null
  role: OrganizationRole
  /** The custom role they hold, and the name of the role they hold (AWD-74). */
  roleId: string | null
  roleName: string
  /** What apps acting for them may do, when limited; shown to managers and to them. */
  agentLimit: string[] | null
  enabled: boolean
  version: number
  joinedAt: string
  projectCount: number
  isYou: boolean
  /** Whether the workspace can be handed to them (they sign in themselves). */
  mayOwn: boolean
  capabilities: { changeRole: boolean; remove: boolean; assignRole: boolean; limitApps: boolean }
}

export type WorkspaceInvitation = {
  id: string
  role: 'admin' | 'member'
  roleId: string | null
  roleName: string | null
  tokenPrefix: string
  status: 'active' | 'accepted' | 'revoked' | 'expired'
  version: number
  issuedByDisplayName: string
  expiresAt: string
  acceptedByDisplayName: string | null
  acceptedAt: string | null
  createdAt: string
  note: string | null
}

export type AuditActor = { actorId: string; displayName: string; kind: string }
export type AuditProject = { id: string; name: string; key: string }

export type AuditEvent = {
  id: string
  occurredAt: string
  action: string
  outcome: 'succeeded' | 'denied' | 'failed'
  actor: AuditActor | null
  project: AuditProject | null
  app: { channel: string | null; clientName: string | null }
  target: { kind: string; id: string | null }
  metadata: Record<string, unknown>
}

export type AuditFacets = {
  actors: AuditActor[]
  apps: { value: string; label: string }[]
  kinds: { value: string; label: string }[]
  projects: AuditProject[]
}

export type AuditFilters = {
  actorId?: string
  app?: string
  kind?: string
  projectId?: string
  since?: string
  until?: string
}

export type WorkspaceSecret = {
  id: string
  name: string
  description: string | null
  lastFour: string
  createdByName: string | null
  createdAt: string
  updatedAt: string
  lastUsedAt: string | null
  version: number
}

const base = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}`

export const workspaceKeys = {
  workspace: (organizationId: string) => ['organization', organizationId] as const,
  serverVersion: ['server-version'] as const,
  members: (organizationId: string) => ['organization', organizationId, 'members'] as const,
  invitations: (organizationId: string) => ['organization', organizationId, 'invitations'] as const,
  secrets: (organizationId: string) => ['organization', organizationId, 'secrets'] as const,
  auditFacets: (organizationId: string) => ['organization', organizationId, 'audit', 'facets'] as const,
  audit: (organizationId: string, filters: AuditFilters) => ['organization', organizationId, 'audit', 'events', filters] as const,
}

/** Which server build is running: the release script stamps the commit. */
export type ServerVersion = { version: string; commit: string }
export const getServerVersion = () => requestJson<ServerVersion>('/health/version')

export const getWorkspace = (organizationId: string) => requestJson<Workspace>(base(organizationId))

/**
 * Renames the workspace through `workspace_update`, in the workspace's own
 * space, and reads it back whole.
 */
export async function renameWorkspace(organizationId: string, name: string, expectedVersion: number) {
  ranOrThrow(
    await callOp('workspace_update', { projectId: 'workspace', name }, { workspaceId: organizationId, expectedVersion }),
    'Renaming the workspace',
  )
  return getWorkspace(organizationId)
}

export const listWorkspaceMembers = (organizationId: string) =>
  requestJson<{ members: WorkspaceMember[] }>(`${base(organizationId)}/members`)

export const updateWorkspaceMember = (
  organizationId: string,
  member: WorkspaceMember,
  change: { role?: 'admin' | 'member'; enabled?: false },
) =>
  requestJson<WorkspaceMember>(`${base(organizationId)}/members/${encodeURIComponent(member.actorId)}`, {
    method: 'PATCH',
    json: { expectedVersion: member.version, ...change },
  })

export const transferOwnership = (organizationId: string, actorId: string) =>
  requestJson<Workspace>(`${base(organizationId)}/transfer-ownership`, { method: 'POST', json: { actorId } })

export const leaveWorkspace = (organizationId: string) =>
  requestJson<void>(`${base(organizationId)}/leave`, { method: 'POST', json: {} })

export const listWorkspaceInvitations = (organizationId: string) =>
  requestJson<{ invitations: WorkspaceInvitation[] }>(`${base(organizationId)}/invitations`)

/** A new workspace invitation, through `invitation_create`: its join link is shown only now. */
export async function createWorkspaceInvitation(
  organizationId: string,
  input: { role: 'admin' | 'member'; roleId?: string; expiresInHours: number; note?: string },
) {
  const issued = await callOp(
    'invitation_create',
    {
      projectId: 'workspace',
      wholeWorkspace: true,
      role: input.roleId ?? input.role,
      expiresInHours: input.expiresInHours,
      note: input.note,
    },
    { workspaceId: organizationId },
  )
  const { invitation, joinUrl } = ranOrThrow(issued, 'Inviting them')
  return { id: invitation.id, invitationUrl: joinUrl ?? '' }
}

export async function revokeWorkspaceInvitation(organizationId: string, invitation: WorkspaceInvitation) {
  const answer = await callOp(
    'invitation_update',
    { projectId: 'workspace', wholeWorkspace: true, invitation: invitation.id, revoke: true },
    { workspaceId: organizationId, expectedVersion: invitation.version },
  )
  return ranOrThrow(answer, 'Revoking the invitation')
}

export function listAuditEvents(organizationId: string, filters: AuditFilters, before: string | null, limit = 50) {
  const query = new URLSearchParams()
  for (const [name, value] of Object.entries(filters)) if (value) query.set(name, value)
  if (before) query.set('before', before)
  query.set('limit', String(limit))
  return requestJson<{ events: AuditEvent[]; nextCursor: string | null }>(
    `${base(organizationId)}/audit-events?${query.toString()}`,
  )
}

export const getAuditFacets = (organizationId: string) => requestJson<AuditFacets>(`${base(organizationId)}/audit-events/facets`)

export const listWorkspaceSecrets = (organizationId: string) =>
  requestJson<{ secrets: WorkspaceSecret[]; configured: boolean }>(`${base(organizationId)}/secrets`)

export const putWorkspaceSecret = (organizationId: string, name: string, value: string, description: string | null) =>
  requestJson<WorkspaceSecret>(`${base(organizationId)}/secrets/${encodeURIComponent(name)}`, {
    method: 'PUT',
    json: description ? { value, description } : { value },
  })

export const deleteWorkspaceSecret = (organizationId: string, name: string) =>
  requestJson<void>(`${base(organizationId)}/secrets/${encodeURIComponent(name)}`, { method: 'DELETE' })
