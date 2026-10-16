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

/** The old Project settings tabs (`/projects/:id/settings?tab=`) and the pages that replaced them. */
const LEGACY_PROJECT_TABS: Record<string, string> = {
  general: 'general',
  members: 'members',
  agents: 'agents',
  codex: 'codex-runs',
  repositories: 'repositories',
  github: 'repositories',
}

export function legacyProjectSettingsHref(projectId: string, search: string) {
  const tab = new URLSearchParams(search).get('tab') ?? 'general'
  return projectSettingsHref(projectId, LEGACY_PROJECT_TABS[tab] ?? 'general')
}
