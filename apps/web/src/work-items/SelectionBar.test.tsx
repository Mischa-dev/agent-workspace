import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { toIsoDate } from '../ui/pickers'
import { bulkUpdateWorkItems } from './planningApi'
import { SelectionBar } from './SelectionBar'
import type { WorkItemCard } from './types'

const { areas, launch, risk } = vi.hoisted(() => ({
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
    position: 2,
    archived: false,
    version: 1,
  },
  launch: {
    id: 'launch',
    name: 'Launch',
    description: '',
    type: 'date' as const,
    options: [],
    target: null,
    position: 3,
    archived: false,
    version: 1,
  },
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

vi.mock('../fields/api', () => ({ useFields: () => ({ data: { fields: [risk, areas, launch], canManage: false } }) }))
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

  it('sets a date from the calendar, and clears it', async () => {
    renderBar()
    const today = toIsoDate(new Date())

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Set Launch' }))
    // The same calendar as the task page's date picker, not a text box.
    expect(await screen.findByRole('grid')).toBeTruthy()
    expect(screen.queryByRole('textbox', { name: /Launch/ })).toBeNull()
    const day = document.querySelector<HTMLButtonElement>(`[data-date="${today}"]`)
    expect(day).not.toBeNull()
    fireEvent.click(day as HTMLButtonElement)
    await waitFor(() => expect(bulkUpdateWorkItems).toHaveBeenCalledTimes(1))
    expect(sent()[0]).toMatchObject({ changes: { fieldValues: { launch: today } } })

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Clear Launch' }))
    await waitFor(() => expect(bulkUpdateWorkItems).toHaveBeenCalledTimes(2))
    expect(sent()[1]).toMatchObject({ changes: { fieldValues: { launch: null } } })
  })

  it('sends a multiple choice once, with every option chosen, when its editor closes', async () => {
    renderBar()

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Set Areas' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Ops' }))
    fireEvent.click(screen.getByRole('option', { name: 'Research' }))
    expect(bulkUpdateWorkItems).not.toHaveBeenCalled()
    fireEvent.keyDown(screen.getByRole('dialog', { name: 'Areas' }), { key: 'Escape' })
    await waitFor(() => expect(bulkUpdateWorkItems).toHaveBeenCalledTimes(1))
    // In the field's own order, as the server stores them.
    expect(sent()[0]).toMatchObject({ changes: { fieldValues: { areas: ['research', 'ops'] } } })
  })

  it('changes nothing when a multiple choice is toggled back to empty, or closed untouched', async () => {
    renderBar()

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Set Areas' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Research' }))
    fireEvent.click(screen.getByRole('option', { name: 'Research' }))
    fireEvent.keyDown(screen.getByRole('dialog', { name: 'Areas' }), { key: 'Escape' })

    fireEvent.click(screen.getByRole('button', { name: 'Field' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Set Areas' }))
    fireEvent.keyDown(await screen.findByRole('dialog', { name: 'Areas' }), { key: 'Escape' })

    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(bulkUpdateWorkItems).not.toHaveBeenCalled()
  })
})
