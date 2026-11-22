import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'
import { modulePart } from './registry'
import type { PartId } from './parts'

/** A built module's sidebar entries as one layout part a switch can hide. */
export type ModulePart = { id: PartId; label: string; description: string }

/**
 * The built modules with sidebar entries, as layout parts (`module.<key>`):
 * in a space, the ones whose entries show there; across the workspace,
 * every one that's on.
 */
export function useModuleParts(workspaceId: string, spaceId?: string): ModulePart[] {
  const list = useQuery({ ...opQuery('module_list', spaceId ? { projectId: spaceId } : {}, { workspaceId }), staleTime: 60_000 })
  return (list.data?.built ?? [])
    .filter((module) => module.on && module.nav.some((entry) => !spaceId || entry.spaceId === spaceId || module.spaceId === spaceId))
    .map((module) => ({
      id: modulePart(module.key),
      label: module.name,
      description: module.nav.length === 1 ? `Its sidebar entry, ${module.nav[0]?.label ?? module.name}.` : `Its ${module.nav.length} sidebar entries.`,
    }))
}
