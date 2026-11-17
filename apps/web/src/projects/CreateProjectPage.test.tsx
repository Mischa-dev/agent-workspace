import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen } from '@testing-library/react'
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
vi.mock('./hooks', () => ({
  useCreateProject: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useSwitchProject: () => ({ mutateAsync: vi.fn() }),
}))
vi.mock('../views/api', () => ({
  listProjectTemplates: vi.fn().mockResolvedValue({ templates: [] }),
  viewKeys: { projectTemplates: (id: string) => ['templates', id] },
}))
vi.mock('../experience/PresetPicker', () => ({ default: () => null }))
vi.mock('../shell/shellStore', () => ({ useShellPage: vi.fn() }))

afterEach(cleanup)

function page(workspace: OrganizationSummary, client: QueryClient): ReactNode {
  return (
    <QueryClientProvider client={client}>
      <MemoryRouter>
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
})
