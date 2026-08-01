import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, Route, Routes } from 'react-router-dom'

import { WorkItemsPage } from './WorkItemsPage'
import {
  useAssignableActors,
  useCreateWorkItem,
  useSetWorkItemArchived,
  useUpdateWorkItem,
  useWorkItem,
  useWorkItems,
  useWorkItemWorkflow,
} from './hooks'
import { WorkItemDetailPage } from './WorkItemDetailPage'
import { ApiError } from '../api/errors'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'

vi.mock('../auth/session', () => ({
  useAuthenticatedSession: () => ({ user: { displayName: 'Mischa' } }),
}))
vi.mock('../projects/hooks', () => ({ useProject: vi.fn() }))
vi.mock('../projects/useRememberProjectLocation', () => ({
  useRememberProjectLocation: vi.fn(),
}))
vi.mock('./hooks', () => ({
  useAssignableActors: vi.fn(),
  useCreateWorkItem: vi.fn(),
  useSetWorkItemArchived: vi.fn(),
  useUpdateWorkItem: vi.fn(),
  useWorkItem: vi.fn(),
  useWorkItems: vi.fn(),
  useWorkItemWorkflow: vi.fn(),
}))
vi.mock('../shell/AppShell', () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => <main>{children}</main>,
}))
vi.mock('../projects/ProjectContextDrawer', () => ({
  ProjectContextDrawer: () => null,
}))

const projectId = '019fb600-0000-7000-8000-000000000001'
const workItemId = '019fb600-0000-7000-8000-000000000002'
const project = {
  id: projectId,
  organizationId: 'organization-a',
  key: 'AW',
  name: 'Agent Workspace',
  archivedAt: null,
  role: 'owner',
  version: 1,
  attentionCount: 0,
  capabilities: { view: true, manage: true, archive: true, navigationWrite: true },
  navigation: {
    location: { kind: 'work_items' },
    href: `/projects/${projectId}/work-items`,
    filters: {},
    pinned: false,
    lastOpenedAt: null,
    version: 1,
  },
}
const workflow = {
  types: [{ id: 'type-a', name: 'Task', position: 1, isDefault: true }],
  statuses: [
    {
      id: 'status-a',
      name: 'Backlog',
      semanticCategory: 'backlog',
      position: 1,
      isDefault: true,
    },
  ],
  transitions: [],
}
const hostileTitle = '<img src=x onerror=alert(1)>'
const workItem = {
  id: workItemId,
  projectId,
  key: 'AW-1',
  number: 1,
  title: 'Original title',
  description: 'Original description',
  itemType: { id: 'type-a', name: 'Task' },
  status: { id: 'status-a', name: 'Backlog', semanticCategory: 'backlog' },
  priority: 'high' as const,
  assignee: null,
  version: 1,
  archivedAt: null,
  createdAt: '2026-07-31T00:00:00Z',
  updatedAt: '2026-07-31T00:00:00Z',
  capabilities: { view: true, edit: true, archive: true },
}

beforeEach(() => {
  vi.mocked(useProject).mockReturnValue({
    data: project,
    isPending: false,
    isError: false,
    isSuccess: true,
  } as unknown as ReturnType<typeof useProject>)
  vi.mocked(useRememberProjectLocation).mockReturnValue({ error: null, retry: vi.fn() })
  vi.mocked(useWorkItemWorkflow).mockReturnValue({
    data: workflow,
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useWorkItemWorkflow>)
  vi.mocked(useWorkItems).mockReturnValue({
    data: {
      items: [
        {
          id: workItemId,
          projectId,
          key: 'AW-1',
          number: 1,
          title: hostileTitle,
          itemType: { id: 'type-a', name: 'Task' },
          status: { id: 'status-a', name: 'Backlog', semanticCategory: 'backlog' },
          priority: 'high',
          assignee: null,
          version: 1,
          archivedAt: null,
          updatedAt: '2026-07-31T00:00:00Z',
          capabilities: { view: true, edit: true, archive: true },
        },
      ],
      nextCursor: null,
    },
    isPending: false,
    isError: false,
    isSuccess: true,
  } as unknown as ReturnType<typeof useWorkItems>)
  vi.mocked(useCreateWorkItem).mockReturnValue({
    mutate: vi.fn(),
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useCreateWorkItem>)
  vi.mocked(useWorkItem).mockReturnValue({
    data: workItem,
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useWorkItem>)
  vi.mocked(useAssignableActors).mockReturnValue({
    data: [],
  } as unknown as ReturnType<typeof useAssignableActors>)
  vi.mocked(useUpdateWorkItem).mockReturnValue({
    mutate: vi.fn(),
    reset: vi.fn(),
    isPending: false,
    isError: false,
    isSuccess: false,
  } as unknown as ReturnType<typeof useUpdateWorkItem>)
  vi.mocked(useSetWorkItemArchived).mockReturnValue({
    mutate: vi.fn(),
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useSetWorkItemArchived>)
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

function renderPage(path = `/projects/${projectId}/work-items`) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route element={<WorkItemsPage />} path="/projects/:projectId/work-items" />
      </Routes>
    </MemoryRouter>,
  )
}

describe('Work Items list', () => {
  it('renders bounded cards as inert text and has no basic accessibility violations', async () => {
    const rendered = renderPage()

    expect(screen.getByText(hostileTitle)).toBeVisible()
    expect(rendered.container.querySelector('img')).toBeNull()
    expect(screen.getByRole('link', { name: /AW-1/ })).toHaveAttribute(
      'href',
      `/projects/${projectId}/work-items/${workItemId}`,
    )
    expect((await axe.run(rendered.container)).violations).toEqual([])
  })

  it('keeps filters in the URL and sends them to the query', () => {
    renderPage(`/projects/${projectId}/work-items?priority=urgent`)
    expect(screen.getByRole('combobox', { name: 'Priority' })).toHaveValue('urgent')

    fireEvent.change(screen.getByRole('combobox', { name: 'Status' }), {
      target: { value: 'status-a' },
    })

    expect(vi.mocked(useWorkItems)).toHaveBeenLastCalledWith(
      projectId,
      expect.objectContaining({ priority: 'urgent', statusId: 'status-a' }),
    )
  })

  it('submits creation with an idempotency key', () => {
    const mutate = vi.fn()
    vi.mocked(useCreateWorkItem).mockReturnValue({
      mutate,
      isPending: false,
      isError: false,
    } as unknown as ReturnType<typeof useCreateWorkItem>)
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: 'New Work Item' }))
    fireEvent.change(screen.getByLabelText('Title'), {
      target: { value: 'First durable task' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create Work Item' }))

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId,
        title: 'First durable task',
        idempotencyKey: expect.any(String),
      }),
      expect.any(Object),
    )
  })
})

describe('Work Item detail', () => {
  it('preserves the draft and exposes the authoritative resource on conflict', () => {
    const current = { ...workItem, title: 'Title from another editor', version: 2 }
    const mutate = vi.fn((_input, options) => {
      options.onError(
        new ApiError({
          status: 409,
          code: 'version_conflict',
          message: 'The Work Item changed.',
          details: { currentVersion: 2, current },
        }),
      )
    })
    vi.mocked(useUpdateWorkItem).mockReturnValue({
      mutate,
      reset: vi.fn(),
      isPending: false,
      isError: true,
      isSuccess: false,
      error: new Error('conflict'),
    } as unknown as ReturnType<typeof useUpdateWorkItem>)
    render(
      <MemoryRouter
        initialEntries={[
          `/projects/${projectId}/work-items/${workItemId}`,
        ]}
      >
        <Routes>
          <Route
            element={<WorkItemDetailPage />}
            path="/projects/:projectId/work-items/:workItemId"
          />
        </Routes>
      </MemoryRouter>,
    )

    const title = screen.getByLabelText('Title')
    fireEvent.change(title, { target: { value: 'My unsaved draft' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }))

    expect(title).toHaveValue('My unsaved draft')
    expect(screen.getByText('A newer version exists.')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Load latest version' }))
    expect(title).toHaveValue('Title from another editor')
  })
})
