import { PERMISSION_KEYS, type PermissionKey } from '../settings/workspace/roles'

const MANAGING: readonly PermissionKey[] = [
  'projects.manage',
  'members.manage',
  'members.groups',
  'roles.manage',
  'settings.manage',
  'settings.audit',
  'modules.manage',
  'modules.build',
  'agents.manage_any',
  'channels.moderate',
  'channels.mention_everyone',
  'hosts.manage',
  'code.manage',
  'webhooks.manage',
  'automations.manage',
  'workspace.delete',
]
const SEEING: readonly PermissionKey[] = [
  'projects.view',
  'tasks.view',
  'code.view',
  'members.view',
  'directory.view',
  'spaces.browse',
  'conversations.see',
  'pages.see',
  'dashboards.view',
]

const BY_ROLE: Record<string, readonly PermissionKey[]> = {
  owner: PERMISSION_KEYS,
  admin: PERMISSION_KEYS.filter((key) => key !== 'workspace.delete'),
  member: PERMISSION_KEYS.filter((key) => !MANAGING.includes(key)),
  viewer: SEEING,
}

/** A built-in role's permissions, near enough to the server's catalogue for fixtures (K1.26). */
export function permissionsOf(role: string): string[] {
  return [...(BY_ROLE[role] ?? [])]
}
