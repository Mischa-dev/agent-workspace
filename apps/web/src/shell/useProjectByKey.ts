import { useProjectSearch, useProjects } from '../projects/hooks'
import type { Project } from '../projects/types'

/**
 * The Project with this key. Keys are unique only within an organization, so
 * the server is always asked too (the rail lists only the first 100), and
 * when several Projects share the key none is picked unless `preferredId` is
 * one of them.
 */
export function useProjectByKey(
  projectKey: string | null,
  preferredId?: string | null,
): { project: Project | null; matches: Project[]; pending: boolean; error: unknown; retry: () => void } {
  const projects = useProjects()
  const wanted = projectKey?.toUpperCase() ?? ''
  const search = useProjectSearch(wanted && projects.isSuccess ? wanted : '')
  const matches: Project[] = []
  for (const project of [...(projects.data?.projects ?? []), ...(search.data?.projects ?? [])]) {
    if (!wanted || project.archivedAt || project.key.toUpperCase() !== wanted) continue
    if (!matches.some((match) => match.id === project.id)) matches.push(project)
  }
  const pending = Boolean(wanted) && (projects.isPending || (projects.isSuccess && search.isPending))
  const error = wanted ? (projects.error ?? search.error) : null
  // Without the server's answer another organization's Project may share the key.
  const project = pending || error
    ? null
    : matches.length === 1
      ? matches[0]
      : matches.find((match) => match.id === preferredId) ?? null
  return {
    project,
    matches: pending ? [] : matches,
    pending,
    error,
    retry: () => {
      void projects.refetch()
      if (wanted) void search.refetch()
    },
  }
}
