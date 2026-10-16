import { useProjectSearch, useProjects } from '../projects/hooks'
import type { Project } from '../projects/types'

/**
 * The Project with this key. The rail lists only the first 100 Projects, so a
 * key it doesn't hold is looked up on the server.
 */
export function useProjectByKey(projectKey: string | null): { project: Project | null; pending: boolean } {
  const projects = useProjects()
  const wanted = projectKey?.toUpperCase() ?? ''
  const listed = wanted
    ? (projects.data?.projects ?? []).find((project) => project.key.toUpperCase() === wanted) ?? null
    : null
  const searching = Boolean(wanted) && projects.isSuccess && !listed
  const search = useProjectSearch(searching ? wanted : '')
  const found = searching
    ? (search.data?.projects ?? []).find((project) => project.key.toUpperCase() === wanted) ?? null
    : null
  return {
    project: listed ?? found,
    pending: Boolean(wanted) && (projects.isPending || (searching && search.isPending)),
  }
}
