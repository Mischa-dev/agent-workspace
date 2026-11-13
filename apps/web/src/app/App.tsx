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
import { BrandMark } from '../auth/AuthLayout'
import { AppShell } from '../shell/AppShell'
import { inHomeSpace } from '../shell/homeSpace'
import { MobileNavigationProvider } from '../shell/MobileNavigationProvider'
import { ThemeProvider } from '../theme/ThemeProvider'
import { Button, EmptyState, PageSkeleton, ToastViewport } from '../ui'
import { AppErrorBoundary } from './AppErrorBoundary'
import { ModuleGate } from '../modules/ModuleGate'

const SettingsArea = lazy(() => import('../settings/SettingsArea'))
const SignInPage = lazy(() => import('../auth/SignInPage').then((module) => ({ default: module.SignInPage })))
const ForgotPasswordPage = lazy(() => import('../auth/PasswordResetPages').then((module) => ({ default: module.ForgotPasswordPage })))
const ResetPasswordPage = lazy(() => import('../auth/PasswordResetPages').then((module) => ({ default: module.ResetPasswordPage })))
const VerifyEmailPage = lazy(() => import('../auth/PasswordResetPages').then((module) => ({ default: module.VerifyEmailPage })))
const JoinPage = lazy(() => import('../invitations/JoinPage').then((module) => ({ default: module.JoinPage })))
const FormPage = lazy(() => import('../forms/Form'))
const CreateProjectPage = lazy(() => import('../projects/CreateProjectPage').then((module) => ({ default: module.CreateProjectPage })))
const ProjectsPage = lazy(() => import('../projects/ProjectsPage').then((module) => ({ default: module.ProjectsPage })))
const ChannelPage = lazy(() => import('../conversations/ChannelPage').then((module) => ({ default: module.ChannelPage })))
const ConversationArchivePage = lazy(() => import('../conversations/ConversationArchivePage').then((module) => ({ default: module.ConversationArchivePage })))
const MessagesPage = lazy(() => import('../direct/MessagesPage').then((module) => ({ default: module.MessagesPage })))
const AgentsPage = lazy(() => import('../direct/MessagesPage').then((module) => ({ default: module.AgentsPage })))
const DirectConversationPage = lazy(() => import('../direct/DirectConversationPage').then((module) => ({ default: module.DirectConversationPage })))
const HomePage = lazy(() => import('../projects/HomePage').then((module) => ({ default: module.HomePage })))
const OnboardingPage = lazy(() => import('../onboarding/OnboardingPage').then((module) => ({ default: module.OnboardingPage })))
const ProjectOverviewPage = lazy(() => import('../projects/ProjectOverviewPage').then((module) => ({ default: module.ProjectOverviewPage })))
const ThreadPage = lazy(() => import('../threads/ThreadPage').then((module) => ({ default: module.ThreadPage })))
const WorkItemDetailPage = lazy(() => import('../work-items/WorkItemDetailPage').then((module) => ({ default: module.WorkItemDetailPage })))
const WorkItemsPage = lazy(() => import('../work-items/WorkItemsPage').then((module) => ({ default: module.WorkItemsPage })))
const TimelinePage = lazy(() => import('../views/TimelinePage').then((module) => ({ default: module.TimelinePage })))
const MilestonesPage = lazy(() => import('../work-items/MilestonesPage').then((module) => ({ default: module.MilestonesPage })))
const NextPage = lazy(() => import('../ai/PlanningPages').then((module) => ({ default: module.NextPage })))
const SuggestionsPage = lazy(() => import('../ai/PlanningPages').then((module) => ({ default: module.SuggestionsPage })))
const GraphPage = lazy(() => import('../ai/GraphPage').then((module) => ({ default: module.GraphPage })))
const PlanImportPage = lazy(() => import('../ai/PlanImportPage').then((module) => ({ default: module.PlanImportPage })))
const ProjectRunsPage = lazy(() => import('../runs/RunsPage').then((module) => ({ default: module.ProjectRunsPage })))
const PagesPage = lazy(() => import('../memory/MemoryPages').then((module) => ({ default: module.PagesPage })))
const PageDetailPage = lazy(() => import('../memory/MemoryPages').then((module) => ({ default: module.PageDetailPage })))
const DecisionsPage = lazy(() => import('../memory/MemoryPages').then((module) => ({ default: module.DecisionsPage })))
const CollectionsArea = lazy(() => import('../collections/CollectionsArea'))
const SheetsArea = lazy(() => import('../sheets/SheetsArea'))
const MyWorkPage = lazy(() => import('../views/MyWorkPage').then((module) => ({ default: module.MyWorkPage })))
const RunsPage = lazy(() => import('../runs/RunsPage').then((module) => ({ default: module.RunsPage })))
const RunPage = lazy(() => import('../runs/RunPage').then((module) => ({ default: module.RunPage })))
const WhiteboardsPage = lazy(() => import('../whiteboards/WhiteboardsPage').then((module) => ({ default: module.WhiteboardsPage })))
const WhiteboardPage = lazy(() => import('../whiteboards/WhiteboardPage').then((module) => ({ default: module.WhiteboardPage })))
const InboxPage = lazy(() => import('../inbox/InboxPage').then((module) => ({ default: module.InboxPage })))
const LaterPage = lazy(() => import('../later/LaterPage').then((module) => ({ default: module.LaterPage })))
const QuickAddPage = lazy(() => import('../quick-add/QuickAddPage').then((module) => ({ default: module.QuickAddPage })))
const TaskKeyRedirect = lazy(() => import('./TaskKeyRedirect').then((module) => ({ default: module.TaskKeyRedirect })))
const DesignSystemPage = lazy(() => import('../design/DesignSystemPage').then((module) => ({ default: module.DesignSystemPage })))
const PieceGallery = lazy(() => import('../modules/pieces/PieceGallery').then((module) => ({ default: module.PieceGallery })))
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
          <RoutedErrorBoundary>
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
                      <ForgotPasswordPage />
                    </PublicRouteBoundary>
                  }
                  path="/forgot-password"
                />
                <Route
                  element={
                    <PublicRouteBoundary>
                      <ResetPasswordPage />
                    </PublicRouteBoundary>
                  }
                  path="/reset-password"
                />
                <Route
                  element={
                    <PublicRouteBoundary>
                      <VerifyEmailPage />
                    </PublicRouteBoundary>
                  }
                  path="/verify-email"
                />
                <Route
                  element={
                    <PublicRouteBoundary>
                      <JoinPage />
                    </PublicRouteBoundary>
                  }
                  path="/join"
                />
                <Route
                  element={
                    <PublicRouteBoundary>
                      <FormPage />
                    </PublicRouteBoundary>
                  }
                  path="/forms/:token"
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
                        <OnboardingPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/welcome"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <InboxPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/inbox"
                  />
                  <Route element={<AuthenticatedRouteBoundary><LaterPage /></AuthenticatedRouteBoundary>} path="/later" />
                  <Route element={<AuthenticatedRouteBoundary><QuickAddPage /></AuthenticatedRouteBoundary>} path="/new-task" />
                  <Route element={<AuthenticatedRouteBoundary><QuickAddPage /></AuthenticatedRouteBoundary>} path="/share" />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <MyWorkPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/my-work"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <RunsPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/runs"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <RunPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/runs/:runId"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <WhiteboardsPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/whiteboards"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <WhiteboardPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/whiteboards/:whiteboardId"
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
                  <Route element={<AuthenticatedRouteBoundary><ProjectsPage /></AuthenticatedRouteBoundary>} path="/projects" />
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
                        <MilestonesPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/milestones"
                  />
                  <Route element={<AuthenticatedRouteBoundary><NextPage /></AuthenticatedRouteBoundary>} path="/projects/:projectId/next" />
                  <Route element={<AuthenticatedRouteBoundary><SuggestionsPage /></AuthenticatedRouteBoundary>} path="/projects/:projectId/suggestions" />
                  <Route element={<AuthenticatedRouteBoundary><GraphPage /></AuthenticatedRouteBoundary>} path="/projects/:projectId/graph" />
                  <Route element={<AuthenticatedRouteBoundary><PlanImportPage /></AuthenticatedRouteBoundary>} path="/projects/:projectId/plan-import" />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <TimelinePage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/timeline"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <ProjectRunsPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/launches"
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
                        <CollectionsArea />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/collections/*"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <SheetsArea />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/projects/:projectId/sheets/*"
                  />
                  <Route
                    element={
                      <AuthenticatedRouteBoundary>
                        <SearchPage />
                      </AuthenticatedRouteBoundary>
                    }
                    path="/search"
                  />
                  <Route element={<AuthenticatedRouteBoundary><DesignSystemPage /></AuthenticatedRouteBoundary>} path="/design-system" />
                  <Route element={<AuthenticatedRouteBoundary><PieceGallery /></AuthenticatedRouteBoundary>} path="/design-system/pieces" />
                  <Route element={<AuthenticatedRouteBoundary><TaskKeyRedirect /></AuthenticatedRouteBoundary>} path="/:taskKey" />
                  <Route element={<NotFoundPage />} path="*" />
                </Route>
              </Routes>
            </MobileNavigationProvider>
            <ToastViewport />
          </RoutedErrorBoundary>
        </BrowserRouter>
      </QueryClientProvider>
    </ThemeProvider>
  )
}

/** A crash screen lasts only until you go somewhere else. */
function RoutedErrorBoundary({ children }: { children: ReactNode }) {
  const { pathname } = useLocation()
  return <AppErrorBoundary resetKey={pathname}>{children}</AppErrorBoundary>
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
function LegacyProjectSettingsRedirect() {
  const { projectId = '' } = useParams()
  const location = useLocation()
  return <Navigate replace to={`/settings/projects/${encodeURIComponent(projectId)}${location.search}`} />
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
