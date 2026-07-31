import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  AuthenticatedSessionProvider,
  type AuthenticatedSession,
} from '../auth/session'
import { useProjects, useSwitchProject } from './hooks'
import { HomePage } from './HomePage'
import type { Project } from './types'

vi.mock('../shell/AppShell', () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => children,
}))

vi.mock('./hooks', () => ({
  useProjects: vi.fn(),
  useSwitchProject: vi.fn(),
}))

const session: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: '019fb600-0000-7000-8000-000000000001',
    actorId: '019fb600-0000-7000-8000-000000000002',
    actorKind: 'human',
    loginName: 'viewer',
    displayName: 'Project Viewer',
    capabilities: { createProject: false },
  },
  organizations: [],
}

const viewerProject: Project = {
  id: '019fb600-0000-7000-8000-000000000003',
  organizationId: '019fb600-0000-7000-8000-000000000004',
  key: 'VIEW',
  name: 'Viewer Project',
  archivedAt: null,
  role: 'viewer',
  version: 1,
  attentionCount: 0,
  capabilities: {
    view: true,
    manage: false,
    archive: false,
    navigationWrite: false,
  },
  navigation: {
    location: { kind: 'settings' },
    href: '/projects/019fb600-0000-7000-8000-000000000003/settings',
    pinned: false,
    lastOpenedAt: '2026-07-31T06:00:00Z',
    version: 3,
  },
}

function LocationProbe() {
  return <output aria-label="Current route">{useLocation().pathname}</output>
}

describe('HomePage recent Projects', () => {
  const switchProject = vi.fn()

  beforeEach(() => {
    switchProject.mockReset()
    vi.mocked(useProjects).mockReturnValue({
      data: { projects: [viewerProject] },
      isError: false,
      isPending: false,
    } as ReturnType<typeof useProjects>)
    vi.mocked(useSwitchProject).mockReturnValue({
      mutateAsync: switchProject,
    } as unknown as ReturnType<typeof useSwitchProject>)
  })

  it('opens a Viewer Project at its server-derived location without a switch write', async () => {
    render(
      <AuthenticatedSessionProvider session={session}>
        <MemoryRouter initialEntries={['/home']}>
          <Routes>
            <Route element={<HomePage />} path="/home" />
            <Route element={<LocationProbe />} path="*" />
          </Routes>
        </MemoryRouter>
      </AuthenticatedSessionProvider>,
    )

    const projectLink = screen.getByRole('link', { name: /Viewer Project/ })
    expect(projectLink).toHaveAttribute('href', viewerProject.navigation.href)
    fireEvent.click(projectLink)

    await waitFor(() =>
      expect(screen.getByLabelText('Current route')).toHaveTextContent(
        viewerProject.navigation.href,
      ),
    )
    expect(switchProject).not.toHaveBeenCalled()
  })
})
