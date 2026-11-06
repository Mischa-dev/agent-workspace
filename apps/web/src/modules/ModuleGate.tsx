import { lazy, Suspense, type ReactNode } from 'react'
import { useLocation, useParams } from 'react-router-dom'

import { useHomeExperience, useProjectExperience } from '../experience/layout'
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
 * This hides pages in the app; the server checks access on its own.
 */
export function ModuleGate({ children, waiting }: { children: ReactNode; waiting: ReactNode }) {
  const { pathname } = useLocation()
  const { projectId } = useParams()
  const project = useProjectExperience(projectId)
  const home = useHomeExperience()
  const module = moduleOfPath(pathname)
  const experience = projectId ? project : home
  if (module && moduleSwitchable(module)) {
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
