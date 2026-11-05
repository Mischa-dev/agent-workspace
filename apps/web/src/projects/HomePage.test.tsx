import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  AuthenticatedSessionProvider,
  type AuthenticatedSession,
} from '../auth/session'
import { useProjects, useSwitchProject } from './hooks'
import { homeGreeting } from './greeting'
import { HomePage } from './HomePage'
import type { Project } from './types'

vi.mock('../shell/AppShell', () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => children,
}))

vi.mock('./HomeSections', () => ({
  Capacity: () => null,
  NeedsYou: () => null,
  QuietLine: () => null,
  SetupCard: () => null,
  WorkingNow: () => null,
  YourTasks: () => null,
}))

vi.mock('./homeQuiet', () => ({
  useQuietSections: () => ({ needsYou: false, working: false, capacity: false, tasks: false }),
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

function renderHome() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <AuthenticatedSessionProvider session={session}>
        <MemoryRouter initialEntries={['/home']}>
          <Routes>
            <Route element={<HomePage />} path="/home" />
            <Route element={<LocationProbe />} path="*" />
          </Routes>
        </MemoryRouter>
      </AuthenticatedSessionProvider>
    </QueryClientProvider>,
  )
}

describe('HomePage recent Projects', () => {
  const switchProject = vi.fn()

  afterEach(cleanup)

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
    renderHome()

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

  it('opens a project at once and records the visit in the background', async () => {
    switchProject.mockReturnValue(new Promise(() => undefined))
    const writable: Project = {
      ...viewerProject,
      capabilities: { ...viewerProject.capabilities, navigationWrite: true },
    }
    vi.mocked(useProjects).mockReturnValue({
      data: { projects: [writable] },
      isError: false,
      isPending: false,
      isSuccess: true,
    } as ReturnType<typeof useProjects>)
    renderHome()

    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(/^Good (morning|afternoon|evening), Project$/)
    fireEvent.click(screen.getByRole('link', { name: /Viewer Project/ }))
    expect(screen.getByLabelText('Current route')).toHaveTextContent(writable.navigation.href)
    expect(switchProject).toHaveBeenCalledWith(expect.objectContaining({ projectId: writable.id }))
  })
})

describe('homeGreeting', () => {
  it('welcomes someone who has opened nothing yet, and greets by the time of day after', () => {
    expect(homeGreeting('Mischa', true)).toBe('Welcome, Mischa')
    expect(homeGreeting('Mischa', false, new Date(2026, 9, 19, 9))).toBe('Good morning, Mischa')
    expect(homeGreeting('Mischa', false, new Date(2026, 9, 19, 14))).toBe('Good afternoon, Mischa')
    expect(homeGreeting('Mischa', false, new Date(2026, 9, 19, 22))).toBe('Good evening, Mischa')
  })
})
