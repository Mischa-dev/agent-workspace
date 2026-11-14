import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { getWorkItem } from './api'
import { BlockerPriorityNotice } from './BlockerPriorityNotice'
import { bulkUpdateWorkItems } from './planningApi'
import type { TaskRef, WorkItem, WorkItemPriority } from './types'

vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./api')>()),
  getWorkItem: vi.fn(),
}))
vi.mock('./planningApi', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./planningApi')>()),
  bulkUpdateWorkItems: vi.fn(async () => ({ items: [] })),
}))

function ref(key: string, priority: WorkItemPriority, extra: Partial<TaskRef> = {}): TaskRef {
  return {
    id: key.toLowerCase(),
    projectId: 'project-a',
    key,
    title: key,
    priority,
    statusName: 'Todo',
    statusCategory: 'unstarted',
    finished: false,
    archived: false,
    href: '',
    ...extra,
  }
}

function task(blockedBy: TaskRef[], priority: WorkItemPriority = 'high') {
  return { id: 'aw-1', projectId: 'project-a', key: 'AW-1', priority, blockedBy } as unknown as WorkItem
}

function fresh(key: string, priority: WorkItemPriority, projectId = 'project-a', done = false) {
  return { id: key.toLowerCase(), projectId, key, priority, version: 7, archivedAt: null, status: { semanticCategory: done ? 'completed' : 'unstarted' } } as unknown as WorkItem
}

function renderNotice(item: WorkItem, priority: WorkItemPriority = 'high') {
  const onDone = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient()}>
      <BlockerPriorityNotice item={item} onDone={onDone} priority={priority} />
    </QueryClientProvider>,
  )
  return onDone
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('BlockerPriorityNotice', () => {
  it('offers to raise only the open blockers below the new priority, and says which stay', async () => {
    vi.mocked(getWorkItem).mockImplementation(async (_project, id) => fresh(id.toUpperCase(), 'low'))
    const onDone = renderNotice(
      task([ref('AW-2', 'low'), ref('AW-3', 'urgent'), ref('AW-4', 'no_priority', { finished: true }), ref('AW-5', 'high')]),
    )

    const notice = screen.getByRole('status').textContent
    expect(notice).toContain('AW-2 blocks this and is low priority.')
    expect(notice).toContain('Raise it to high too, so it doesn’t hold this up? AW-3 and AW-5 are already high or higher, so they stay as they are.')

    fireEvent.click(screen.getByRole('button', { name: 'Raise to High' }))
    await waitFor(() => expect(onDone).toHaveBeenCalled())
    expect(vi.mocked(getWorkItem).mock.calls).toEqual([['project-a', 'aw-2']])
    expect(vi.mocked(bulkUpdateWorkItems).mock.calls.map(([input]) => ({ ...input, idempotencyKey: '' }))).toEqual([
      { projectId: 'project-a', items: [{ id: 'aw-2', expectedVersion: 7 }], changes: { priority: 'high' }, idempotencyKey: '' },
    ])
  })

  it('never touches a blocker that moved up or finished since the task was read', async () => {
    vi.mocked(getWorkItem).mockImplementation(async (_project, id) =>
      id === 'aw-2' ? fresh('AW-2', 'urgent') : id === 'aw-7' ? fresh('AW-7', 'low', 'project-a', true) : fresh('AW-6', 'medium', 'project-b'),
    )
    const onDone = renderNotice(task([ref('AW-2', 'low'), ref('AW-6', 'no_priority', { projectId: 'project-b' }), ref('AW-7', 'low')]))

    expect(screen.getByRole('status').textContent).toContain('AW-2, AW-6 and AW-7 block this at a lower priority.')
    fireEvent.click(screen.getByRole('button', { name: 'Raise to High' }))
    await waitFor(() => expect(onDone).toHaveBeenCalled())
    // Each project changes on its own; AW-2 (now urgent) and AW-7 (now done) aren't touched.
    expect(vi.mocked(bulkUpdateWorkItems).mock.calls.map(([input]) => [input.projectId, input.items])).toEqual([
      ['project-b', [{ id: 'aw-6', expectedVersion: 7 }]],
    ])
  })

  it('shows nothing when no blocker is below it, or the priority did not stick', () => {
    renderNotice(task([ref('AW-2', 'urgent')]))
    expect(screen.queryByRole('button', { name: /Raise/ })).toBeNull()
    cleanup()
    renderNotice(task([ref('AW-2', 'low')], 'medium'), 'high')
    expect(screen.queryByRole('button', { name: /Raise/ })).toBeNull()
  })
})
