import { useLocation } from 'react-router-dom'

import { useProject } from '../projects/hooks'
import type { Project } from '../projects/types'

export const projectPath = (projectId: string, rest = '') => `/projects/${encodeURIComponent(projectId)}${rest}`

/** The Project in the address bar, if any. */
export function useRouteProject(projects: Project[]) {
  const location = useLocation()
  const match = /^\/projects\/([^/]+)/.exec(location.pathname)
  const id = match ? decodeURIComponent(match[1]) : ''
  const inList = projects.find((project) => project.id === id) ?? null
  // The rail holds the first 100 Projects, and may not have loaded; the page's
  // own Project is fetched directly (the page shares this cached read).
  const fetched = useProject(!inList && UUID.test(id) ? id : '')
  return inList ?? (fetched.data && !fetched.data.archivedAt ? fetched.data : null)
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
