import { useCallback, useMemo, useRef, useState } from 'react'
import { Outlet, useMatches } from 'react-router-dom'

import RouteReadyMarks from '../app/routeReady'
import type { RouteHandle } from '../app/routes'
import { useAuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import { ShellFrame, type ShellPresence } from './ShellFrame'
import { mergePresence, samePresence, ShellPresenceContext, type ShellRegistrar } from './shellStore'

/** The deepest route's handle and the project its address names, if any. */
function useRouteSpace(): { space: RouteHandle['space'] | undefined; canvas: boolean; projectId: string | undefined } {
  const matches = useMatches()
  let handle: RouteHandle | undefined
  for (const match of matches) {
    const candidate = match.handle as RouteHandle | undefined
    if (candidate?.space) handle = candidate
  }
  const leaf = matches[matches.length - 1]
  const projectId = (leaf?.params as Record<string, string | undefined> | undefined)?.projectId
  return { space: handle?.space, canvas: handle?.layout === 'canvas', projectId }
}

/**
 * What the route alone says the shell shows (W1.12c): its space's sidebar,
 * so a page that is still loading, or failed, keeps it instead of jumping.
 * A page's own say wins; a canvas (a whiteboard) has no sidebar.
 */
function useRoutePresence(): { presence: ShellPresence; space: RouteHandle['space'] | undefined } {
  const { canvas, projectId, space } = useRouteSpace()
  const projects = useProjects()
  const project = projectId ? (projects.data?.projects.find((candidate) => candidate.id === projectId && !candidate.archivedAt) ?? null) : null
  const presence = useMemo<ShellPresence>(() => {
    if (canvas) return {}
    if (project) return { project }
    if (space === 'home') return { home: true }
    if (space === 'direct') return { direct: true }
    return {}
  }, [canvas, project, space])
  return { presence, space }
}

/**
 * The persistent shell (W1.11): the rail, the sidebar and the panel, mounted
 * once around every shell page. Pages say what they need beside them
 * (`useShellPage`, `useShellSpace`, `useShellPanel`); the route says which
 * space they're in. Moving from page to page changes what the shell draws
 * without remounting it, so the sidebar keeps its place and scroll.
 */
export function ShellLayout() {
  const session = useAuthenticatedSession()
  const registrants = useRef(new Map<string, ShellPresence>())
  const [pagePresence, setPagePresence] = useState<ShellPresence>({})
  const register = useCallback<ShellRegistrar>((id, next) => {
    if (next) registrants.current.set(id, next)
    else registrants.current.delete(id)
    const merged = mergePresence(registrants.current.values())
    setPagePresence((current) => (samePresence(current, merged) ? current : merged))
  }, [])
  const route = useRoutePresence()
  // The page names its space once it has loaded it; until then, the route's.
  const pageNamesSpace = Boolean(pagePresence.project || pagePresence.home || pagePresence.direct)
  const presence = pageNamesSpace ? pagePresence : { ...route.presence, ...pagePresence }
  const outlet = useMemo(() => <Outlet />, [])
  return (
    <ShellPresenceContext.Provider value={register}>
      <ShellFrame {...presence} space={route.space} user={session.user}>
        {outlet}
      </ShellFrame>
      {/* Each page's time to content, for the speed test and telemetry (W1.33). */}
      <RouteReadyMarks />
    </ShellPresenceContext.Provider>
  )
}
