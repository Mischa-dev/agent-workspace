import { Suspense, type ReactNode } from 'react'
import { Navigate, Outlet, useLocation, useParams, useRouteError } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { BrandMark } from '../auth/AuthLayout'
import { ModuleGate } from '../modules/ModuleGate'
import { useProjects } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { inHomeSpace } from '../shell/homeSpace'
import { MobileNavigationProvider } from '../shell/MobileNavigationProvider'
import { reportError } from '../telemetry/reportErrors'
import { Button, PageSkeleton, ToastViewport } from '../ui'
import { AppErrorBoundary } from './AppErrorBoundary'

/** Around every route: the crash screen, the phone drawer and the toasts. */
export function RootLayout() {
  return (
    <RoutedErrorBoundary>
      <MobileNavigationProvider>
        <Outlet />
      </MobileNavigationProvider>
      <ToastViewport />
    </RoutedErrorBoundary>
  )
}

/** A crash screen lasts only until you go somewhere else. */
export function RoutedErrorBoundary({ children }: { children: ReactNode }) {
  const { pathname } = useLocation()
  return <AppErrorBoundary resetKey={pathname}>{children}</AppErrorBoundary>
}

/**
 * What the router shows when something fails outside a page's own boundary
 * (the router itself, or a route's data): the same screen as a crash.
 */
export function RouteErrorPage() {
  const error = useRouteError()
  const message = error instanceof Error ? error.message || error.name : 'route error'
  reportError(message, 'router')
  return (
    <main className="aw-boot">
      <div role="alert">
        <h1>Something went wrong on this page</h1>
        <p>Try again, or reload the workspace if it keeps happening. Your messages and running work are safe on the server.</p>
      </div>
      <div className="aw-boot__actions">
        <Button onClick={() => window.location.reload()} variant="primary">
          Reload workspace
        </Button>
        <Button onClick={() => window.location.assign('/home')} variant="ghost">
          Go to Home
        </Button>
      </div>
    </main>
  )
}

export function PublicRouteBoundary({ children }: { children: ReactNode }) {
  return <Suspense fallback={<PublicRouteFallback />}>{children}</Suspense>
}

export function AuthenticatedRouteBoundary({
  children,
}: {
  children: ReactNode
}) {
  return (
    <Suspense fallback={<AuthenticatedRouteFallback />}>
      <ModuleGate waiting={<AuthenticatedRouteFallback />}>{children}</ModuleGate>
    </Suspense>
  )
}

function PublicRouteFallback() {
  return (
    <main aria-busy="true" className="aw-boot">
      <BrandMark />
      <p>Opening…</p>
    </main>
  )
}

function AuthenticatedRouteFallback() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const { pathname } = useLocation()
  const projects = useProjects()
  const project =
    projects.data?.projects.find((candidate) => candidate.id === projectId) ??
    null
  // The same sidebar the page will have, so nothing jumps when it arrives. A
  // whiteboard's canvas has none.
  const home = inHomeSpace(pathname) && !pathname.startsWith('/whiteboards/')
  const direct = pathname === '/messages' || pathname.startsWith('/messages/')

  return (
    <AppShell direct={direct} home={home} mainClassName="aw-main--page" project={project} user={session.user}>
      <div aria-busy="true" className="aw-page">
        <p className="visually-hidden" role="status">
          Loading workspace view.
        </p>
        <PageSkeleton decorative />
      </div>
    </AppShell>
  )
}

/** Project settings moved to /settings/projects/:projectId/:section; the settings area reads the old `?tab=`. */
export function LegacyProjectSettingsRedirect() {
  const { projectId = '' } = useParams()
  const location = useLocation()
  return <Navigate replace to={`/settings/projects/${encodeURIComponent(projectId)}${location.search}`} />
}
