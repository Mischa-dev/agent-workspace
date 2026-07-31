import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  AuthenticatedSessionProvider,
  type AuthenticatedSession,
} from '../auth/session'
import { useArchiveProject, useProject } from './hooks'
import { ProjectSettingsPage } from './ProjectSettingsPage'
import type { Project } from './types'
import { useRememberProjectLocation } from './useRememberProjectLocation'

vi.mock('./hooks', () => ({
  useArchiveProject: vi.fn(),
  useProject: vi.fn(),
}))

vi.mock('./useRememberProjectLocation', () => ({
  useRememberProjectLocation: vi.fn(),
}))

const session: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: '019fb600-0000-7000-8000-000000000011',
    actorId: '019fb600-0000-7000-8000-000000000012',
    actorKind: 'human',
    loginName: 'owner',
    displayName: 'Project Owner',
    capabilities: { createProject: true },
  },
  organizations: [],
}

const archivedProject: Project = {
  id: '019fb600-0000-7000-8000-000000000013',
  organizationId: '019fb600-0000-7000-8000-000000000014',
  key: 'OLD',
  name: 'Archived Project',
  archivedAt: '2026-07-31T08:00:00Z',
  role: 'owner',
  version: 2,
  attentionCount: 0,
  capabilities: {
    view: true,
    manage: true,
    archive: true,
    navigationWrite: false,
  },
  navigation: {
    location: { kind: 'project_overview' },
    href: '/projects/019fb600-0000-7000-8000-000000000013',
    pinned: false,
    lastOpenedAt: '2026-07-31T07:00:00Z',
    version: 2,
  },
}

function LocationProbe() {
  const location = useLocation()
  return <output aria-label="Current route">{location.pathname}</output>
}

describe('ProjectSettingsPage', () => {
  beforeEach(() => {
    vi.mocked(useArchiveProject).mockReturnValue({
      mutateAsync: vi.fn(),
    } as unknown as ReturnType<typeof useArchiveProject>)
    vi.mocked(useRememberProjectLocation).mockReturnValue({
      failed: false,
      retry: vi.fn(),
    } as unknown as ReturnType<typeof useRememberProjectLocation>)
  })

  it('falls back from archived settings to the Project overview after loading', async () => {
    vi.mocked(useProject).mockReturnValue({
      data: archivedProject,
      isError: false,
      isPending: false,
      isSuccess: true,
    } as ReturnType<typeof useProject>)

    render(
      <AuthenticatedSessionProvider session={session}>
        <MemoryRouter
          initialEntries={[
            `/projects/${archivedProject.id}/settings`,
          ]}
        >
          <Routes>
            <Route
              element={<ProjectSettingsPage />}
              path="/projects/:projectId/settings"
            />
            <Route
              element={<LocationProbe />}
              path="/projects/:projectId"
            />
          </Routes>
        </MemoryRouter>
      </AuthenticatedSessionProvider>,
    )

    expect(await screen.findByLabelText('Current route')).toHaveTextContent(
      archivedProject.navigation.href,
    )
    expect(useRememberProjectLocation).toHaveBeenCalledWith(
      archivedProject.id,
      { kind: 'settings' },
      false,
      archivedProject.navigation,
    )
  })
})
