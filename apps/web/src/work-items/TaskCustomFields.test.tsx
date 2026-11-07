import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { useUpdateWorkItem } from './hooks'
import { TaskCustomFields } from './TaskCustomFields'
import { EMPTY_PLANNING } from './testFixtures'
import type { WorkItem } from './types'
import { useTaskSave } from './useTaskSave'

const { areas } = vi.hoisted(() => ({
  areas: {
    id: 'areas',
    name: 'Areas',
    description: '',
    type: 'multiple_choice' as const,
    options: [
      { id: 'research', name: 'Research', color: '#3366ff', archived: false },
      { id: 'ops', name: 'Ops', color: '#33aa66', archived: false },
    ],
    target: null,
    position: 1,
    archived: false,
    version: 1,
  },
}))

vi.mock('../fields/api', () => ({ useFields: () => ({ data: { fields: [areas], canManage: false } }) }))
vi.mock('./hooks', () => ({ useUpdateWorkItem: vi.fn(), useAssignableActors: vi.fn() }))

const task: WorkItem = {
  id: 'task-a',
  projectId: 'project-a',
  key: 'AW-7',
  number: 7,
  title: 'Parse the flags',
  description: '',
  itemType: { id: 'type', name: 'Task' },
  status: { id: 'todo', name: 'Todo', semanticCategory: 'unstarted' },
  priority: 'medium',
  assignee: null,
  version: 1,
  archivedAt: null,
  createdAt: '',
  updatedAt: '',
  labels: [],
  acceptanceCriteria: [],
  linkedThreads: [],
  linkedRepositories: [],
  linkedPullRequests: [],
  capabilities: { view: true, edit: true, archive: true },
  ...EMPTY_PLANNING,
}

/** The task's custom fields as the task page shows them, saving through the real save queue. */
function Fields({ item }: { item: WorkItem }) {
  const save = useTaskSave(item)
  return <TaskCustomFields control="aw-prop" disabled={false} item={item} render={(_, value) => value} save={save} tucked={false} />
}

afterEach(() => vi.clearAllMocks())

describe('TaskCustomFields', () => {
  it('saves a choice cleared again while the first save is still on its way', async () => {
    let finish: () => void = () => undefined
    const first = new Promise<void>((resolve) => {
      finish = resolve
    })
    const mutateAsync = vi.fn().mockReturnValueOnce(first).mockResolvedValue(undefined)
    vi.mocked(useUpdateWorkItem).mockReturnValue({ mutateAsync } as unknown as ReturnType<typeof useUpdateWorkItem>)
    const client = new QueryClient()
    const key = queryKeys.workItem(task.projectId, task.id)
    client.setQueryData(key, task)
    render(
      <QueryClientProvider client={client}>
        <Fields item={task} />
      </QueryClientProvider>,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Areas: empty. Change' }))
    fireEvent.click(screen.getByRole('option', { name: 'Research' }))
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1))
    expect(mutateAsync).toHaveBeenLastCalledWith(
      expect.objectContaining({ patch: { expectedVersion: 1, fieldValues: { areas: ['research'] } } }),
    )

    // Cleared before that save lands: the task still holds nothing, but the
    // person's own change doesn't, so this is a change to save too.
    fireEvent.click(screen.getByRole('option', { name: 'Research' }))
    act(() => {
      client.setQueryData(key, { ...task, fieldValues: { areas: ['research'] }, version: 2 })
      finish()
    })
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(2))
    expect(mutateAsync).toHaveBeenLastCalledWith(expect.objectContaining({ patch: { expectedVersion: 2, fieldValues: { areas: null } } }))
  })
})
