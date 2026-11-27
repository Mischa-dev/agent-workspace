import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { WorkspaceContext } from '../app/workspace'
import type { OrganizationSummary } from '../auth/session'
import { CreateProjectPage } from './CreateProjectPage'

const workspaces = vi.hoisted(() => [
  { id: 'org-a', name: 'Alpha', slug: 'alpha', role: 'owner' },
  { id: 'org-b', name: 'Beta', slug: 'beta', role: 'admin' },
])

vi.mock('../auth/session', () => ({
  useAuthenticatedSession: () => ({ organizations: workspaces, user: { capabilities: { createProject: true } } }),
}))
const created = vi.hoisted(() => ({ mutateAsync: vi.fn() }))
vi.mock('./hooks', () => ({
  useCreateProject: () => ({ mutateAsync: created.mutateAsync, isPending: false }),
  useProjects: () => ({
    data: {
      projects: [
        { id: 'site', organizationId: 'org-a', name: 'Client site', kind: 'project', archivedAt: null, capabilities: { manage: true } },
        { id: 'read', organizationId: 'org-a', name: 'Read only', kind: 'project', archivedAt: null, capabilities: { manage: false } },
      ],
    },
  }),
  useSwitchProject: () => ({ mutateAsync: vi.fn() }),
}))
vi.mock('../views/api', () => ({
  listProjectTemplates: vi.fn().mockResolvedValue({ templates: [] }),
  viewKeys: { projectTemplates: (id: string) => ['templates', id] },
}))
vi.mock('../experience/PresetPicker', () => ({ default: () => null }))
vi.mock('../shell/shellStore', () => ({ useShellPage: vi.fn() }))

afterEach(cleanup)

function page(workspace: OrganizationSummary, client: QueryClient, at = '/projects/new'): ReactNode {
  return (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[at]}>
        <WorkspaceContext.Provider value={workspace}>
          <CreateProjectPage />
        </WorkspaceContext.Provider>
      </MemoryRouter>
    </QueryClientProvider>
  )
}

describe('CreateProjectPage', () => {
  it('creates in the workspace in the address, and follows a switch', () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const view = render(page(workspaces[1] as OrganizationSummary, client))
    expect(screen.getByLabelText('Workspace')).toHaveValue('org-b')
    view.rerender(page(workspaces[0] as OrganizationSummary, client))
    expect(screen.getByLabelText('Workspace')).toHaveValue('org-a')
  })

  it('makes a project inside the one in the address, with its people unless told otherwise', async () => {
    created.mutateAsync.mockRejectedValue(new Error('stop here'))
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(page(workspaces[0] as OrganizationSummary, client, '/projects/new?inside=site'))
    expect(screen.getByLabelText('Inside')).toHaveValue('site')
    // Only projects you manage can hold a new one.
    expect(screen.queryByRole('option', { name: 'Read only' })).toBeNull()
    expect(screen.getByLabelText('People')).toHaveValue('parent')
    fireEvent.change(screen.getByLabelText('People'), { target: { value: 'own' } })
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Launch' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create project' }))
    await waitFor(() => expect(created.mutateAsync).toHaveBeenCalled())
    expect(created.mutateAsync.mock.calls[0][0]).toMatchObject({ organizationId: 'org-a', name: 'Launch', parentId: 'site', people: 'own' })
  })
})
