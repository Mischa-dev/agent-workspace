import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'
import { modulePart } from './registry'
import type { PartId } from './parts'

/** A built module's sidebar entries as one layout part a switch can hide. */
export type ModulePart = { id: PartId; label: string; description: string }

/**
 * The built modules with sidebar entries, as layout parts (`module.<key>`):
 * in a space, the ones whose entries show there; across the workspace,
 * every workspace-wide one that's on, or with `'everywhere'` (for your own
 * view, which follows you into every space) each reachable space's own too.
 */
export function useModuleParts(workspaceId: string, spaceId?: string | 'everywhere'): ModulePart[] {
  const everywhere = spaceId === 'everywhere'
  const space = everywhere ? undefined : spaceId
  const list = useQuery({ ...opQuery('module_list', space ? { projectId: space } : everywhere ? { everywhere: true } : {}, { workspaceId }), staleTime: 60_000 })
  return (list.data?.built ?? [])
    .filter((module) => module.on && module.nav.some((entry) => !space || entry.spaceId === space || module.spaceId === space))
    .map((module) => ({
      id: modulePart(module.key),
      label: module.name,
      description: module.nav.length === 1 ? `Its sidebar entry, ${module.nav[0]?.label ?? module.name}.` : `Its ${module.nav.length} sidebar entries.`,
    }))
}
