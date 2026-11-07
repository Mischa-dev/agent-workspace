import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import MoveTaskDialog from './MoveTaskDialog'
import { moveTask, previewMove, type MovePlan } from './moveApi'

const { plan, risk } = vi.hoisted(() => {
  const risk = {
    id: 'risk-dst',
    name: 'Risk',
    description: '',
    type: 'single_choice' as const,
    options: [
      { id: 'low', name: 'Low', color: '#33aa66', archived: false },
      { id: 'medium', name: 'Medium', color: '#cc9933', archived: false },
    ],
    target: null,
    position: 1,
    archived: false,
    version: 1,
  }
  const plan: MovePlan = {
    targetProjectId: 'dst',
    targetProjectKey: 'DST',
    tasks: [
      { id: 'root', key: 'SRC-1', title: 'Root' },
      { id: 'sub', key: 'SRC-2', title: 'Sub', parentId: 'root' },
    ],
    carried: [{ taskId: 'root', taskKey: 'SRC-1', field: 'Notes', targetField: 'Notes', value: 'plain note' }],
    issues: [
      {
        id: 'root:risk',
        taskId: 'root',
        taskKey: 'SRC-1',
        fieldId: 'risk-src',
        field: 'Risk',
        fieldType: 'single_choice',
        reason: 'option_missing',
        message: 'DST’s "Risk" doesn’t have this option.',
        value: 'High',
        candidates: [{ id: 'risk-dst', name: 'Risk', takesValue: false, options: ['Low', 'Medium'] }],
      },
      {
        id: 'root:secret',
        taskId: 'root',
        taskKey: 'SRC-1',
        fieldId: 'secret',
        field: 'Secret',
        fieldType: 'text',
        reason: 'no_matching_field',
        message: 'DST has no text field called "Secret".',
        value: 'private words',
        candidates: [{ id: 'summary', name: 'Summary', takesValue: true }],
      },
      {
        id: 'sub:follows',
        taskId: 'sub',
        taskKey: 'SRC-2',
        fieldId: 'follows',
        field: 'Follows',
        fieldType: 'link',
        linkTarget: 'task',
        reason: 'link_outside_move',
        message: 'The linked task stays where it is.',
        value: 'SRC-3 Stays',
      },
    ],
    snapshot: 'snapshot-1',
  }
  return { plan, risk }
})

vi.mock('../projects/hooks', () => ({
  useProjects: () => ({
    data: {
      projects: [
        { id: 'src', key: 'SRC', name: 'Source', archivedAt: null },
        { id: 'dst', key: 'DST', name: 'Destination', archivedAt: null },
        { id: 'old', key: 'OLD', name: 'Old', archivedAt: '2026-01-01T00:00:00Z' },
      ],
    },
    isLoading: false,
    isError: false,
  }),
}))
vi.mock('../fields/api', () => ({ useFields: () => ({ data: { fields: [risk], canManage: false } }) }))
vi.mock('./moveApi', () => ({
  previewMove: vi.fn(async () => plan),
  moveTask: vi.fn(async () => ({ moved: [{ from: 'SRC-1', to: 'DST-1', id: 'new-root', title: 'Root', url: '' }], created: true, leftBehind: 2 })),
}))

function renderDialog() {
  const onClose = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter>
        <MoveTaskDialog onClose={onClose} projectId="src" task={{ id: 'root', key: 'SRC-1', title: 'Root' }} />
      </MemoryRouter>
    </QueryClientProvider>,
  )
  return onClose
}

async function review() {
  fireEvent.change(screen.getByLabelText('Project'), { target: { value: 'dst' } })
  fireEvent.click(screen.getByRole('button', { name: 'Review the move' }))
  await screen.findByRole('heading', { name: 'Needs a choice' })
}

const where = (field: string, key: string) => screen.getByLabelText(`What happens to ${field} on ${key}`)
const sent = () => vi.mocked(moveTask).mock.calls

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('MoveTaskDialog (AWD-102)', () => {
  it('offers only other, active projects and previews the one picked', async () => {
    renderDialog()
    const options = Array.from((screen.getByLabelText('Project') as HTMLSelectElement).options).map((option) => option.value)
    expect(options).toEqual(['', 'dst'])
    await review()
    expect(previewMove).toHaveBeenCalledWith('src', 'root', 'dst')
    expect(screen.getByText(/plain note/)).toBeTruthy()
    expect(screen.getByText(/private words/)).toBeTruthy()
  })

  it('moves once, with every choice, only when each issue is settled', async () => {
    renderDialog()
    await review()
    const confirm = screen.getByRole('button', { name: 'Move 2 tasks' }) as HTMLButtonElement
    expect(confirm.disabled).toBe(true)

    // Put the secret in Summary as it is; leave the outside link.
    fireEvent.change(where('Secret', 'SRC-1'), { target: { value: 'set:summary' } })
    fireEvent.change(where('Follows', 'SRC-2'), { target: { value: 'leave' } })
    // Risk needs a value in the target's field before it counts.
    fireEvent.change(where('Risk', 'SRC-1'), { target: { value: 'set:risk-dst' } })
    expect(screen.getByText('Choose a value')).toBeTruthy()
    expect(confirm.disabled).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Choose…' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Medium' }))
    await waitFor(() => expect(confirm.disabled).toBe(false))

    fireEvent.click(confirm)
    fireEvent.click(confirm)
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(1))
    const [project, task, input, key] = sent()[0]
    expect([project, task]).toEqual(['src', 'root'])
    expect(input).toEqual({
      targetProjectId: 'dst',
      snapshot: 'snapshot-1',
      leaveUnresolved: false,
      resolutions: [
        { issue: 'root:risk', action: 'set', targetField: 'risk-dst', value: 'medium' },
        { issue: 'root:secret', action: 'set', targetField: 'summary' },
        { issue: 'sub:follows', action: 'leave' },
      ],
    })
    expect(key).toMatch(/[0-9a-f-]{36}/)
  })

  it('leaves everything unresolved at once', async () => {
    renderDialog()
    await review()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(screen.getByRole('button', { name: 'Move 2 tasks' }))
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(1))
    expect(sent()[0][2].resolutions.map((resolution) => resolution.action)).toEqual(['leave', 'leave', 'leave'])
  })

  it('retries a lost response with the same key, and changed choices with a new one', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    renderDialog()
    await review()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    const confirm = screen.getByRole('button', { name: 'Move 2 tasks' })
    fireEvent.click(confirm)
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(1))
    await waitFor(() => expect((confirm as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(confirm)
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(2))
    expect(sent()[1][3]).toBe(sent()[0][3])

    cleanup()
    vi.mocked(moveTask).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    renderDialog()
    await review()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(screen.getByRole('button', { name: 'Move 2 tasks' }))
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(3))
    fireEvent.change(where('Secret', 'SRC-1'), { target: { value: 'set:summary' } })
    fireEvent.click(screen.getByRole('button', { name: 'Move 2 tasks' }))
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(4))
    expect(sent()[3][3]).not.toBe(sent()[2][3])
  })

  it('asks for a fresh preview when the server says it changed', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(
      new ApiError({ status: 409, code: 'conflict', message: 'Something changed since the preview.', details: null, requestId: null }),
    )
    renderDialog()
    await review()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(screen.getByRole('button', { name: 'Move 2 tasks' }))
    const again = await screen.findByRole('button', { name: 'Preview again' })
    expect((screen.getByRole('button', { name: 'Move 2 tasks' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(again)
    await waitFor(() => expect(previewMove).toHaveBeenCalledTimes(2))
    // Choices start over against the new preview.
    expect((where('Secret', 'SRC-1') as HTMLSelectElement).value).toBe('')
  })

  it('goes back, cancels and closes without moving anything', async () => {
    const onClose = renderDialog()
    await review()
    fireEvent.change(where('Follows', 'SRC-2'), { target: { value: 'leave' } })
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect(screen.getByLabelText('Project')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Review the move' }))
    await screen.findByRole('heading', { name: 'Needs a choice' })
    expect((where('Follows', 'SRC-2') as HTMLSelectElement).value).toBe('')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(onClose).toHaveBeenCalled()
    expect(moveTask).not.toHaveBeenCalled()
  })
})
