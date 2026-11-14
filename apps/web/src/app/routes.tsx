import { lazy, type ComponentType } from 'react'
import { Navigate, type RouteObject } from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import {
  AuthenticatedRouteBoundary,
  LegacyProjectSettingsRedirect,
  PublicRouteBoundary,
  RootLayout,
  RouteErrorPage,
} from './boundaries'
import type { ModuleRoute } from '../modules/define'
import { WEB_MODULES } from '../modules/registry'
import { ShellLayout } from '../shell/ShellLayout'
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
const moduleRoutes = (key: 'routes' | 'publicRoutes'): AppRoute[] =>
  WEB_MODULES.flatMap((module) => (module[key] ?? []).map(modulePage))

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
 * Signed in: every page needs the session. Keep paths exactly as they are;
 * `public/sw.js` lists the ones it serves offline. The workspace's own pages
 * are here; each module's come from its registration. The router ranks
 * routes by how specific they are, so their order doesn't matter.
 */
const sessionRoutes: AppRoute[] = [
  page('/settings/*', SettingsArea, 'settings'),
  { path: '/', element: <Navigate replace to="/home" />, handle: { space: 'workspace', layout: 'redirect' } },
  page('/home', HomePage, 'home'),
  page('/welcome', OnboardingPage, 'workspace'),
  { path: '/projects/:projectId/settings', element: <LegacyProjectSettingsRedirect />, handle: { space: 'project', layout: 'redirect' } },
  page('/search', SearchPage, 'workspace'),
  page('/design-system', DesignSystemPage, 'workspace'),
  page('/design-system/pieces', PieceGallery, 'workspace'),
  ...moduleRoutes('routes'),
  page('*', NotFoundPage, 'workspace'),
]

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
          // W1.11: every shell page inside one persistent shell, so the rail
          // and sidebar stay mounted from page to page.
          { element: <ShellLayout />, children: sessionRoutes.filter((route) => route.handle?.layout === 'shell') },
          ...sessionRoutes.filter((route) => route.handle?.layout !== 'shell'),
        ],
      },
    ],
  },
]

/** Every route path, in table order: what the module guards read. */
export function routePaths(routes: RouteObject[] = appRoutes): string[] {
  return routes.flatMap((route) => [
    ...(route.path ? [route.path] : []),
    ...routePaths(route.children ?? []),
  ])
}

/** Every route that has a path, with its handle. */
export function routesWithHandles(routes: RouteObject[] = appRoutes): AppRoute[] {
  return routes.flatMap((route) => [
    ...(route.path ? [route as AppRoute] : []),
    ...routesWithHandles(route.children ?? []),
  ])
}
