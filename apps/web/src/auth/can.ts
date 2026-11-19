import type { PermissionKey } from '../settings/workspace/roles'

/** Anything the server returns with the caller's permissions there: a workspace in the session, a space, a project. */
export type HoldsPermissions = { permissions?: readonly string[] } | null | undefined

/**
 * Whether the caller holds `permission` where `holder` is (K1.26). The
 * server works permissions out from roles, custom roles included; the web
 * never decides from a role's name.
 */
export function can(holder: HoldsPermissions, permission: PermissionKey): boolean {
  return Boolean(holder?.permissions?.includes(permission))
}
