import { lazy, Suspense, useState, type ReactNode } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useLocation,
  useNavigate,
  useParams,
} from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import { useAuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import { legacyProjectSettingsHref } from '../settings/paths'
import { BrandMark } from '../auth/AuthLayout'
import { AppShell } from '../shell/AppShell'
import { MobileNavigationProvider } from '../shell/MobileNavigationProvider'
import { ThemeProvider } from '../theme/ThemeProvider'
import { Button, EmptyState, PageSkeleton, ToastViewport } from '../ui'
import { AppErrorBoundary } from './AppErrorBoundary'

const SettingsArea = lazy(() => import('../settings/SettingsArea'))
const SignInPage = lazy(() => import('../auth/SignInPage').then((module) => ({ default: module.SignInPage })))
const JoinPage = lazy(() => import('../invitations/JoinPage').then((module) => ({ default: module.JoinPage })))
const CreateProjectPage = lazy(() => import('../projects/CreateProjectPage').then((module) => ({ default: module.CreateProjectPage })))
const ChannelPage = lazy(() => import('../conversations/ChannelPage').then((module) => ({ default: module.ChannelPage })))
const ConversationArchivePage = lazy(() => import('../conversations/ConversationArchivePage').then((module) => ({ default: module.ConversationArchivePage })))
const MessagesPage = lazy(() => import('../direct/MessagesPage').then((module) => ({ default: module.MessagesPage })))
const AgentsPage = lazy(() => import('../direct/MessagesPage').then((module) => ({ default: module.AgentsPage })))
const DirectConversationPage = lazy(() => import('../direct/DirectConversationPage').then((module) => ({ default: module.DirectConversationPage })))
const HomePage = lazy(() => import('../projects/HomePage').then((module) => ({ default: module.HomePage })))
const ProjectOverviewPage = lazy(() => import('../projects/ProjectOverviewPage').then((module) => ({ default: module.ProjectOverviewPage })))
const ThreadPage = lazy(() => import('../threads/ThreadPage').then((module) => ({ default: module.ThreadPage })))
const WorkItemDetailPage = lazy(() => import('../work-items/WorkItemDetailPage').then((module) => ({ default: module.WorkItemDetailPage })))
const WorkItemsPage = lazy(() => import('../work-items/WorkItemsPage').then((module) => ({ default: module.WorkItemsPage })))
const PagesPage = lazy(() => import('../memory/MemoryPages').then((module) => ({ default: module.PagesPage })))
const PageDetailPage = lazy(() => import('../memory/MemoryPages').then((module) => ({ default: module.PageDetailPage })))
const DecisionsPage = lazy(() => import('../memory/MemoryPages').then((module) => ({ default: module.DecisionsPage })))
const TaskKeyRedirect = lazy(() => import('./TaskKeyRedirect').then((module) => ({ default: module.TaskKeyRedirect })))
// Only development builds include the kit page; production drops the import.
const DevUiPage = import.meta.env.DEV
  ? lazy(() => import('./DevUiPage').then((module) => ({ default: module.DevUiPage })))
  : null
const SearchPage = lazy(() => import('../memory/SearchPage').then((module) => ({ default: module.SearchPage })))

function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        refetchOnWindowFocus: true,
        retry: 1,
      },
      mutations: {
        retry: false,
      },
    },
  })
}

export function App() {
  const [queryClient] = useState(createQueryClient)
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <BrowserRouter>
          <AppErrorBoundary>
            <MobileNavigationProvider>
              <Routes>
                <Route
                  element={
                    <PublicRouteBoundary>
                      <SignInPage />
                    </PublicRouteBoundary>
                  }
                  path="/sign-in"
                />
                <Route
                  element={
                    <PublicRouteBoundary>
                      <JoinPage />
                    </PublicRouteBoundary>
                  }
                  path="/join"
                />
                <Route element={<SessionBoundary />}>
                  <Route path="/settings/*" element={<AuthenticatedRouteBoundary><SettingsArea /></AuthenticatedRouteBoundary>} />
                  <Route element={<Navigate replace to="/home" />} path="/" />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <HomePage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/home"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <MessagesPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/messages"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <AgentsPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/messages/agents"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <DirectConversationPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/messages/:conversationId"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <CreateProjectPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/new"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <ProjectOverviewPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId"
                  />
                  <Route element={<LegacyProjectSettingsRedirect />} path="/projects/:projectId/settings" />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <WorkItemsPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/work-items"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <WorkItemDetailPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/work-items/:workItemId"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <ChannelPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/channels/:channelId"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <ConversationArchivePage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/conversations/archived"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <ThreadPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/threads/:threadId"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <PagesPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/pages"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <PageDetailPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/pages/:pageId"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <DecisionsPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/decisions"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <SearchPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/search"
                  />
                  {DevUiPage ? (
                    <Route element={<AuthenticatedRouteBoundary><DevUiPage /></AuthenticatedRouteBoundary>} path="/dev/ui" />
                  ) : null}
                  <Route element={<AuthenticatedRouteBoundary><TaskKeyRedirect /></AuthenticatedRouteBoundary>} path="/:taskKey" />
                  <Route element={<NotFoundPage />} path="*" />
                </Route>
              </Routes>
            </MobileNavigationProvider>
            <ToastViewport />
          </AppErrorBoundary>
        </BrowserRouter>
      </QueryClientProvider>
    </ThemeProvider>
  )
}

function PublicRouteBoundary({ children }: { children: ReactNode }) {
  return <Suspense fallback={<PublicRouteFallback />}>{children}</Suspense>
}

export function AuthenticatedRouteBoundary({
  children,
}: {
  children: ReactNode
}) {
  return (
    <Suspense fallback={<AuthenticatedRouteFallback />}>
      {children}
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
  const projects = useProjects()
  const project =
    projects.data?.projects.find((candidate) => candidate.id === projectId) ??
    null

  return (
    <AppShell mainClassName="aw-main--page" project={project} user={session.user}>
      <div aria-busy="true" className="aw-page">
        <p className="visually-hidden" role="status">
          Loading workspace view.
        </p>
        <PageSkeleton decorative />
      </div>
    </AppShell>
  )
}

/** Project settings moved to /settings/projects/:projectId/:section; `?tab=` picks the page. */
function LegacyProjectSettingsRedirect() {
  const { projectId = '' } = useParams()
  const location = useLocation()
  return <Navigate replace to={legacyProjectSettingsHref(projectId, location.search)} />
}

function NotFoundPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page" role="alert">
        <EmptyState
          action={
            <Button onClick={() => navigate('/home')} variant="primary">
              Go to Home
            </Button>
          }
          icon="search"
          title="Page not found"
        >
          That address isn’t part of this workspace. It may have moved, or the link may be old.
        </EmptyState>
      </div>
    </AppShell>
  )
}
