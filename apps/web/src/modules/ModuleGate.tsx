import { useQuery } from '@tanstack/react-query'
import { lazy, Suspense, type ReactNode } from 'react'
import { useLocation, useParams } from 'react-router-dom'

import { requestJson } from '../api/client'
import { useHomeWorkspaceId, useProjectExperience, useWorkspaceExperience } from '../experience/layout'
import { moduleSwitchable } from './ownership'
import { moduleOfPath } from './pageOwners'

// AWD-75: a link into a module that's off opens the turned-off page, not a 404.
// That page loads only when it's needed.
const TurnedOffPage = lazy(() => import('./TurnedOffPage').then((module) => ({ default: module.TurnedOffPage })))

/** Where a board or run is. */
type Owner = { projectId: string | null; organizationId: string }

/**
 * Renders `children`, or the turned-off page when the page's module is off for
 * the person. For a module that can be off, it shows `waiting` until the
 * person's module settings have loaded, so a page that may be off never shows
 * first; the address stays as it is, and the page shows once they say it's
 * on. Where the settings can't load, every module counts as on, as before.
 * A whiteboard's or a run's page follows its own project (or the workspace,
 * for a board across one), not Home's: it waits for the board or run too,
 * and where that can't load the page shows and says so itself. A task key
 * opens its task's project, whose own page decides.
 * This hides pages in the app; the server checks access on its own.
 */
export function ModuleGate({ children, waiting }: { children: ReactNode; waiting: ReactNode }) {
  const { pathname } = useLocation()
  const { projectId, whiteboardId, runId, taskKey } = useParams()
  const homeId = useHomeWorkspaceId()
  // The same request and cache entry as the board's or run's own page
  // (whiteboardKeys.detail, runKeys.detail), so it's asked for once.
  const id = whiteboardId ?? runId ?? ''
  const target = useQuery({
    queryKey: [whiteboardId ? 'whiteboards' : 'runs', 'detail', id],
    queryFn: () =>
      requestJson<Owner & { whiteboard?: Owner }>(`/api/v1/${whiteboardId ? 'whiteboards' : 'agent-runs'}/${encodeURIComponent(id)}`),
    enabled: !!id,
  })
  const shown = target.data?.whiteboard ?? target.data
  const inProject = projectId ?? shown?.projectId
  const project = useProjectExperience(inProject)
  const workspace = useWorkspaceExperience(shown?.organizationId ?? homeId)
  const module = moduleOfPath(pathname)
  const experience = inProject ? project : workspace
  if (module && moduleSwitchable(module) && !taskKey) {
    if (id && !shown) return target.isError ? children : waiting
    if (experience.modules === 'loading') return waiting
    if (!experience.moduleOn(module)) {
      return (
        <Suspense fallback={waiting}>
          <TurnedOffPage module={module} />
        </Suspense>
      )
    }
  }
  return children
}
