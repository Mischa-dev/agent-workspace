import type { ComponentType } from 'react'
import { Navigate, type RouteObject } from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import {
  AuthenticatedRouteBoundary,
  LegacyProjectSettingsRedirect,
  PublicRouteBoundary,
  RootLayout,
  RouteErrorPage,
} from './boundaries'
import {
  AgentsPage,
  ChannelPage,
  CollectionsArea,
  ConversationArchivePage,
  CreateProjectPage,
  DecisionsPage,
  DesignSystemPage,
  DirectConversationPage,
  ForgotPasswordPage,
  FormPage,
  GraphPage,
  HomePage,
  InboxPage,
  JoinPage,
  LaterPage,
  MessagesPage,
  MilestonesPage,
  MyWorkPage,
  NextPage,
  NotFoundPage,
  OnboardingPage,
  PageDetailPage,
  PagesPage,
  PieceGallery,
  PlanImportPage,
  ProjectOverviewPage,
  ProjectRunsPage,
  ProjectsPage,
  QuickAddPage,
  ResetPasswordPage,
  RunPage,
  RunsPage,
  SearchPage,
  SettingsArea,
  SheetsArea,
  SignInPage,
  SuggestionsPage,
  TaskKeyRedirect,
  ThreadPage,
  TimelinePage,
  VerifyEmailPage,
  WhiteboardPage,
  WhiteboardsPage,
  WorkItemDetailPage,
  WorkItemsPage,
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

/** Signed out: these pages need no session. */
const publicRoutes: AppRoute[] = [
  publicPage('/sign-in', SignInPage),
  publicPage('/forgot-password', ForgotPasswordPage),
  publicPage('/reset-password', ResetPasswordPage),
  publicPage('/verify-email', VerifyEmailPage),
  publicPage('/join', JoinPage),
  publicPage('/forms/:token', FormPage),
]

/** Signed in: every page needs the session. Keep paths exactly as they are; `public/sw.js` lists the ones it serves offline. */
const sessionRoutes: AppRoute[] = [
  page('/settings/*', SettingsArea, 'settings'),
  { path: '/', element: <Navigate replace to="/home" />, handle: { space: 'workspace', layout: 'redirect' } },
  page('/home', HomePage, 'home'),
  page('/welcome', OnboardingPage, 'workspace'),
  page('/inbox', InboxPage, 'home'),
  page('/later', LaterPage, 'home'),
  page('/new-task', QuickAddPage, 'workspace'),
  page('/share', QuickAddPage, 'workspace'),
  page('/my-work', MyWorkPage, 'home'),
  page('/runs', RunsPage, 'home'),
  page('/runs/:runId', RunPage, 'home'),
  page('/whiteboards', WhiteboardsPage, 'home'),
  page('/pages', PagesPage, 'workspace'),
  page('/whiteboards/:whiteboardId', WhiteboardPage, 'home', 'canvas'),
  page('/messages', MessagesPage, 'direct'),
  page('/messages/agents', AgentsPage, 'direct'),
  page('/messages/:conversationId', DirectConversationPage, 'direct'),
  page('/projects', ProjectsPage, 'home'),
  page('/projects/new', CreateProjectPage, 'workspace'),
  page('/projects/:projectId', ProjectOverviewPage, 'project'),
  { path: '/projects/:projectId/settings', element: <LegacyProjectSettingsRedirect />, handle: { space: 'project', layout: 'redirect' } },
  page('/projects/:projectId/work-items', WorkItemsPage, 'project'),
  page('/projects/:projectId/work-items/:workItemId', WorkItemDetailPage, 'project'),
  page('/projects/:projectId/milestones', MilestonesPage, 'project'),
  page('/projects/:projectId/next', NextPage, 'project'),
  page('/projects/:projectId/suggestions', SuggestionsPage, 'project'),
  page('/projects/:projectId/graph', GraphPage, 'project'),
  page('/projects/:projectId/plan-import', PlanImportPage, 'project'),
  page('/projects/:projectId/timeline', TimelinePage, 'project'),
  page('/projects/:projectId/launches', ProjectRunsPage, 'project'),
  page('/projects/:projectId/channels/:channelId', ChannelPage, 'project'),
  page('/projects/:projectId/conversations/archived', ConversationArchivePage, 'project'),
  page('/projects/:projectId/threads/:threadId', ThreadPage, 'project'),
  page('/projects/:projectId/pages', PagesPage, 'project'),
  page('/projects/:projectId/pages/:pageId', PageDetailPage, 'project'),
  page('/projects/:projectId/decisions', DecisionsPage, 'project'),
  page('/projects/:projectId/collections/*', CollectionsArea, 'project'),
  page('/projects/:projectId/sheets/*', SheetsArea, 'project'),
  page('/search', SearchPage, 'workspace'),
  page('/design-system', DesignSystemPage, 'workspace'),
  page('/design-system/pieces', PieceGallery, 'workspace'),
  page('/:taskKey', TaskKeyRedirect, 'workspace'),
  page('*', NotFoundPage, 'workspace'),
]

/** The whole app's addresses, as data (W1.10). */
export const appRoutes: RouteObject[] = [
  {
    element: <RootLayout />,
    errorElement: <RouteErrorPage />,
    children: [...publicRoutes, { element: <SessionBoundary />, children: sessionRoutes }],
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
