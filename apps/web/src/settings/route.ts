/** Reading /settings/* addresses. Kept apart from ./paths so the shell's first load doesn't carry it. */
import { ACCOUNT_HOME, accountSettingsHref, projectSettingsHref, workspaceSettingsHref } from './paths'

export type SettingsRoute =
  | { kind: 'redirect'; to: string }
  | { kind: 'page'; group: 'account' | 'workspace'; section: string }
  /** `projectId` is null at /settings/projects: the layout picks one. */
  | { kind: 'page'; group: 'project'; projectId: string | null; section: string }

/** Reads the part of the address after /settings/. */
export function parseSettingsPath(splat: string): SettingsRoute {
  const segments = splat.split('/').filter(Boolean)
  let decoded: string[]
  try {
    decoded = segments.map((segment) => decodeURIComponent(segment))
  } catch {
    return { kind: 'redirect', to: ACCOUNT_HOME }
  }
  const [group, second, third] = decoded
  if (decoded.length === 0) return { kind: 'redirect', to: ACCOUNT_HOME }
  if (group === 'appearance' && decoded.length === 1) return { kind: 'redirect', to: accountSettingsHref('appearance') }
  if (group === 'account') {
    return decoded.length === 2 ? { kind: 'page', group, section: second } : { kind: 'redirect', to: ACCOUNT_HOME }
  }
  if (group === 'workspace') {
    return decoded.length === 2
      ? { kind: 'page', group, section: second }
      : { kind: 'redirect', to: workspaceSettingsHref('general') }
  }
  if (group === 'projects') {
    if (decoded.length === 1) return { kind: 'page', group: 'project', projectId: null, section: 'general' }
    if (decoded.length === 2) return { kind: 'redirect', to: projectSettingsHref(second) }
    if (decoded.length === 3) return { kind: 'page', group: 'project', projectId: second, section: third }
  }
  return { kind: 'redirect', to: ACCOUNT_HOME }
}
