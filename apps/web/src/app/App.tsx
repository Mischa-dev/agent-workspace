import { useState } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useNavigate,
} from 'react-router-dom'

import { SessionBoundary } from '../auth/SessionBoundary'
import { SignInPage } from '../auth/SignInPage'
import { useAuthenticatedSession } from '../auth/session'
import { CreateProjectPage } from '../projects/CreateProjectPage'
import { ChannelPage } from '../conversations/ChannelPage'
import { ConversationArchivePage } from '../conversations/ConversationArchivePage'
import { HomePage } from '../projects/HomePage'
import { ProjectOverviewPage } from '../projects/ProjectOverviewPage'
import { ProjectSettingsPage } from '../projects/ProjectSettingsPage'
import { AppShell } from '../shell/AppShell'
import { ThreadPage } from '../threads/ThreadPage'
import { WorkItemDetailPage } from '../work-items/WorkItemDetailPage'
import { WorkItemsPage } from '../work-items/WorkItemsPage'

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
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route element={<SignInPage />} path="/sign-in" />
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
      </BrowserRouter>
    </QueryClientProvider>
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
