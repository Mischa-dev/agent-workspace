import { act, renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { discardOutbound, useOutbound, type OutboundEntry } from '../data/outbound'
import { useQueueWorkItemUpdate, useUpdateWorkItem } from './hooks'
import { EMPTY_PLANNING } from './testFixtures'
import type { WorkItem } from './types'
import { useTaskSave } from './useTaskSave'

vi.mock('./hooks', () => ({ useUpdateWorkItem: vi.fn(), useQueueWorkItemUpdate: vi.fn() }))
vi.mock('../data/outbound', () => ({ useOutbound: vi.fn(() => []), discardOutbound: vi.fn() }))
vi.mock('../api/ops', async (original) => ({ ...(await original<typeof import('../api/ops')>()), callOp: vi.fn() }))

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
  openIn: [],
  capabilities: { view: true, edit: true, archive: true },
  ...EMPTY_PLANNING,
}

// `task_update`'s refusal once `updateWorkItem` has read the whole task (W1.9a).
const conflict = (current: WorkItem) =>
  new ApiError({ status: 409, code: 'conflict', message: 'The task changed.', details: { reason: 'version', currentVersion: current.version, current: { task: current, whole: true } } })

function setup(mutateAsync: ReturnType<typeof vi.fn>, suggest = false) {
  vi.mocked(useUpdateWorkItem).mockReturnValue({ mutateAsync } as unknown as ReturnType<typeof useUpdateWorkItem>)
  vi.mocked(useQueueWorkItemUpdate).mockReturnValue({ mutateAsync } as unknown as ReturnType<typeof useQueueWorkItemUpdate>)
  const client = new QueryClient()
  client.setQueryData(queryKeys.workItem(task.projectId, task.id), task)
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>
  return { client, ...renderHook(() => useTaskSave(task, { suggest }), { wrapper }) }
}

afterEach(() => {
  vi.clearAllMocks()
  vi.mocked(useOutbound).mockReturnValue([])
})

// Offline: queueOp keeps the edit on the device under its request id, then answers null.
const keptOffline = () =>
  vi.fn().mockImplementation(async (input: { idempotencyKey: string }) => {
    vi.mocked(useOutbound).mockReturnValue([{ id: input.idempotencyKey } as OutboundEntry])
    return null
  })

describe('useTaskSave', () => {
  it('suggests through task_update, and a retry of a lost answer reuses the request id', async () => {
    const mutateAsync = vi.fn()
    vi.mocked(callOp)
      .mockRejectedValueOnce(new ApiError({ status: 503, code: 'temporarily_unavailable', message: 'Try again.' }))
      .mockResolvedValue({ workItemId: task.id, version: 1, changedFields: [], resultId: 'p1', idempotentReplay: true, proposalId: 'p1' })
    const { client, result } = setup(mutateAsync, true)

    act(() => result.current.save('priority', { priority: 'high' }))
    await waitFor(() => expect(result.current.stopped).toHaveLength(1))
    // An unrelated change arrives, then the same patch again, as the
    // description's retry button sends it.
    act(() => client.setQueryData(queryKeys.workItem(task.projectId, task.id), { ...task, dueDate: '2026-12-01', version: 2 }))
    act(() => result.current.retry('priority', { priority: 'high' }))
    await waitFor(() => expect(result.current.stopped).toHaveLength(0))
    expect(mutateAsync).not.toHaveBeenCalled()
    const [first, second] = vi.mocked(callOp).mock.calls
    expect(first[1]).toEqual({ taskId: task.id, expectedVersion: 1, priority: 'high' })
    expect(second[2]?.requestId).toBe(first[2]?.requestId)
    expect(second[1]).toEqual(first[1])
  })

  it('sends a second suggestion for a field while the first is on its way', async () => {
    let answer: (value: unknown) => void = () => {}
    vi.mocked(callOp)
      .mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)) as never)
      .mockResolvedValue({ workItemId: task.id, version: 1, changedFields: [], resultId: 'p2', idempotentReplay: false, proposalId: 'p2' })
    const { result } = setup(vi.fn(), true)

    act(() => result.current.save('title', { title: 'Parse the options' }))
    await waitFor(() => expect(callOp).toHaveBeenCalledTimes(1))
    act(() => result.current.save('title', { title: 'Parse every flag' }))
    answer({ workItemId: task.id, version: 1, changedFields: [], resultId: 'p1', idempotentReplay: false, proposalId: 'p1' })
    await waitFor(() => expect(callOp).toHaveBeenCalledTimes(2))
    expect(vi.mocked(callOp).mock.calls[1][1]).toMatchObject({ title: 'Parse every flag' })
    await waitFor(() => expect(result.current.pending('title')).toBeNull())
  })

  it('keeps the task as the baseline for a draft opened while a suggestion is on its way', async () => {
    let answer: (value: unknown) => void = () => {}
    vi.mocked(callOp)
      .mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)) as never)
      .mockResolvedValue({ workItemId: task.id, version: 1, changedFields: [], resultId: 'p2', idempotentReplay: false, proposalId: 'p2' })
    const { result } = setup(vi.fn(), true)

    act(() => result.current.save('description', { description: 'First idea' }))
    await waitFor(() => expect(callOp).toHaveBeenCalledTimes(1))
    // The description editor opens now and records what it shows as its baseline.
    const base = result.current.baseFor('description')
    expect(base).toBe('First idea')
    answer({ workItemId: task.id, version: 1, changedFields: [], resultId: 'p1', idempotentReplay: false, proposalId: 'p1' })
    await waitFor(() => expect(result.current.pending('description')).toBeNull())

    act(() => result.current.save('description', { description: 'Second idea' }, base))
    await waitFor(() => expect(callOp).toHaveBeenCalledTimes(2))
    expect(vi.mocked(callOp).mock.calls[1][1]).toMatchObject({ description: 'Second idea' })
    await waitFor(() => expect(result.current.pending('description')).toBeNull())
  })

  it('stops a queued suggestion when someone else changed the field meanwhile', async () => {
    let answer: (value: unknown) => void = () => {}
    vi.mocked(callOp).mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)) as never)
    const { client, result } = setup(vi.fn(), true)

    act(() => result.current.save('title', { title: 'Parse the options' }))
    await waitFor(() => expect(callOp).toHaveBeenCalledTimes(1))
    act(() => result.current.save('title', { title: 'Parse every flag' }))
    // A live update brings someone else's title before the first answer.
    act(() => client.setQueryData(queryKeys.workItem(task.projectId, task.id), { ...task, title: 'Parse the CLI', version: 2 }))
    answer({ workItemId: task.id, version: 1, changedFields: [], resultId: 'p1', idempotentReplay: false, proposalId: 'p1' })
    await waitFor(() => expect(result.current.pending('title')?.state).toBe('conflict'))
    expect(callOp).toHaveBeenCalledTimes(1)
  })

  it('shows an edit kept offline, and lets it go once it has been sent', async () => {
    const mutateAsync = keptOffline()
    const { client, result, rerender } = setup(mutateAsync)

    act(() => result.current.save('priority', { priority: 'urgent' }))
    await waitFor(() => expect(result.current.queued).toBe(true))
    expect(client.getQueryData<WorkItem>(queryKeys.workItem(task.projectId, task.id))?.priority).toBe('urgent')
    expect(result.current.baseFor('priority')).toBe('urgent')

    rerender()
    expect(result.current.queued).toBe(true)
    vi.mocked(useOutbound).mockReturnValue([])
    rerender()
    await waitFor(() => expect(result.current.queued).toBe(false))
    expect(result.current.stopped).toEqual([])
  })

  it('turns an edit kept offline that the server refused into a failed save', async () => {
    const mutateAsync = keptOffline()
    const { result, rerender } = setup(mutateAsync)

    act(() => result.current.save('dueDate', { dueDate: '2026-12-01' }))
    await waitFor(() => expect(result.current.queued).toBe(true))
    const { idempotencyKey } = mutateAsync.mock.calls[0][0] as { idempotencyKey: string }

    vi.mocked(useOutbound).mockReturnValue([{ id: idempotencyKey, refused: 'You can’t change this task.' } as OutboundEntry])
    rerender()
    await waitFor(() => expect(result.current.stopped).toEqual([expect.objectContaining({ field: 'dueDate', state: 'failed', message: 'You can’t change this task.' })]))
    expect(discardOutbound).toHaveBeenCalledWith(idempotencyKey)
  })

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

  it('saves a custom field put back to what the task held, while the first change is on its way', async () => {
    let finish: () => void = () => undefined
    const first = new Promise<void>((resolve) => {
      finish = resolve
    })
    const mutateAsync = vi.fn().mockReturnValueOnce(first).mockResolvedValue(undefined)
    const { client, result } = setup(mutateAsync)

    act(() => result.current.save('field:areas', { fieldValues: { areas: ['research'] } }))
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1))
    // The person sees their own change on its way, not the task's empty value,
    // so clearing the field again is a change of its own (TaskCustomFields).
    expect(result.current.baseFor('field:areas')).toBe(JSON.stringify(['research']))

    act(() => result.current.save('field:areas', { fieldValues: { areas: null } }))
    act(() => {
      client.setQueryData(queryKeys.workItem(task.projectId, task.id), { ...task, fieldValues: { areas: ['research'] }, version: 2 })
      finish()
    })
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(2))
    expect(mutateAsync).toHaveBeenLastCalledWith(expect.objectContaining({ patch: { expectedVersion: 2, fieldValues: { areas: null } } }))
  })

  it('takes its own normalized value as its own, not as someone else’s change', async () => {
    let finish: (saved: WorkItem) => void = () => undefined
    const first = new Promise<WorkItem>((resolve) => {
      finish = resolve
    })
    const mutateAsync = vi.fn().mockReturnValueOnce(first).mockResolvedValue({ ...task, version: 3 })
    const { client, result } = setup(mutateAsync)

    act(() => result.current.save('field:site', { fieldValues: { site: 'example.com' } }))
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1))
    act(() => result.current.save('field:site', { fieldValues: { site: 'other.com' } }))
    // The server stores the first as https://example.com.
    const stored = { ...task, fieldValues: { site: 'https://example.com' }, version: 2 }
    act(() => {
      client.setQueryData(queryKeys.workItem(task.projectId, task.id), stored)
      finish(stored)
    })
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(2))
    expect(mutateAsync).toHaveBeenLastCalledWith(expect.objectContaining({ patch: { expectedVersion: 2, fieldValues: { site: 'other.com' } } }))
    await waitFor(() => expect(result.current.pending('field:site')).toBeNull())
  })

  it('still stops when someone else changed the field after its own save', async () => {
    let finish: (saved: WorkItem) => void = () => undefined
    const first = new Promise<WorkItem>((resolve) => {
      finish = resolve
    })
    const mutateAsync = vi.fn().mockReturnValueOnce(first)
    const { client, result } = setup(mutateAsync)

    act(() => result.current.save('field:site', { fieldValues: { site: 'example.com' } }))
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1))
    act(() => result.current.save('field:site', { fieldValues: { site: 'other.com' } }))
    const stored = { ...task, fieldValues: { site: 'https://example.com' }, version: 2 }
    act(() => {
      // A live update brings someone else's value before the queued save goes.
      client.setQueryData(queryKeys.workItem(task.projectId, task.id), { ...task, fieldValues: { site: 'https://theirs.com' }, version: 3 })
      finish(stored)
    })
    await waitFor(() => expect(result.current.pending('field:site')?.state).toBe('conflict'))
    expect(mutateAsync).toHaveBeenCalledTimes(1)
  })
})
