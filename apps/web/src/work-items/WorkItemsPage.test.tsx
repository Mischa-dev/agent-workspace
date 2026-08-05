import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'

import { WorkItemsPage } from './WorkItemsPage'
import {
  useAssignableActors,
  useCreateAcceptanceCriterion,
  useCreateWorkItem,
  useCreateWorkItemComment,
  useMoveWorkItem,
  useRemoveAcceptanceCriterion,
  useSetWorkItemArchived,
  useSetWorkItemPrimaryRepository,
  useSetWorkItemRepositoryLink,
  useSetWorkItemThreadLink,
  useUpdateAcceptanceCriterion,
  useUpdateWorkItem,
  useWorkItem,
  useWorkItemBoard,
  useWorkItems,
  useWorkItemWorkflow,
  useWorkItemTimeline,
  useWorkWithWorker,
} from './hooks'
import { WorkItemDetailPage } from './WorkItemDetailPage'
import { buildOpenInCodexLink } from './codexDeepLink'
import type { WorkItem } from './types'
import { ApiError } from '../api/errors'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { useProjectConversationSidebar } from '../conversations/hooks'

vi.mock('../auth/session', () => ({
  useAuthenticatedSession: () => ({ user: { displayName: 'Mischa' } }),
}))
vi.mock('../projects/hooks', () => ({ useProject: vi.fn() }))
vi.mock('../conversations/hooks', () => ({
  useProjectConversationSidebar: vi.fn(),
}))
vi.mock('../repositories/api', () => ({
  listRepositories: () => Promise.resolve({ repositories: [], canManage: false }),
}))
vi.mock('../projects/useRememberProjectLocation', () => ({
  useRememberProjectLocation: vi.fn(),
}))
vi.mock('./hooks', () => ({
  useAssignableActors: vi.fn(),
  useCreateAcceptanceCriterion: vi.fn(),
  useCreateWorkItem: vi.fn(),
  useCreateWorkItemComment: vi.fn(),
  useMoveWorkItem: vi.fn(),
  useRemoveAcceptanceCriterion: vi.fn(),
  useSetWorkItemArchived: vi.fn(),
  useSetWorkItemPrimaryRepository: vi.fn(),
  useSetWorkItemRepositoryLink: vi.fn(),
  useSetWorkItemThreadLink: vi.fn(),
  useUpdateAcceptanceCriterion: vi.fn(),
  useUpdateWorkItem: vi.fn(),
  useWorkItem: vi.fn(),
  useWorkItemBoard: vi.fn(),
  useWorkItems: vi.fn(),
  useWorkItemWorkflow: vi.fn(),
  useWorkItemTimeline: vi.fn(),
  useWorkWithWorker: vi.fn(),
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
  acceptanceCriteria: [],
  linkedThreads: [],
  linkedRepositories: [],
  linkedPullRequests: [],
  capabilities: { view: true, edit: true, archive: true },
}

beforeEach(() => {
  vi.mocked(useProjectConversationSidebar).mockReturnValue({
    data: { channels: [] },
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useProjectConversationSidebar>)
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
  vi.mocked(useWorkItemBoard).mockReturnValue({
    data: { columns: [] },
    isPending: false,
    isError: false,
    isSuccess: true,
  } as unknown as ReturnType<typeof useWorkItemBoard>)
  vi.mocked(useMoveWorkItem).mockReturnValue({
    mutate: vi.fn(),
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useMoveWorkItem>)
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
  for (const hook of [
    useCreateAcceptanceCriterion,
    useUpdateAcceptanceCriterion,
    useRemoveAcceptanceCriterion,
    useCreateWorkItemComment,
    useSetWorkItemThreadLink,
    useSetWorkItemRepositoryLink,
    useSetWorkItemPrimaryRepository,
  ]) {
    vi.mocked(hook).mockReturnValue({
      mutate: vi.fn(),
      isPending: false,
      isError: false,
    } as never)
  }
  vi.mocked(useWorkItemTimeline).mockReturnValue({
    data: [],
    isPending: false,
    isError: false,
    isSuccess: true,
  } as unknown as ReturnType<typeof useWorkItemTimeline>)
  vi.mocked(useWorkWithWorker).mockReturnValue({
    mutate: vi.fn(),
    isPending: false,
    isError: false,
  } as unknown as ReturnType<typeof useWorkWithWorker>)
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

function renderPage(path = `/projects/${projectId}/work-items`) {
  const container = document.createElement('div')
  container.id = 'root'
  document.body.append(container)
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route element={<WorkItemsPage />} path="/projects/:projectId/work-items" />
      </Routes>
    </MemoryRouter>,
    { container },
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

  it('keeps the creation dialog modal to keyboard users and restores focus', async () => {
    renderPage()
    const trigger = screen.getByRole('button', { name: 'New Work Item' })
    fireEvent.click(trigger)

    const dialog = screen.getByRole('dialog', { name: 'Capture the work' })
    const title = screen.getByLabelText('Title')
    const close = screen.getByRole('button', { name: 'Close' })
    const cancel = screen.getByRole('button', { name: 'Cancel' })
    expect(title).toHaveFocus()
    expect(document.querySelector('#root')).toHaveAttribute('inert')

    close.focus()
    fireEvent.keyDown(dialog, { key: 'Tab', shiftKey: true })
    expect(cancel).toHaveFocus()
    fireEvent.keyDown(dialog, { key: 'Tab' })
    expect(close).toHaveFocus()
    expect((await axe.run(document.body)).violations).toEqual([])

    fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(document.querySelector('#root')).not.toHaveAttribute('inert')
    await waitFor(() => expect(trigger).toHaveFocus())
  })

  it('renders the Kanban projection with keyboard-equivalent move controls', () => {
    vi.mocked(useWorkItemBoard).mockReturnValue({
      data: {
        columns: [{ status: workItem.status, items: [workItem], hasMore: false }],
      },
      isPending: false,
      isError: false,
      isSuccess: true,
    } as unknown as ReturnType<typeof useWorkItemBoard>)
    renderPage(`/projects/${projectId}/work-items?view=board`)
    expect(screen.getByRole('region', { name: 'Backlog column' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Move AW-1 up' })).toBeDisabled()
    expect(screen.getByRole('combobox', { name: 'Move AW-1 to status' })).toBeVisible()
  })
})

describe('Work Item detail', () => {
  it('builds an exactly encoded, bounded Codex deep link without mutable content or secrets', () => {
    const ready = {
      ...workItem,
      title: hostileTitle,
      description: 'AGENT_WORKSPACE_MCP_TOKEN=secret-canary /var/lib/private',
      linkedRepositories: [{
        id: 'repository-a',
        owner: 'Mischa-dev',
        name: 'agent-workspace',
        cloneUrl: 'https://github.com/Mischa-dev/agent-workspace.git',
        role: 'primary' as const,
        preparationState: 'ready',
        trusted: true,
        available: true,
        archivedAt: null,
        href: `/projects/${projectId}/settings`,
      }],
    } as WorkItem
    const result = buildOpenInCodexLink(ready, 'http://127.0.0.1:3100')
    expect(result.reason).toBeNull()
    const prompt = [
      'Use the Agent Workspace MCP server.',
      `Project ID: ${projectId}`,
      `Project URL: http://127.0.0.1:3100/projects/${projectId}`,
      `Work Item ID: ${workItemId}`,
      `Work Item URL: http://127.0.0.1:3100/projects/${projectId}/work-items/${workItemId}`,
      'Work Item key: AW-1',
      'Load the Work Item and related Project instructions through MCP before changing anything.',
      'Work in the repository matched by this Git origin. Set the Work Item to In Progress only when meaningful work begins.',
      'Post only useful progress or blocker updates. Respect the current version; refetch and make an informed retry after a version conflict.',
      'When complete, link the canonical pull request through MCP and mark the Work Item ready for review.',
    ].join('\n')
    expect(result.href).toBe(
      `codex://new?originUrl=${encodeURIComponent(ready.linkedRepositories[0].cloneUrl)}&prompt=${encodeURIComponent(prompt)}`,
    )
    expect(result.href).not.toContain(hostileTitle)
    expect(result.href).not.toContain('secret-canary')
    expect(result.href).not.toContain('/var/lib')
    expect(prompt.length).toBeLessThanOrEqual(2_000)
  })

  it.each([
    ['missing', [], 'Link one trusted repository'],
    ['ambiguous', [
      { id: 'repository-a', trusted: true, available: true, archivedAt: null, cloneUrl: 'https://github.com/a/a.git' },
      { id: 'repository-b', trusted: true, available: true, archivedAt: null, cloneUrl: 'https://github.com/b/b.git' },
    ], 'Keep exactly one linked repository'],
    ['untrusted', [{ id: 'repository-a', trusted: false, available: false, archivedAt: null, cloneUrl: 'https://github.com/a/a.git' }], 'must be trusted'],
    ['archived', [{ id: 'repository-a', trusted: true, available: false, archivedAt: '2026-07-31T00:00:00Z', cloneUrl: 'https://github.com/a/a.git' }], 'Restore the linked repository'],
    ['unsupported', [{ id: 'repository-a', trusted: true, available: true, archivedAt: null, cloneUrl: 'ssh://git@github.com/a/a.git' }], 'supported canonical Git origin URL'],
  ])('disables Open in Codex for a %s repository selection with an accessible reason', (_case, repositories, reason) => {
    const linkedRepositories = repositories.map((repository) => ({
      owner: 'owner',
      name: repository.id,
      role: 'primary' as const,
      preparationState: 'ready',
      href: '/settings',
      ...repository,
    }))
    vi.mocked(useWorkItem).mockReturnValue({
      data: { ...workItem, linkedRepositories },
      isPending: false,
      isError: false,
    } as unknown as ReturnType<typeof useWorkItem>)
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter initialEntries={[`/projects/${projectId}/work-items/${workItemId}`]}>
          <Routes><Route element={<WorkItemDetailPage />} path="/projects/:projectId/work-items/:workItemId" /></Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    )

    const button = screen.getByRole('button', { name: 'Open in Codex' })
    expect(button).toBeDisabled()
    expect(screen.getByText(new RegExp(reason))).toHaveAttribute('id', button.getAttribute('aria-describedby'))
  })

  it('starts a linked Worker Thread with explicit user instructions', () => {
    vi.mocked(useProjectConversationSidebar).mockReturnValue({
      data: {
        channels: [{ id: 'channel-a', name: 'Implementation', archivedAt: null, threads: [] }],
      },
      isPending: false,
      isError: false,
    } as unknown as ReturnType<typeof useProjectConversationSidebar>)
    const mutate = vi.fn((_input, options) => {
      options.onSuccess({
        threadId: 'thread-a',
        run: { runId: 'run-a', state: 'queued', repositoryId: 'repository-a' },
      })
    })
    vi.mocked(useWorkWithWorker).mockReturnValue({
      mutate,
      isPending: false,
      isError: false,
    } as unknown as ReturnType<typeof useWorkWithWorker>)
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter initialEntries={[`/projects/${projectId}/work-items/${workItemId}`]}>
          <Routes>
            <Route element={<WorkItemDetailPage />} path="/projects/:projectId/work-items/:workItemId" />
            <Route element={<p>Worker Thread opened</p>} path="/projects/:projectId/threads/:threadId" />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    )

    fireEvent.change(screen.getByLabelText('Channel'), { target: { value: 'channel-a' } })
    fireEvent.change(screen.getByLabelText('Instructions'), {
      target: { value: 'Implement one verified change.' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create Thread and start' }))

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({
        channelId: 'channel-a',
        expectedVersion: 1,
        prompt: 'Implement one verified change.',
        repositoryId: undefined,
      }),
      expect.any(Object),
    )
    expect(screen.getByText('Worker Thread opened')).toBeVisible()
  })

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
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
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
        </MemoryRouter>
      </QueryClientProvider>,
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
