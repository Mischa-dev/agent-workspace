import { Suspense, type ReactNode } from 'react'
import { Outlet, useLocation, useRouteError } from 'react-router-dom'

import { BrandMark } from '../auth/AuthLayout'
import { ModuleGate } from '../modules/ModuleGate'
import { reportError } from '../telemetry/reportErrors'
import { Button, PageSkeleton, ToastViewport } from '../ui'
import { AppErrorBoundary } from './AppErrorBoundary'
import { CURRENT_WORKSPACE, workspaceHref } from './href'

/** Around every route: the crash screen and the toasts. */
export function RootLayout() {
  return (
    <RoutedErrorBoundary>
      <Outlet />
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
        <Button onClick={() => window.location.assign(workspaceHref(CURRENT_WORKSPACE))} variant="ghost">
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

/** While a page loads: the shell around it already shows its space, from the route. */
function AuthenticatedRouteFallback() {
  return (
    <div aria-busy="true" className="aw-page">
      <p className="visually-hidden" role="status">
        Loading workspace view.
      </p>
      <PageSkeleton decorative />
    </div>
  )
}

