import { useQueries } from '@tanstack/react-query'

import { getEffectiveSettings, settingsKeys, type EffectiveSettings } from '../settings/api'
import { offlineReadingOn } from './offlinePointer'

export type WorkspaceRef = { id: string; name: string }

const allows = (settings: EffectiveSettings) => settings.settings.find((setting) => setting.key === 'offline.allowed')?.value !== false

/**
 * Whether this device may keep the person's reading (W1.31), and which
 * workspaces stop it. The kept copy is the person's, across every workspace
 * they're in: lists such as the inbox, My work and Later mix workspaces, and
 * a cached answer doesn't always say whose it is. So one workspace that
 * doesn't allow reading offline keeps nothing on the device, wherever the
 * person turned it on.
 *
 * `on` is null while any workspace's settings are still unknown (loading, or
 * offline), which changes nothing, unless what is known already says no.
 */
export function offlineGate(
  current: string,
  workspaces: readonly WorkspaceRef[],
  settingsOf: (id: string) => EffectiveSettings | undefined,
): { on: boolean | null; blockedBy: WorkspaceRef[] } {
  const known = workspaces.map((workspace) => ({ workspace, settings: settingsOf(workspace.id) }))
  const blockedBy = known.flatMap(({ workspace, settings }) => (settings && !allows(settings) ? [workspace] : []))
  if (blockedBy.length) return { on: false, blockedBy }
  const mine = settingsOf(current)
  // The person's own switch, once known, being off is enough on its own.
  if (mine && !offlineReadingOn(mine)) return { on: false, blockedBy }
  if (!mine || known.some(({ settings }) => !settings)) return { on: null, blockedBy }
  return { on: offlineReadingOn(mine), blockedBy }
}

/** `offlineGate` over each workspace's effective settings. */
export function useOfflineGate(current: string, workspaces: readonly WorkspaceRef[]) {
  const ids = workspaces.some((workspace) => workspace.id === current) ? workspaces : [...workspaces, { id: current, name: '' }]
  const answers = useQueries({
    queries: ids.map((workspace) => {
      const target = { scope: 'workspace' as const, organizationId: workspace.id }
      return { queryKey: settingsKeys.effective(target), queryFn: () => getEffectiveSettings(target) }
    }),
  })
  const byId = new Map(ids.map((workspace, index) => [workspace.id, answers[index]?.data]))
  return offlineGate(current, ids, (id) => byId.get(id))
}
