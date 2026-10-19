import { act, renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useUpdateWorkItem } from './hooks'
import { EMPTY_PLANNING } from './testFixtures'
import type { WorkItem } from './types'
import { useTaskSave } from './useTaskSave'

vi.mock('./hooks', () => ({ useUpdateWorkItem: vi.fn() }))

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

const conflict = (current: WorkItem) =>
  new ApiError({ status: 409, code: 'version_conflict', message: 'The task changed.', details: { currentVersion: current.version, current } })

function setup(mutateAsync: ReturnType<typeof vi.fn>) {
  vi.mocked(useUpdateWorkItem).mockReturnValue({ mutateAsync } as unknown as ReturnType<typeof useUpdateWorkItem>)
  const client = new QueryClient()
  client.setQueryData(queryKeys.workItem(task.projectId, task.id), task)
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>
  return { client, ...renderHook(() => useTaskSave(task), { wrapper }) }
}

afterEach(() => vi.clearAllMocks())

describe('useTaskSave', () => {
  it('stops instead of overwriting when someone else changed the same field, and keeps yours only when asked', async () => {
    const theirs = { ...task, title: 'Parse the CLI flags', version: 2 }
    const mutateAsync = vi.fn().mockRejectedValueOnce(conflict(theirs)).mockResolvedValue({ ...theirs, title: 'Mine', version: 3 })
    const { result } = setup(mutateAsync)

    act(() => result.current.save('title', { title: 'Mine' }))
    await waitFor(() => expect(result.current.pending('title')?.state).toBe('conflict'))
    expect(mutateAsync).toHaveBeenCalledTimes(1)
    expect(result.current.pending('title')).toMatchObject({ text: 'Mine', theirs: { title: 'Parse the CLI flags' } })

    act(() => result.current.keepMine('title'))
    await waitFor(() => expect(result.current.pending('title')).toBeNull())
    expect(mutateAsync).toHaveBeenLastCalledWith(expect.objectContaining({ patch: { expectedVersion: 2, title: 'Mine' } }))
  })

  it('retries on its own when the newer version changed a different field', async () => {
    const newer = { ...task, priority: 'urgent' as const, version: 2 }
    const mutateAsync = vi.fn().mockRejectedValueOnce(conflict(newer)).mockResolvedValue({ ...newer, title: 'Mine', version: 3 })
    const { result } = setup(mutateAsync)

    act(() => result.current.save('title', { title: 'Mine' }))
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(2))
    expect(mutateAsync).toHaveBeenLastCalledWith(expect.objectContaining({ patch: { expectedVersion: 2, title: 'Mine' } }))
    await waitFor(() => expect(result.current.pending('title')).toBeNull())
  })

  it('sends nothing when the task already changed under what the person saw', async () => {
    const mutateAsync = vi.fn()
    const { client, result } = setup(mutateAsync)
    // A live update brought someone else's status before this change was sent.
    client.setQueryData(queryKeys.workItem(task.projectId, task.id), { ...task, status: { id: 'doing', name: 'Doing', semanticCategory: 'started' }, version: 2 })

    act(() => result.current.save('status', { statusId: 'done' }, 'todo'))
    await waitFor(() => expect(result.current.stopped).toHaveLength(1))
    expect(result.current.stopped[0]).toMatchObject({ field: 'status', state: 'conflict' })
    expect(mutateAsync).not.toHaveBeenCalled()

    act(() => result.current.discard('status'))
    expect(result.current.stopped).toHaveLength(0)
  })

  it('leaves a change alone when the task already holds the same value', async () => {
    const mutateAsync = vi.fn()
    const { client, result } = setup(mutateAsync)
    client.setQueryData(queryKeys.workItem(task.projectId, task.id), { ...task, title: 'Mine', version: 2 })

    act(() => result.current.save('title', { title: 'Mine' }, 'Parse the flags'))
    await waitFor(() => expect(result.current.saving).toBe(false))
    expect(result.current.pending('title')).toBeNull()
    expect(mutateAsync).not.toHaveBeenCalled()
  })
})
