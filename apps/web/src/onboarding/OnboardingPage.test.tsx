import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { AuthenticatedSessionProvider, type AuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import type { Project } from '../projects/types'
import { createWorkItem } from '../work-items/api'
import { getWorkspace, renameWorkspace, type Workspace } from '../settings/workspace/api'
import { getOnboarding, updateOnboarding, type Onboarding, type OnboardingStepId } from './api'
import { OnboardingPage } from './OnboardingPage'

vi.mock('../shell/AppShell', () => ({ AppShell: ({ children }: { children: React.ReactNode }) => children }))
vi.mock('../projects/hooks', () => ({ useProjects: vi.fn(), useCreateProject: vi.fn(() => ({ isPending: false })) }))
vi.mock('../work-items/api', () => ({ createWorkItem: vi.fn() }))
vi.mock('./api', async (actual) => ({ ...(await actual<typeof import('./api')>()), getOnboarding: vi.fn(), updateOnboarding: vi.fn() }))
vi.mock('../settings/workspace/api', async (actual) => ({
  ...(await actual<typeof import('../settings/workspace/api')>()),
  getWorkspace: vi.fn(),
  renameWorkspace: vi.fn(),
}))

const organizationId = '019fb600-0000-7000-8000-000000000004'
const project = {
  id: '019fb600-0000-7000-8000-000000000003',
  organizationId,
  key: 'TRI',
  name: 'Trident',
  archivedAt: null,
  role: 'owner',
  capabilities: { view: true, manage: true, archive: true, navigationWrite: true },
} as unknown as Project

const session = {
  authenticated: true,
  user: { id: 'u', actorId: 'a', actorKind: 'human', loginName: 'mara', displayName: 'Mara', capabilities: { createProject: true } },
  organizations: [{ id: organizationId, name: 'Acme', slug: 'acme', role: 'owner' }],
} as unknown as AuthenticatedSession

const ORDER: OnboardingStepId[] = ['workspace', 'project', 'github', 'repository', 'account', 'environment', 'tasks', 'agent', 'launch']

function onboarding(done: OnboardingStepId[]): Onboarding {
  const steps = ORDER.map((id) => ({ id, done: done.includes(id), skipped: false, optional: id === 'agent' }))
  return {
    organizationId,
    organizationName: 'Acme',
    projectId: project.id,
    projectName: project.name,
    steps,
    nextStep: steps.find((step) => !step.done)?.id ?? null,
    complete: false,
    dismissed: false,
    canManageWorkspace: true,
  }
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={client}>
      <AuthenticatedSessionProvider session={session}>
        <MemoryRouter>
          <OnboardingPage />
        </MemoryRouter>
      </AuthenticatedSessionProvider>
    </QueryClientProvider>,
  )
}

describe('OnboardingPage', () => {
  beforeEach(() => {
    vi.mocked(useProjects).mockReturnValue({ data: { projects: [project] }, isPending: false, isError: false } as ReturnType<typeof useProjects>)
    vi.mocked(createWorkItem).mockReset()
  })

  it('opens on the next step and adds a task with its criteria from there', async () => {
    vi.mocked(getOnboarding).mockResolvedValue(onboarding(['workspace', 'project', 'github', 'repository', 'account', 'environment']))
    vi.mocked(createWorkItem).mockResolvedValue({ id: 'w1', key: 'TRI-1', title: 'Add a dark theme' } as Awaited<ReturnType<typeof createWorkItem>>)
    renderPage()

    expect(await screen.findByRole('heading', { name: 'Add your first tasks' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Add your first tasks/ })).toHaveAttribute('aria-current', 'step')
    expect(screen.getByText(/6 of 9 done/)).toBeInTheDocument()

    fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'Add a dark theme' } })
    fireEvent.change(screen.getByLabelText('Acceptance criteria'), { target: { value: '- Follows the system\n\nRemembers the choice' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add task' }))

    await waitFor(() => expect(createWorkItem).toHaveBeenCalledTimes(1))
    expect(vi.mocked(createWorkItem).mock.calls[0][0]).toMatchObject({
      projectId: project.id,
      title: 'Add a dark theme',
      acceptanceCriteria: ['Follows the system', 'Remembers the choice'],
    })
    expect(await screen.findByRole('link', { name: 'TRI-1 Add a dark theme' })).toBeInTheDocument()
  })

  it('lets a done step be reopened from the stepper', async () => {
    vi.mocked(getOnboarding).mockResolvedValue(onboarding(['workspace', 'project', 'github', 'repository', 'account', 'environment']))
    renderPage()
    fireEvent.click(await screen.findByRole('button', { name: /Create a project/ }))
    expect(screen.getByRole('heading', { name: 'Create a project' })).toBeInTheDocument()
    expect(screen.getByText(/^Done\./)).toBeInTheDocument()
  })

  it('ticks the workspace step when its name is confirmed, renaming it first if it changed', async () => {
    vi.mocked(getOnboarding).mockResolvedValue(onboarding([]))
    vi.mocked(getWorkspace).mockResolvedValue({ id: organizationId, name: 'Acme', version: 4 } as Workspace)
    vi.mocked(renameWorkspace).mockResolvedValue({ id: organizationId, name: 'Acme Labs', version: 5 } as Workspace)
    vi.mocked(updateOnboarding).mockResolvedValue(onboarding(['workspace']))
    renderPage()

    const field = await screen.findByLabelText('Workspace name')
    await waitFor(() => expect(field).toHaveValue('Acme'))
    fireEvent.change(field, { target: { value: 'Acme Labs' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save and continue' }))

    await waitFor(() => expect(updateOnboarding).toHaveBeenCalledWith(organizationId, { confirm: 'workspace' }))
    expect(renameWorkspace).toHaveBeenCalledWith(organizationId, 'Acme Labs', 4)
  })
})
