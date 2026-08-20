import { lazy, type ComponentType } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen } from '@testing-library/react'
import axe from 'axe-core'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { afterEach, describe, expect, it } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import {
  AuthenticatedSessionProvider,
  type AuthenticatedSession,
} from '../auth/session'
import type { Project } from '../projects/types'
import { ThemeProvider } from '../theme/ThemeProvider'
import { AuthenticatedRouteBoundary } from './App'

const projectId = '019fb600-0000-7000-8000-000000000001'
const project: Project = {
  id: projectId,
  organizationId: '019fb600-0000-7000-8000-000000000002',
  key: 'AW',
  name: 'Agent Workspace',
  archivedAt: null,
  role: 'owner',
  version: 1,
  attentionCount: 0,
  capabilities: {
    view: true,
    manage: true,
    archive: true,
    navigationWrite: true,
  },
  navigation: {
    location: { kind: 'work_items' },
    href: `/projects/${projectId}/work-items`,
    pinned: true,
    lastOpenedAt: '2026-08-19T18:00:00Z',
    version: 1,
  },
}
const session: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: '019fb600-0000-7000-8000-000000000003',
    actorId: '019fb600-0000-7000-8000-000000000004',
    actorKind: 'human',
    loginName: 'mischa',
    displayName: 'Mischa Nelson',
    capabilities: { createProject: true },
  },
  organizations: [],
}
const SuspendedRoute = lazy(
  () => new Promise<{ default: ComponentType }>(() => undefined),
)

afterEach(cleanup)

describe('authenticated route loading', () => {
  it('keeps the authenticated Project shell while a lazy route chunk loads', async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    queryClient.setQueryData(queryKeys.projects, { projects: [project] })
    queryClient.setQueryData(queryKeys.conversationSidebar(projectId), {
      projectId,
      channels: [],
      capabilities: { createChannel: false },
    })

    const { container } = render(
      <ThemeProvider>
        <QueryClientProvider client={queryClient}>
          <MemoryRouter
            initialEntries={[`/projects/${projectId}/work-items`]}
          >
            <AuthenticatedSessionProvider session={session}>
              <Routes>
                <Route
                  element={
                    <AuthenticatedRouteBoundary>
                      <SuspendedRoute />
                    </AuthenticatedRouteBoundary>
                  }
                  path="/projects/:projectId/work-items"
                />
              </Routes>
            </AuthenticatedSessionProvider>
          </MemoryRouter>
        </QueryClientProvider>
      </ThemeProvider>,
    )

    expect(
      screen.getByRole('navigation', { name: 'Projects' }),
    ).toBeVisible()
    expect(
      screen.getByRole('complementary', {
        name: 'Agent Workspace navigation',
      }),
    ).toBeVisible()
    expect(
      screen.getByRole('main').querySelector('.page-content'),
    ).toHaveAttribute('aria-busy', 'true')
    expect(screen.getByRole('status')).toHaveTextContent(
      'Loading workspace view.',
    )
    expect(screen.queryByText('Opening workspace…')).not.toBeInTheDocument()
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
  })
})
