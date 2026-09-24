import { lazy, Suspense, useState, type ReactNode } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useNavigate,
  useParams,
} from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import { useAuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { ThemeProvider } from '../theme/ThemeProvider'
import { AppErrorBoundary } from './AppErrorBoundary'

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
const ProjectSettingsPage = lazy(() => import('../projects/ProjectSettingsPage').then((module) => ({ default: module.ProjectSettingsPage })))
const ThreadPage = lazy(() => import('../threads/ThreadPage').then((module) => ({ default: module.ThreadPage })))
const WorkItemDetailPage = lazy(() => import('../work-items/WorkItemDetailPage').then((module) => ({ default: module.WorkItemDetailPage })))
const WorkItemsPage = lazy(() => import('../work-items/WorkItemsPage').then((module) => ({ default: module.WorkItemsPage })))

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
                <Route
                  element={
                    <AuthenticatedRouteBoundary>
                      <ProjectSettingsPage />
                    </AuthenticatedRouteBoundary>
                  }
                  path="/projects/:projectId/settings"
                />
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
                <Route element={<NotFoundPage />} path="*" />
              </Route>
            </Routes>
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
    <main aria-busy="true" className="session-state">
      <div aria-hidden="true" className="brand-mark">A</div>
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
    <AppShell project={project} user={session.user}>
      <div aria-busy="true" className="page-content">
        <p className="visually-hidden" role="status">
          Loading workspace view.
        </p>
        <div aria-hidden="true" className="page-skeleton">
          <span />
          <span />
          <span />
        </div>
      </div>
    </AppShell>
  )
}

function NotFoundPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  return (
    <AppShell user={session.user}>
      <div className="page-content">
        <div className="error-state" role="alert">
          <h1>Page not found</h1>
          <p>The requested location is not part of this workspace.</p>
          <button onClick={() => navigate('/home')} type="button">
            Go to Home
          </button>
        </div>
      </div>
    </AppShell>
  )
}
