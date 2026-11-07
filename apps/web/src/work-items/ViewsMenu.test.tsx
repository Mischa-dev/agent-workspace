import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { FieldDefinition } from '../fields/fieldModel'
import { createView, type SavedView } from '../views/api'
import { readListState, type ListState } from './listModel'
import ViewsMenu from './ViewsMenu'

const fields = vi.hoisted(() => ({ current: {} as Record<string, unknown> }))

vi.mock('../fields/api', () => ({ useFields: () => fields.current }))
vi.mock('../experience/layout', () => ({ useProjectExperience: () => ({ shows: () => true }) }))
vi.mock('../views/api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../views/api')>()),
  listViews: vi.fn(async () => ({ views: [], canShare: true })),
  createView: vi.fn(),
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

afterEach(() => vi.clearAllMocks())

describe('ViewsMenu', () => {
  it('waits for the project fields before saving field filters, then saves "me" for a person field', async () => {
    fields.current = { isSuccess: false, isError: false, data: undefined, refetch: vi.fn() }
    vi.mocked(createView).mockResolvedValue({ id: 'view-a', name: 'Mine', href: '/x', pinned: true } as SavedView)
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
    await waitFor(() => expect(createView).toHaveBeenCalledTimes(1))
    expect(vi.mocked(createView).mock.calls[0][1].query.fieldFilters).toBe(
      JSON.stringify([{ field: owner.id, op: 'is', values: ['me'] }]),
    )
  })

  it('offers to load the fields again when they failed, instead of saving', async () => {
    const refetch = vi.fn()
    fields.current = { isSuccess: false, isError: true, data: undefined, refetch }
    render(menu(ownedByMe))

    fireEvent.click(screen.getByRole('button', { name: 'Views' }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Fields didn’t load, so the view can’t be saved. Try again' }))
    expect(refetch).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('menuitem', { name: 'Save as new view…' })).toBeNull()
    expect(createView).not.toHaveBeenCalled()
  })

  it('saves a view without field filters while the fields are still loading', async () => {
    fields.current = { isSuccess: false, isError: false, data: undefined, refetch: vi.fn() }
    render(menu(readListState(new URLSearchParams({ priority: 'high' }))))

    fireEvent.click(screen.getByRole('button', { name: 'Views' }))
    expect(await screen.findByRole('menuitem', { name: 'Save as new view…' })).toBeTruthy()
    expect(screen.queryByRole('menuitem', { name: 'Loading fields to save this view…' })).toBeNull()
  })
})
