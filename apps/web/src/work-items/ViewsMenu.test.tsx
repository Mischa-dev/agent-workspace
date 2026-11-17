import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { FieldDefinition } from '../fields/fieldModel'
import { createTaskView, type TaskView } from '../views/api'
import { listWorkItems } from './api'
import { readListState, type ListState } from './listModel'
import ViewsMenu from './ViewsMenu'

const fields = vi.hoisted(() => ({ current: {} as Record<string, unknown> }))

vi.mock('../fields/api', () => ({ useFields: () => fields.current }))
vi.mock('../experience/layout', () => ({ useProjectExperience: () => ({ shows: () => true }) }))
vi.mock('../views/api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../views/api')>()),
  listProjectTaskViews: vi.fn(async () => ({ views: [], defaultViewId: null, collectionId: 'tasks-a' })),
  createTaskView: vi.fn(),
  pinTaskView: vi.fn(),
}))
vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./api')>()),
  listWorkItems: vi.fn(async () => ({ items: [], nextCursor: null, total: 0, collectionId: 'tasks-a', config: { filters: ['read'] } })),
}))

const me = '01a1b5c0-0000-7000-8000-000000000001'
const owner: FieldDefinition = {
  id: '01a1b5c0-0000-7000-8000-0000000000aa',
  name: 'Owner',
  description: '',
  type: 'person',
  options: [],
  target: null,
  position: 1,
  archived: false,
  version: 1,
}
const ownedByMe = readListState(
  new URLSearchParams({ fieldFilters: JSON.stringify([{ field: owner.id, op: 'is', values: [me] }]) }),
)

function menu(state: ListState) {
  return (
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <ViewsMenu actorId={me} projectId="project-a" readOnly={false} state={state} update={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>
  )
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('ViewsMenu', () => {
  it('waits for the project fields before saving field filters, then saves "me" for a person field', async () => {
    fields.current = { isSuccess: false, isError: false, data: undefined, refetch: vi.fn() }
    vi.mocked(createTaskView).mockResolvedValue({ id: 'view-a', name: 'Mine', type: 'table', listParams: null, pinned: true } as unknown as TaskView)
    const client = new QueryClient()
    const wrap = (state: ListState) => (
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <ViewsMenu actorId={me} projectId="project-a" readOnly={false} state={state} update={vi.fn()} />
        </MemoryRouter>
      </QueryClientProvider>
    )
    const { rerender } = render(wrap(ownedByMe))

    fireEvent.click(screen.getByRole('button', { name: 'Views' }))
    const waiting = await screen.findByRole('menuitem', { name: 'Loading fields to save this view…' })
    expect((waiting as HTMLButtonElement).disabled).toBe(true)
    expect(screen.queryByRole('menuitem', { name: 'Save as new view…' })).toBeNull()

    // Loaded: saving goes ahead, with the person's own ID kept as "me".
    fields.current = { isSuccess: true, isError: false, data: { fields: [owner], canManage: false }, refetch: vi.fn() }
    rerender(wrap(ownedByMe))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Save as new view…' }))
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'Mine' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save view' }))
    await waitFor(() => expect(createTaskView).toHaveBeenCalledTimes(1))
    // The server reads the list's address as the view, "me" kept.
    expect((vi.mocked(listWorkItems).mock.calls[0][1] as Record<string, unknown>).fieldFilters).toBe(
      JSON.stringify([{ field: owner.id, op: 'is', values: ['me'] }]),
    )
    expect(vi.mocked(createTaskView).mock.calls[0][0]).toBe('tasks-a')
    expect(vi.mocked(createTaskView).mock.calls[0][1].config).toEqual({ filters: ['read'] })
  })

  it('offers to load the fields again when they failed, instead of saving', async () => {
    const refetch = vi.fn()
    fields.current = { isSuccess: false, isError: true, data: undefined, refetch }
    render(menu(ownedByMe))

    fireEvent.click(screen.getByRole('button', { name: 'Views' }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Fields didn’t load, so the view can’t be saved. Try again' }))
    expect(refetch).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('menuitem', { name: 'Save as new view…' })).toBeNull()
    expect(createTaskView).not.toHaveBeenCalled()
  })

  it('saves a view without field filters while the fields are still loading', async () => {
    fields.current = { isSuccess: false, isError: false, data: undefined, refetch: vi.fn() }
    render(menu(readListState(new URLSearchParams({ priority: 'high' }))))

    fireEvent.click(screen.getByRole('button', { name: 'Views' }))
    expect(await screen.findByRole('menuitem', { name: 'Save as new view…' })).toBeTruthy()
    expect(screen.queryByRole('menuitem', { name: 'Loading fields to save this view…' })).toBeNull()
  })
})
