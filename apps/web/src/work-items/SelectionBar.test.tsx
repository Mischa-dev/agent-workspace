import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { bulkUpdateWorkItems } from './planningApi'
import { SelectionBar } from './SelectionBar'
import type { WorkItemCard } from './types'

const { risk } = vi.hoisted(() => ({
  risk: {
    id: 'risk',
    name: 'Risk',
    description: '',
    type: 'single_choice' as const,
    options: [
      { id: 'low', name: 'Low', color: '#33aa66', archived: false },
      { id: 'high', name: 'High', color: '#cc3333', archived: false },
    ],
    target: null,
    position: 1,
    archived: false,
    version: 1,
  },
}))

vi.mock('../fields/api', () => ({ useFields: () => ({ data: { fields: [risk], canManage: false } }) }))
vi.mock('../experience/layout', () => ({ useProjectExperience: () => ({ shows: () => true }) }))
vi.mock('./planningApi', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./planningApi')>()),
  bulkUpdateWorkItems: vi.fn(async () => ({ items: [] })),
}))

const cards = [
  { id: 'task-a', key: 'AW-1', version: 3, archivedAt: null },
  { id: 'task-b', key: 'AW-2', version: 5, archivedAt: null },
] as unknown as WorkItemCard[]

function renderBar() {
  const notify = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <SelectionBar busy={false} notify={notify} onClear={vi.fn()} onPick={vi.fn()} projectId="project-a" selected={cards} />
      </MemoryRouter>
    </QueryClientProvider>,
  )
  return notify
}

const sent = () => vi.mocked(bulkUpdateWorkItems).mock.calls.map(([input]) => input)

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('SelectionBar custom fields (AWD-101)', () => {
  it('clears a field on every selected task in one change', async () => {
    const notify = renderBar()

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Clear Risk' }))
    await waitFor(() => expect(bulkUpdateWorkItems).toHaveBeenCalledTimes(1))
    expect(sent()[0]).toMatchObject({
      projectId: 'project-a',
      items: [
        { id: 'task-a', expectedVersion: 3 },
        { id: 'task-b', expectedVersion: 5 },
      ],
      changes: { fieldValues: { risk: null } },
    })
    await waitFor(() => expect(notify).toHaveBeenCalledWith(expect.objectContaining({ title: 'Cleared Risk on 2 tasks' })))
  })

  it('sets a field in its own editor, once, when the editor closes', async () => {
    renderBar()

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Set Risk' }))
    fireEvent.click(await screen.findByRole('option', { name: 'High' }))
    await waitFor(() => expect(bulkUpdateWorkItems).toHaveBeenCalledTimes(1))
    expect(sent()[0]).toMatchObject({ changes: { fieldValues: { risk: 'high' } } })
  })
})
