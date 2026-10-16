/** Addresses in the settings area, and where old settings links go now. */

export const ACCOUNT_HOME = '/settings/account/profile'

export function accountSettingsHref(section: string) {
  return `/settings/account/${encodeURIComponent(section)}`
}

export function workspaceSettingsHref(section: string) {
  return `/settings/workspace/${encodeURIComponent(section)}`
}

export function projectSettingsHref(projectId: string, section = 'general') {
  return `/settings/projects/${encodeURIComponent(projectId)}/${encodeURIComponent(section)}`
}
