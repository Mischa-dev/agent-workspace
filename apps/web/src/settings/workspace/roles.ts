import { requestJson } from '../../api/client'

/**
 * Every permission a role can hold (AWD-74). The server's catalogue
 * (`policy::PERMISSIONS`) is the source of truth and comes with the roles;
 * this list types it. A new permission adds its key here too.
 */
export const PERMISSION_KEYS = [
  'projects.view',
  'projects.create',
  'projects.manage',
  'tasks.view',
  'tasks.create',
  'tasks.edit',
  'tasks.delete',
  'tasks.workflow',
  'channels.post',
  'channels.create',
  'channels.moderate',
  'pages.edit',
  'whiteboards.edit',
  'collections.edit',
  'collections.build',
  'sheets.edit',
  'runs.launch',
  'runs.approve',
  'runs.interrupt',
  'runs.accounts',
  'code.view',
  'code.publish',
  'code.manage',
  'agents.manage',
  'agents.manage_any',
  'members.view',
  'directory.view',
  'members.invite',
  'members.manage',
  'roles.manage',
  'settings.manage',
] as const

export type PermissionKey = (typeof PERMISSION_KEYS)[number]
export type BaseRole = 'owner' | 'admin' | 'member' | 'viewer'

export type RoleDefaults = {
  layoutPreset?: string
  channels?: string[]
  pinnedViews?: string[]
  homeHidden?: string[]
}

export type WorkspaceRoleView = {
  id: string
  key: string
  builtIn: boolean
  baseRole: BaseRole
  name: string
  description: string
  permissions: PermissionKey[]
  /** What apps and agents acting for holders may do; null is the same as the holder. */
  agentLimit: PermissionKey[] | null
  defaults: RoleDefaults
  /** Modules holders don't get, on top of what the workspace has off (AWD-75). */
  modulesOff: string[]
  isDefault: boolean
  version: number
  updatedAt: string
  workspaceHolders: number
  projectHolders: number
  /** What you may do with it. `assign` gives it to a member; `invite` invites someone to it. */
  capabilities: { edit: boolean; editPermissions: boolean; remove: boolean; assign: boolean; invite: boolean }
}

export type PermissionInfo = {
  key: PermissionKey
  module: string
  label: string
  meaning: string
  scope: 'project' | 'workspace' | 'both'
  builtIn: BaseRole[]
  peopleOnly: boolean
  required: boolean
}

export type StartingPoint = {
  key: string
  name: string
  description: string
  baseRole: Exclude<BaseRole, 'owner'>
  permissions: PermissionKey[]
  agentLimit: PermissionKey[] | null
  defaults: RoleDefaults
}

export type RolesResponse = {
  roles: WorkspaceRoleView[]
  modules: { key: string; name: string }[]
  permissions: PermissionInfo[]
  startingPoints: StartingPoint[]
  canManage: boolean
  yourPermissions: PermissionKey[]
}

export type RoleInput = {
  name: string
  description: string
  baseRole: Exclude<BaseRole, 'owner'>
  permissions: PermissionKey[]
  agentLimit: PermissionKey[] | null
  defaults: RoleDefaults
  modulesOff: string[]
  isDefault: boolean
}

export type ScopePermissions = {
  role: string
  roleId: string | null
  roleName: string
  permissions: PermissionKey[]
  forApps: PermissionKey[]
}

export type PermissionsView = {
  actorId: string
  workspace: ScopePermissions
  project: ScopePermissions | null
  personalAgentLimit: PermissionKey[] | null
}

const base = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}`

export const roleKeys = {
  roles: (organizationId: string) => ['organization', organizationId, 'roles'] as const,
  permissions: (organizationId: string, actorId: string | null) =>
    ['organization', organizationId, 'permissions', actorId ?? 'me'] as const,
}

export const listRoles = (organizationId: string) => requestJson<RolesResponse>(`${base(organizationId)}/roles`)

export const createRole = (organizationId: string, input: RoleInput) =>
  requestJson<WorkspaceRoleView>(`${base(organizationId)}/roles`, { method: 'POST', json: input })

export const changeRole = (
  organizationId: string,
  role: WorkspaceRoleView,
  change: Partial<Omit<RoleInput, 'baseRole'>>,
) =>
  requestJson<WorkspaceRoleView>(`${base(organizationId)}/roles/${encodeURIComponent(role.id)}`, {
    method: 'PATCH',
    json: { expectedVersion: role.version, ...change },
  })

export const removeRole = (organizationId: string, role: WorkspaceRoleView) =>
  requestJson<void>(`${base(organizationId)}/roles/${encodeURIComponent(role.id)}`, { method: 'DELETE' })

export type Assignment = {
  actorId: string
  role: 'admin' | 'member'
  roleId: string | null
  roleName: string
  projectsChanged: number
  projectsSkipped: number
  // Each project left as it was, and why.
  skippedProjects: { projectId: string; reason: string }[]
}

export const assignRole = (
  organizationId: string,
  actorId: string,
  input: { roleId: string; applyToProjects: boolean; expectedVersion: number },
) =>
  requestJson<Assignment>(`${base(organizationId)}/members/${encodeURIComponent(actorId)}/role`, {
    method: 'PUT',
    json: input,
  })

export const setAgentLimit = (organizationId: string, actorId: string, permissions: PermissionKey[] | null) =>
  requestJson<{ permissions: PermissionKey[] | null }>(
    `${base(organizationId)}/members/${encodeURIComponent(actorId)}/agent-limit`,
    { method: 'PUT', json: { permissions } },
  )

export function getPermissions(organizationId: string, options: { actorId?: string; projectId?: string } = {}) {
  const query = new URLSearchParams()
  if (options.actorId) query.set('actorId', options.actorId)
  if (options.projectId) query.set('projectId', options.projectId)
  const suffix = query.toString() ? `?${query.toString()}` : ''
  return requestJson<PermissionsView>(`${base(organizationId)}/permissions${suffix}`)
}

/** The roles someone can be given in the workspace: no owner (that's Transfer ownership) and no plain viewer. */
export function workspaceAssignable(roles: WorkspaceRoleView[]) {
  return roles.filter((role) => role.key !== 'owner' && !(role.builtIn && role.key === 'viewer'))
}

/** The roles someone can be given in a project. */
export function projectAssignable(roles: WorkspaceRoleView[]) {
  return roles.filter((role) => role.key !== 'owner')
}

/** A role's name for a membership: its custom role's name, or the built-in one's. */
export function roleNameFor(roles: WorkspaceRoleView[] | undefined, role: string, roleId: string | null) {
  const found = roles?.find((entry) => (roleId ? entry.id === roleId : entry.builtIn && entry.key === role))
  return found?.name ?? role.charAt(0).toUpperCase() + role.slice(1)
}

/** The value a role picker uses for a membership: the custom role's ID, or the built-in role's. */
export function pickerValue(roles: WorkspaceRoleView[] | undefined, role: string, roleId: string | null) {
  if (roleId) return roleId
  return roles?.find((entry) => entry.builtIn && entry.key === role)?.id ?? role
}
