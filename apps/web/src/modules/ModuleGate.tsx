import { useQuery } from '@tanstack/react-query'
import { lazy, Suspense, type ReactNode } from 'react'
import { useLocation, useParams } from 'react-router-dom'

import { requestJson } from '../api/client'
import { useHomeWorkspaceId, useProjectExperience, useWorkspaceExperience } from '../experience/layout'
import type { WhiteboardDetail } from '../whiteboards/api'
import { moduleSwitchable } from './ownership'
import { moduleOfPath } from './pageOwners'

// AWD-75: a link into a module that's off opens the turned-off page, not a 404.
// That page loads only when it's needed.
const TurnedOffPage = lazy(() => import('./TurnedOffPage').then((module) => ({ default: module.TurnedOffPage })))

/**
 * Renders `children`, or the turned-off page when the page's module is off for
 * the person. For a module that can be off, it shows `waiting` until the
 * person's module settings have loaded, so a page that may be off never shows
 * first; the address stays as it is, and the page shows once they say it's
 * on. Where the settings can't load, every module counts as on, as before.
 * A whiteboard's page follows the board's own project, or its workspace for a
 * board across one, not Home's: it waits for the board too, and where the
 * board can't load the page shows and says so itself.
 * This hides pages in the app; the server checks access on its own.
 */
export function ModuleGate({ children, waiting }: { children: ReactNode; waiting: ReactNode }) {
  const { pathname } = useLocation()
  const { projectId, whiteboardId = '' } = useParams()
  const homeId = useHomeWorkspaceId()
  // The same request and cache entry as the whiteboard's page
  // (whiteboardKeys.detail), so the board is asked for once.
  const board = useQuery({
    queryKey: ['whiteboards', 'detail', whiteboardId],
    queryFn: () => requestJson<WhiteboardDetail>(`/api/v1/whiteboards/${encodeURIComponent(whiteboardId)}`),
    enabled: !!whiteboardId,
  })
  const shown = board.data?.whiteboard
  const inProject = projectId ?? shown?.projectId
  const project = useProjectExperience(inProject)
  const workspace = useWorkspaceExperience(shown?.organizationId ?? homeId)
  const module = moduleOfPath(pathname)
  const experience = inProject ? project : workspace
  if (module && moduleSwitchable(module)) {
    if (whiteboardId && !shown) return <>{board.isError ? children : waiting}</>
    if (experience.modules === 'loading') return <>{waiting}</>
    if (!experience.moduleOn(module)) {
      return (
        <Suspense fallback={waiting}>
          <TurnedOffPage module={module} />
        </Suspense>
      )
    }
  }
  return <>{children}</>
}
