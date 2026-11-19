/** Addresses in the settings area, and where old settings links go now. */
import { currentWorkspaceSlug, spaceSettingsHref, workspaceSettingsPageHref } from '../app/href'

export const ACCOUNT_HOME = '/settings/account/profile'

export function accountSettingsHref(section: string) {
  return `/settings/account/${encodeURIComponent(section)}`
}

/** A page of the workspace's settings, in the workspace in view unless `ws` names another. */
export function workspaceSettingsHref(section: string, ws?: string) {
  return workspaceSettingsPageHref(section, ws ?? currentWorkspaceSlug())
}

/** A space's settings page, in the workspace in view (a space in another one is replaced with its own). */
export function projectSettingsHref(projectId: string, section = 'general') {
  return spaceSettingsHref(projectId, section)
}
