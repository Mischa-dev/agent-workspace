import { lazy, type ComponentType } from 'react'
import { Navigate, type RouteObject } from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import {
  AuthenticatedRouteBoundary,
  PublicRouteBoundary,
  RootLayout,
  RouteErrorPage,
} from './boundaries'
import { isWorkspacePath, workspaceRoutePath } from './href'
import type { ModuleRoute, WebModule } from '../modules/define'
import { WEB_MODULES } from '../modules/registry'
import { ShellLayout } from '../shell/ShellLayout'
import { ThingRedirect } from './ThingRedirect'
import { WorkspaceBoundary, WorkspaceProvider, WorkspaceRedirect } from './WorkspaceBoundary'
import {
  DesignSystemPage,
  ForgotPasswordPage,
  HomePage,
  JoinPage,
  NotFoundPage,
  OnboardingPage,
  PieceGallery,
  ResetPasswordPage,
  SearchPage,
  SettingsArea,
  SignInPage,
  VerifyEmailPage,
} from './pages'


/**
 * What every route says about itself. `space` is the space its pages belong
 * to (what the rail highlights and the shell draws beside it); `layout` is
 * how it's framed: in the shell, on a full-screen canvas, as a public page
 * or as a redirect.
 */
export type RouteHandle = {
  space: 'home' | 'workspace' | 'project' | 'direct' | 'settings' | 'none'
  layout: 'shell' | 'canvas' | 'public' | 'redirect'
}

type AppRoute = RouteObject & { handle?: RouteHandle }

function publicPage(path: string, Page: ComponentType): AppRoute {
  return {
    path,
    element: (
      <PublicRouteBoundary>
        <Page />
      </PublicRouteBoundary>
    ),
    handle: { space: 'none', layout: 'public' },
  }
}

function page(
  path: string,
  Page: ComponentType,
  space: RouteHandle['space'],
  layout: RouteHandle['layout'] = 'shell',
): AppRoute {
  return {
    path,
    element: (
      <AuthenticatedRouteBoundary>
        <Page />
      </AuthenticatedRouteBoundary>
    ),
    handle: { space, layout },
  }
}

/** A module's page (M1.4), loaded when first shown. */
function modulePage(route: ModuleRoute): AppRoute {
  const Page = lazy(route.page)
  return route.layout === 'public' ? publicPage(route.path, Page) : page(route.path, Page, route.space, route.layout)
}

/** Every module's pages, from their registrations (`<folder>/module.ts`). */
export const moduleRoutes = (key: 'routes' | 'publicRoutes', modules: readonly WebModule[] = WEB_MODULES): AppRoute[] =>
  modules.flatMap((module) => (module[key] ?? []).map(modulePage))

/** Signed out: these pages need no session. */
const publicRoutes: AppRoute[] = [
  publicPage('/sign-in', SignInPage),
  publicPage('/forgot-password', ForgotPasswordPage),
  publicPage('/reset-password', ResetPasswordPage),
  publicPage('/verify-email', VerifyEmailPage),
  publicPage('/join', JoinPage),
  ...moduleRoutes('publicRoutes'),
]

/**
 * Signed in: every page needs the session. A workspace's own pages are listed
 * here by their old root paths and served under `/w/:ws` (W1.14a), a
 * space's under `/w/:ws/s/:projectId` (W1.14b, `/projects/:projectId/work-items`
 * becoming `/s/:projectId/tasks`; app/href.ts has the words), the old path
 * redirecting there; `public/sw.js` lists the first segments it serves
 * offline. The workspace's own pages are here; each module's come from its
 * registration. The router ranks routes by how specific they are, so their
 * order doesn't matter.
 */
const sessionRoutes: AppRoute[] = [
  // Account settings, and old workspace settings links (they redirect).
  page('/settings/*', SettingsArea, 'settings'),
  page('/home', HomePage, 'home'),
  page('/welcome', OnboardingPage, 'workspace'),
  page('/search', SearchPage, 'workspace'),
  page('/design-system', DesignSystemPage, 'workspace'),
  page('/design-system/pieces', PieceGallery, 'workspace'),
  ...moduleRoutes('routes'),
  page('*', NotFoundPage, 'workspace'),
]

const inWorkspace = (route: AppRoute) => Boolean(route.path && isWorkspacePath(route.path))
/** In the persistent shell: shell pages and canvases (a canvas draws the rail but no sidebar). */
const framed = (route: AppRoute) => route.handle?.layout === 'shell' || route.handle?.layout === 'canvas'

const redirect = (path: string, space: RouteHandle['space'] = 'workspace'): AppRoute => ({
  path,
  element: <WorkspaceRedirect />,
  handle: { space, layout: 'redirect' },
})

/**
 * The old root address of each workspace page, and `/`: the same page in
 * the current workspace (a space's in its own workspace). Any old space
 * address, and old workspace and space settings links, too. W1.14d deletes
 * them once the server's links have moved.
 */
const workspaceRedirects: AppRoute[] = [
  redirect('/'),
  redirect('/projects/:projectId/*', 'project'),
  redirect('/settings/workspace/*', 'settings'),
  redirect('/settings/projects/*', 'settings'),
  ...sessionRoutes.filter(inWorkspace).map((route) => ({
    path: route.path,
    element: <WorkspaceRedirect />,
    handle: { space: route.handle?.space ?? 'workspace', layout: 'redirect' as const },
  })),
]

/** `/w/:ws` and the workspace pages under it, in the shell or not (redirects). */
function workspaceRoutes(shell: boolean): AppRoute {
  const children: AppRoute[] = sessionRoutes
    .filter((route) => inWorkspace(route) && framed(route) === shell)
    .map((route) => ({ ...route, path: workspaceRoutePath(route.path ?? '').slice(1) }))
  if (shell) {
    children.unshift({ index: true, element: <Navigate replace to="home" />, handle: { space: 'home', layout: 'redirect' } })
    // The workspace's settings and its spaces' (`settings/spaces/:id/...`), and any thing by its permanent address.
    children.push(page('settings/*', SettingsArea, 'settings'), { ...page('t/:kind/:id', ThingRedirect, 'workspace') })
  }
  return { path: '/w/:ws', element: <WorkspaceBoundary />, children }
}

const atRoot = sessionRoutes.filter((route) => !inWorkspace(route))
/** Workspace pages outside the shell (redirects), if any. */
const offShell = workspaceRoutes(false)

/** The whole app's addresses, as data (W1.10). */
export const appRoutes: RouteObject[] = [
  {
    element: <RootLayout />,
    errorElement: <RouteErrorPage />,
    children: [
      ...publicRoutes,
      {
        element: <SessionBoundary />,
        children: [
          {
            element: <WorkspaceProvider />,
            children: [
              // W1.11: every shell page inside one persistent shell, so the rail
              // and sidebar stay mounted from page to page.
              { element: <ShellLayout />, children: [...atRoot.filter(framed), workspaceRoutes(true)] },
              ...atRoot.filter((route) => !framed(route)),
              ...(offShell.children?.length ? [offShell] : []),
              ...workspaceRedirects,
            ],
          },
        ],
      },
    ],
  },
]

const joined = (parent: string, path: string) => (path.startsWith('/') || !parent ? path : `${parent.replace(/\/$/, '')}/${path}`)

/** Every page's route, with its whole path, in table order: what the module guards read. Layout routes (no page of their own) are left out. */
export function routesWithHandles(routes: RouteObject[] = appRoutes, parent = ''): AppRoute[] {
  return routes.flatMap((route): AppRoute[] => {
    const path = route.path ? joined(parent, route.path) : parent
    if (route.children?.length) return routesWithHandles(route.children, path)
    return route.path || route.index ? [{ ...(route as AppRoute), path }] : []
  })
}

/** Every page's whole path, in table order. */
export function routePaths(routes: RouteObject[] = appRoutes): string[] {
  return routesWithHandles(routes).map((route) => route.path ?? '')
}
