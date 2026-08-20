import { lazy, Suspense, useState } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useNavigate,
} from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { ThemeProvider } from '../theme/ThemeProvider'
import { AppErrorBoundary } from './AppErrorBoundary'

const SignInPage = lazy(() => import('../auth/SignInPage').then((module) => ({ default: module.SignInPage })))
const JoinPage = lazy(() => import('../invitations/JoinPage').then((module) => ({ default: module.JoinPage })))
const CreateProjectPage = lazy(() => import('../projects/CreateProjectPage').then((module) => ({ default: module.CreateProjectPage })))
const ChannelPage = lazy(() => import('../conversations/ChannelPage').then((module) => ({ default: module.ChannelPage })))
const ConversationArchivePage = lazy(() => import('../conversations/ConversationArchivePage').then((module) => ({ default: module.ConversationArchivePage })))
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
            <Suspense fallback={<RouteFallback />}>
              <Routes>
          <Route element={<SignInPage />} path="/sign-in" />
          <Route element={<JoinPage />} path="/join" />
          <Route element={<SessionBoundary />}>
            <Route element={<Navigate replace to="/home" />} path="/" />
            <Route element={<HomePage />} path="/home" />
            <Route element={<CreateProjectPage />} path="/projects/new" />
            <Route
              element={<ProjectOverviewPage />}
              path="/projects/:projectId"
            />
            <Route
              element={<ProjectSettingsPage />}
              path="/projects/:projectId/settings"
            />
            <Route
              element={<WorkItemsPage />}
              path="/projects/:projectId/work-items"
            />
            <Route
              element={<WorkItemDetailPage />}
              path="/projects/:projectId/work-items/:workItemId"
            />
            <Route
              element={<ChannelPage />}
              path="/projects/:projectId/channels/:channelId"
            />
            <Route
              element={<ConversationArchivePage />}
              path="/projects/:projectId/conversations/archived"
            />
            <Route
              element={<ThreadPage />}
              path="/projects/:projectId/threads/:threadId"
            />
            <Route element={<NotFoundPage />} path="*" />
          </Route>
              </Routes>
            </Suspense>
          </AppErrorBoundary>
        </BrowserRouter>
      </QueryClientProvider>
    </ThemeProvider>
  )
}

function RouteFallback() {
  return (
    <main aria-busy="true" className="session-state">
      <div aria-hidden="true" className="brand-mark">A</div>
      <p>Opening workspace…</p>
    </main>
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
