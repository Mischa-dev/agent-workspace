import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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
  useRemoveAcceptanceCriterion,
  useSetWorkItemArchived,
  useSetWorkItemPrimaryRepository,
  useSetWorkItemRepositoryLink,
  useSetWorkItemThreadLink,
  useUpdateAcceptanceCriterion,
  useUpdateWorkItem,
  useWorkItem,
  useWorkItemWorkflow,
  useTaskFiles,
  useWorkItemTimeline,
} from './hooks'
import { useCreateLaunches, useLaunchPlan } from '../launch/hooks'
import { WorkItemDetailPage } from './WorkItemDetailPage'
import { buildOpenInCodexLink } from './codexDeepLink'
import { EMPTY_PLANNING } from './testFixtures'
import type { WorkItem } from './types'
import { ApiError } from '../api/errors'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { ToastViewport, clearToasts } from '../ui'

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
  useRemoveAcceptanceCriterion: vi.fn(),
  useSetWorkItemArchived: vi.fn(),
  useSetWorkItemPrimaryRepository: vi.fn(),
  useSetWorkItemRepositoryLink: vi.fn(),
  useSetWorkItemThreadLink: vi.fn(),
  useUpdateAcceptanceCriterion: vi.fn(),
  useUpdateWorkItem: vi.fn(),
  useWorkItem: vi.fn(),
  useWorkItems: vi.fn(),
  useWorkItemWorkflow: vi.fn(),
  useTaskFiles: vi.fn(),
  taskFilesKey: vi.fn(),
  useWorkItemTimeline: vi.fn(),
}))
vi.mock('../launch/hooks', () => ({ useCreateLaunches: vi.fn(), useLaunchPlan: vi.fn() }))
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
  canEdit: true,
  version: 1,
  estimateUnit: 'points',
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
  labels: [],
  acceptanceCriteria: [],
  linkedThreads: [],
  linkedRepositories: [],
  linkedPullRequests: [],
  capabilities: { view: true, edit: true, archive: true },
  ...EMPTY_PLANNING,
}

const card = {
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
  createdAt: '2026-07-31T00:00:00Z',
  updatedAt: '2026-07-31T00:00:00Z',
  labels: [],
  capabilities: { view: true, edit: true, archive: true },
  parentId: null,
  milestone: null,
  startDate: null,
  dueDate: null,
  estimate: null,
  blockedReason: null,
  openBlockerCount: 0,
  ready: true,
  subtaskCount: 0,
  subtasksDone: 0,
  claimedBy: null,
  boardOrder: '0000000001:00000000000000000000',
}
let requests: string[] = []

/** The planning endpoints the list, board and pickers read, answered from memory. */
function stubApi(board: unknown = { columnsBy: 'status', columns: [] }) {
  requests = []
  const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      requests.push(url)
      if (url.includes('/readiness')) return json({ ready: true, readyForAgents: true, checks: [] })
      if (url.endsWith('/ai')) return json({ configured: false, pendingSuggestions: 0, runs: [] })
      if (url.includes('/work-items/board')) return json(board)
      if (url.includes('/work-items/groups')) return json({ groups: [] })
      if (url.includes('/labels')) return json({ labels: [] })
      if (url.includes('/milestones')) return json({ milestones: [] })
      if (url.includes('/work-items')) return json({ items: [card], nextCursor: null, total: 1 })
      return json({})
    }),
  )
}

function listRequests() {
  return requests.filter((url) => /\/work-items(\?|$)/.test(url))
}

const launchPlan = {
  workItem: { id: workItemId, key: 'AW-1', title: 'Task' },
  blockers: [],
  environments: [{ id: 'environment-a', name: 'Web', harness: 'codex_cloud', harnessName: 'Codex Cloud', isDefault: true, lastCheckOk: true }],
  defaultEnvironmentId: 'environment-a',
  environmentId: 'environment-a',
  harness: 'codex_cloud',
  harnesses: [{ kind: 'codex_cloud', displayName: 'Codex Cloud', runsOn: 'cloud', maxAttempts: 4, models: [] }],
  accounts: [{ id: 'account-a', name: 'Mischa', harness: 'codex_cloud', running: 0, maxConcurrentRuns: 3 }],
  settings: {},
  prompt: 'Do AW-1.',
  templateSource: 'default',
  launches: [],
  threadId: null,
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
  stubApi()
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
  vi.mocked(useTaskFiles).mockReturnValue({
    data: [],
    isError: false,
    isSuccess: true,
  } as unknown as ReturnType<typeof useTaskFiles>)
  vi.mocked(useWorkItemTimeline).mockReturnValue({
    data: [],
    isPending: false,
    isError: false,
    isSuccess: true,
  } as unknown as ReturnType<typeof useWorkItemTimeline>)
  vi.mocked(useLaunchPlan).mockReturnValue({ isPending: true, isError: false } as unknown as ReturnType<typeof useLaunchPlan>)
  vi.mocked(useCreateLaunches).mockReturnValue({ mutate: vi.fn(), isPending: false, isError: false } as unknown as ReturnType<typeof useCreateLaunches>)
})

afterEach(() => {
  cleanup()
  clearToasts()
  vi.clearAllMocks()
  vi.unstubAllGlobals()
})

function renderPage(path = `/projects/${projectId}/work-items`) {
  const container = document.createElement('div')
  container.id = 'root'
  document.body.append(container)
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route element={<WorkItemsPage />} path="/projects/:projectId/work-items" />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
    { container },
  )
}

function renderDetail() {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[`/projects/${projectId}/work-items/${workItemId}`]}>
        <Routes>
          <Route element={<WorkItemDetailPage />} path="/projects/:projectId/work-items/:workItemId" />
        </Routes>
      </MemoryRouter>
      <ToastViewport />
    </QueryClientProvider>,
  )
}

describe('Work Items list', () => {
  it('renders titles as inert text and has no basic accessibility violations', async () => {
    const rendered = renderPage()

    expect(await screen.findByText(hostileTitle)).toBeVisible()
    expect(rendered.container.querySelector('img')).toBeNull()
    expect(screen.getByText(hostileTitle).closest('a')).toHaveAttribute('href', `/projects/${projectId}/work-items/${workItemId}`)
    expect((await axe.run(rendered.container)).violations).toEqual([])
  })

  it('reads filters from the URL, sends them to the server and clears them from a chip', async () => {
    renderPage(`/projects/${projectId}/work-items?priority=urgent`)

    await waitFor(() => expect(listRequests().at(-1)).toContain('priority=urgent'))
    fireEvent.click(screen.getByRole('button', { name: 'Remove filter: Priority is Urgent' }))
    await waitFor(() => expect(listRequests().at(-1)).not.toContain('priority='))
  })

  it('submits creation with an idempotency key', () => {
    const mutate = vi.fn()
    vi.mocked(useCreateWorkItem).mockReturnValue({
      mutate,
      isPending: false,
      isError: false,
    } as unknown as ReturnType<typeof useCreateWorkItem>)
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: 'New work item' }))
    fireEvent.change(screen.getByLabelText('Title'), {
      target: { value: 'First durable task' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create work item' }))

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
    const trigger = screen.getByRole('button', { name: 'New work item' })
    fireEvent.click(trigger)

    const dialog = screen.getByRole('dialog', { name: 'New work item' })
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

  it('shows the board with a column per status', async () => {
    stubApi({
      columnsBy: 'status',
      columns: [
        {
          key: 'status-a',
          label: 'Backlog',
          status: { id: 'status-a', name: 'Backlog', semanticCategory: 'backlog' },
          items: [card],
          hasMore: false,
          total: 1,
          nextCursor: null,
          filter: { statusId: 'status-a' },
        },
      ],
    })
    renderPage(`/projects/${projectId}/work-items?view=board`)

    const column = await screen.findByRole('region', { name: 'Backlog' })
    expect(column).toHaveTextContent(hostileTitle)
    expect(requests.some((url) => url.includes('/work-items/board'))).toBe(true)
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

  it('says why a task can’t open in Codex yet, from the task’s menu', async () => {
    renderDetail()

    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Open in Codex' }))

    expect(await screen.findByText('This task can’t open in Codex yet.')).toBeVisible()
    expect(screen.getByText('Link one trusted repository before opening this task in Codex.')).toBeVisible()
  })

  it('launches an agent on the project defaults in one click, and with an edited prompt from the dialog', async () => {
    const launched = {
      id: 'launch-a',
      workItem: { id: workItemId, key: 'AW-1', title: workItem.title },
      conversationId: 'thread-a',
      state: 'started',
      harnessName: 'Codex Cloud',
      statusReason: null,
    }
    const mutate = vi.fn((_input, options) => options.onSuccess({ launches: [launched] }))
    vi.mocked(useCreateLaunches).mockReturnValue({ mutate, isPending: false, isError: false } as unknown as ReturnType<typeof useCreateLaunches>)
    vi.mocked(useLaunchPlan).mockReturnValue({ data: launchPlan, isPending: false, isError: false } as unknown as ReturnType<typeof useLaunchPlan>)
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter initialEntries={[`/projects/${projectId}/work-items/${workItemId}`]}>
          <Routes>
            <Route element={<WorkItemDetailPage />} path="/projects/:projectId/work-items/:workItemId" />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    )

    // Launching loads with the launch pages' code.
    expect(await screen.findByText(/Codex Cloud in Web, on the project’s defaults/)).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }))
    expect(mutate).toHaveBeenLastCalledWith(
      { input: { workItemIds: [workItemId], environmentId: 'environment-a' }, idempotencyKey: expect.any(String) },
      expect.any(Object),
    )

    fireEvent.click(screen.getByRole('button', { name: 'Launch' }), { shiftKey: true })
    const prompt = await screen.findByLabelText('Prompt')
    expect(prompt).toHaveValue('Do AW-1.')
    fireEvent.change(prompt, { target: { value: 'Do AW-1, and only the parser.' } })
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Launch' }))
    expect(mutate).toHaveBeenLastCalledWith(
      {
        input: expect.objectContaining({ workItemIds: [workItemId], environmentId: 'environment-a', prompt: 'Do AW-1, and only the parser.', dialog: true }),
        idempotencyKey: expect.any(String),
      },
      expect.any(Object),
    )
  })

  it('keeps a typed title when someone else changed it first, until the person keeps theirs or takes the other', async () => {
    const current = { ...workItem, title: 'Title from another editor', version: 2 }
    const mutateAsync = vi
      .fn()
      .mockRejectedValueOnce(
        new ApiError({
          status: 409,
          code: 'version_conflict',
          message: 'The task changed.',
          details: { currentVersion: 2, current },
        }),
      )
      .mockResolvedValue({ ...current, title: 'My new title', version: 3 })
    vi.mocked(useUpdateWorkItem).mockReturnValue({ mutateAsync, isPending: false } as unknown as ReturnType<typeof useUpdateWorkItem>)
    renderDetail()

    const title = screen.getByLabelText('Title')
    fireEvent.focus(title)
    fireEvent.change(title, { target: { value: 'My new title' } })
    fireEvent.blur(title)

    expect(await screen.findByText('Someone else changed the title while you were editing.')).toBeVisible()
    expect(mutateAsync).toHaveBeenCalledTimes(1)
    expect(mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ projectId, workItemId, idempotencyKey: expect.any(String), patch: { expectedVersion: 1, title: 'My new title' } }),
    )
    expect(screen.getByText(/Theirs is “Title from another editor”/)).toBeVisible()
    expect(screen.getByLabelText('Title')).toHaveValue('My new title')

    fireEvent.click(screen.getByRole('button', { name: 'Keep yours' }))
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(2))
    expect(mutateAsync).toHaveBeenLastCalledWith(expect.objectContaining({ patch: { expectedVersion: 2, title: 'My new title' } }))
    await waitFor(() => expect(screen.queryByText('Someone else changed the title while you were editing.')).not.toBeInTheDocument())
  })

  it('opens the description editor on a click and saves it with Ctrl+Enter', async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ ...workItem, description: 'New description', version: 2 })
    vi.mocked(useUpdateWorkItem).mockReturnValue({ mutateAsync, isPending: false } as unknown as ReturnType<typeof useUpdateWorkItem>)
    renderDetail()

    fireEvent.click(screen.getByText('Original description'))
    const editor = screen.getByRole('textbox', { name: 'Description' })
    expect(editor).toHaveValue('Original description')
    fireEvent.change(editor, { target: { value: 'New description' } })
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true })

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ patch: { expectedVersion: 1, description: 'New description' } })),
    )
    expect(screen.queryByRole('textbox', { name: 'Description' })).not.toBeInTheDocument()
  })

  it('shows the last handoff note where the next person starts reading', () => {
    vi.mocked(useWorkItem).mockReturnValue({
      data: {
        ...workItem,
        handoff: {
          note: 'Parser done; the CLI flags still need tests.',
          actor: { id: 'actor-b', kind: 'external_agent', displayName: 'Builder' },
          at: '2026-07-31T01:00:00Z',
          via: null,
        },
      },
      isPending: false,
      isError: false,
    } as unknown as ReturnType<typeof useWorkItem>)
    renderDetail()

    expect(screen.getByText('Handoff from Builder')).toBeVisible()
    expect(screen.getByText('Parser done; the CLI flags still need tests.')).toBeVisible()
  })
})
