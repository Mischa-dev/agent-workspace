import { PERMISSION_KEYS } from '../../api/generated/permissions'
import { callOp } from '../../api/ops'

/**
 * Every permission a role can hold (AWD-74), generated from the server's
 * catalogue and the modules that own each one (M1.3o).
 */
export { PERMISSION_KEYS }

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
  /** Holders acting on someone's request stay within what that account could do (K1.25). */
  actWithinRequester?: boolean
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
  /** For a level: the kind of thing it is on. Null for a capability. */
  thingKind: string | null
  /** see < comment < suggest < edit_own < edit < manage; a level holds the lower ones. */
  level: 'see' | 'comment' | 'suggest' | 'edit_own' | 'edit' | 'manage' | null
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
  /** May also make or change admin-based roles (the owner tier). */
  canManageAdminRoles?: boolean
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

export const roleKeys = {
  roles: (organizationId: string) => ['organization', organizationId, 'roles'] as const,
  permissions: (organizationId: string, actorId: string | null) =>
    ['organization', organizationId, 'permissions', actorId ?? 'me'] as const,
}

// The operations (K2.17) answer with these shapes; the generated types say
// `string` where these name the keys.
const inWorkspace = (organizationId: string) => ({ workspaceId: organizationId })

export const listRoles = async (organizationId: string) =>
  (await callOp('role_list', {}, inWorkspace(organizationId))) as unknown as RolesResponse

export const createRole = async (organizationId: string, input: RoleInput) =>
  (await callOp(
    'role_create',
    { ...input, agentLimit: input.agentLimit ?? undefined },
    inWorkspace(organizationId),
  )) as unknown as WorkspaceRoleView

export const changeRole = async (
  organizationId: string,
  role: WorkspaceRoleView,
  change: Partial<Omit<RoleInput, 'baseRole'>>,
) => {
  const { agentLimit, ...rest } = change
  return (await callOp(
    'role_update',
    {
      ...rest,
      role: role.id,
      expectedVersion: role.version,
      // null takes the limit away; left out, it stays.
      ...(agentLimit === null ? { clearAgentLimit: true } : agentLimit ? { agentLimit } : {}),
    },
    inWorkspace(organizationId),
  )) as unknown as WorkspaceRoleView
}

export const removeRole = async (organizationId: string, role: WorkspaceRoleView) => {
  await callOp('role_delete', { role: role.id }, inWorkspace(organizationId))
}

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

export const assignRole = async (
  organizationId: string,
  actorId: string,
  input: { roleId: string; applyToProjects: boolean; expectedVersion: number },
) =>
  (await callOp(
    'role_assign',
    { account: actorId, role: input.roleId, applyToProjects: input.applyToProjects, expectedVersion: input.expectedVersion },
    inWorkspace(organizationId),
  )) as unknown as Assignment

export const setAgentLimit = async (organizationId: string, actorId: string, permissions: PermissionKey[] | null) => {
  const set = await callOp(
    'app_limit_set',
    permissions === null ? { account: actorId, lift: true } : { account: actorId, permissions },
    inWorkspace(organizationId),
  )
  return { permissions: (set.permissions ?? null) as PermissionKey[] | null }
}

export const getPermissions = async (organizationId: string, options: { actorId?: string; projectId?: string } = {}) =>
  (await callOp(
    'permissions_get',
    { account: options.actorId, spaceId: options.projectId },
    inWorkspace(organizationId),
  )) as unknown as PermissionsView

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
