import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useState } from 'react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from '../api/errors'
import { AuthenticatedSessionProvider, clearAuthenticatedQueryCache, type AuthenticatedSession } from '../auth/session'
import MoveTaskDialog from './MoveTaskDialog'
import { listMoveDestinations, moveTask, previewMove, type MoveDone, type MovePlan } from './moveApi'
import { clearPendingMoves } from './moveRecovery'
import { FieldEditor } from './TaskCustomFields'
import { clearToasts, useToasts } from '../ui/toastStore'

const { owner, plan, risk } = vi.hoisted(() => {
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
  const owner = { ...risk, id: 'owner-dst', name: 'Owner', type: 'person' as const, options: [], position: 2 }
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
        id: 'root:owner',
        taskId: 'root',
        taskKey: 'SRC-1',
        fieldId: 'owner-src',
        field: 'Owner',
        fieldType: 'person',
        reason: 'person_not_in_target',
        message: 'This person can’t hold tasks in the target project.',
        value: 'Pat',
        candidates: [{ id: 'owner-dst', name: 'Owner', takesValue: false }],
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
  return { owner, plan, risk }
})

vi.mock('../fields/api', () => ({ useFields: () => ({ data: { fields: [risk, owner], canManage: false } }) }))
vi.mock('./hooks', () => ({
  useWorkItem: () => ({ data: undefined }),
  useAssignableActors: () => ({ data: [{ id: 'dee', displayName: 'Dee Lane', handle: 'dee', kind: 'human' }], isFetching: false, isError: false }),
}))
vi.mock('./moveApi', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./moveApi')>()),
  listMoveDestinations: vi.fn(),
  previewMove: vi.fn(),
  moveTask: vi.fn(),
}))

const done: MoveDone = {
  moved: [{ from: 'SRC-1', to: 'DST-1', id: 'new-root', title: 'Root', url: 'http://localhost/projects/dst/work-items/new-root' }],
  created: true,
  leftBehind: 2,
}

function Where() {
  return <output data-testid="where">{useLocation().pathname}</output>
}

function Toasts() {
  return (
    <ul data-testid="toasts">
      {useToasts().map((toast) => (
        <li key={toast.id}>
          {toast.title} {toast.description}
        </li>
      ))}
    </ul>
  )
}

/** The task page's part: a button that opens the dialog, which unmounts when it closes. */
function Host({ version = 1 }: { version?: number }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button onClick={() => setOpen(true)} type="button">
        Move to project…
      </button>
      {open ? <MoveTaskDialog FieldEditor={FieldEditor} onClose={() => setOpen(false)} projectId="src" task={{ id: 'root', key: 'SRC-1', title: 'Root', version }} /> : null}
      <Where />
      <Toasts />
    </>
  )
}

const sessionFor = (actorId: string) => ({ authenticated: true, user: { id: `user-${actorId}`, actorId } }) as unknown as AuthenticatedSession

function renderHost() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const tree = (version: number, actorId: string) => (
    <QueryClientProvider client={client}>
      <AuthenticatedSessionProvider session={sessionFor(actorId)}>
        <MemoryRouter initialEntries={['/projects/src/work-items/root']}>
          <Host version={version} />
        </MemoryRouter>
      </AuthenticatedSessionProvider>
    </QueryClientProvider>
  )
  const view = render(tree(1, 'actor-a'))
  const rerender = (version: number, actorId = 'actor-a') => view.rerender(tree(version, actorId))
  return { client, rerender }
}

async function openReview() {
  fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
  await screen.findByRole('option', { name: 'Destination (DST)' })
  fireEvent.change(screen.getByLabelText('Project'), { target: { value: 'dst' } })
  fireEvent.click(screen.getByRole('button', { name: 'Review the move' }))
  await screen.findByRole('heading', { name: 'Needs a choice' })
}

const where = (field: string, key: string) => screen.getByLabelText(`What happens to ${field} on ${key}`) as HTMLSelectElement
const confirmButton = () => screen.getByRole('button', { name: /^Move (2 tasks|task)$/ }) as HTMLButtonElement
const calls = () => vi.mocked(moveTask).mock.calls

/** Settles every issue: Risk to Medium, Owner to Dee (picked by name), the outside link left. */
async function settleAll() {
  fireEvent.change(where('Risk', 'SRC-1'), { target: { value: 'set:risk-dst' } })
  fireEvent.click(screen.getAllByRole('button', { name: 'Choose…' })[0])
  fireEvent.click(await screen.findByRole('option', { name: 'Medium' }))
  fireEvent.change(where('Owner', 'SRC-1'), { target: { value: 'set:owner-dst' } })
  fireEvent.click(screen.getByRole('button', { name: 'Choose…' }))
  fireEvent.click(await screen.findByRole('option', { name: /Dee Lane/ }))
  fireEvent.change(where('Follows', 'SRC-2'), { target: { value: 'leave' } })
}

beforeEach(() => {
  vi.mocked(listMoveDestinations).mockResolvedValue({ projects: [{ id: 'dst', key: 'DST', name: 'Destination' }] })
  vi.mocked(previewMove).mockResolvedValue(plan)
  vi.mocked(moveTask).mockResolvedValue(done)
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.mocked(previewMove).mockReset()
  vi.mocked(moveTask).mockReset()
  clearPendingMoves()
  clearToasts()
})

describe('MoveTaskDialog (AWD-102)', () => {
  it('offers the projects the server says the task can move into', async () => {
    renderHost()
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByRole('option', { name: 'Destination (DST)' })
    expect(listMoveDestinations).toHaveBeenCalledWith('src', 'root')
    const options = Array.from((screen.getByLabelText('Project') as HTMLSelectElement).options).map((option) => option.value)
    expect(options).toEqual(['', 'dst'])
  })

  it('moves once, with every choice, after each issue is settled, showing who and what was picked', async () => {
    const { client } = renderHost()
    const invalidate = vi.spyOn(client, 'invalidateQueries')
    await openReview()
    expect(previewMove).toHaveBeenCalledWith('src', 'root', 'dst')
    expect(confirmButton().disabled).toBe(true)
    await settleAll()
    expect(screen.getByText('Medium')).toBeTruthy()
    expect(screen.getByText('Dee Lane (@dee)')).toBeTruthy()
    await waitFor(() => expect(confirmButton().disabled).toBe(false))

    // A second click lands on nothing: the move is already on its way.
    const button = confirmButton()
    fireEvent.click(button)
    fireEvent.click(button)
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(1))
    const [projectId, taskId, body, key] = calls()[0]
    expect([projectId, taskId]).toEqual(['src', 'root'])
    expect(body).toEqual({
      targetProjectId: 'dst',
      snapshot: 'snapshot-1',
      leaveUnresolved: false,
      resolutions: [
        { issue: 'root:risk', action: 'set', targetField: 'risk-dst', value: 'medium' },
        { issue: 'root:owner', action: 'set', targetField: 'owner-dst', value: 'dee' },
        { issue: 'sub:follows', action: 'leave' },
      ],
    })
    expect(key).toMatch(/[0-9a-f-]{36}/)
    // Both projects' tasks are read again, and the new task opens.
    await waitFor(() => expect(screen.getByTestId('where').textContent).toBe('/projects/dst/work-items/new-root'))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['project', 'src', 'work-items'] })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['project', 'dst', 'work-items'] })
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('leaves everything unresolved at once', async () => {
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(1))
    expect(calls()[0][2].resolutions.map((resolution) => resolution.action)).toEqual(['leave', 'leave', 'leave'])
  })

  it('after a lost answer, only sends that same move again, even after closing and reopening', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    // Nothing can be changed while the outcome is unknown.
    expect(screen.queryByLabelText('What happens to Risk on SRC-1')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Back' })).toBeNull()

    // Closing unmounts the dialog; reopening finds the same move waiting.
    fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[0])
    expect(screen.queryByRole('dialog')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    expect(screen.queryByLabelText('Project')).toBeNull()

    vi.mocked(moveTask).mockRejectedValueOnce(new ApiError({ status: 502, code: 'bad_gateway', message: 'Bad gateway' }))
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(2))
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    await waitFor(() => expect(moveTask).toHaveBeenCalledTimes(3))
    // Each attempt is the very same request.
    expect(calls()[1]).toEqual(calls()[0])
    expect(calls()[2]).toEqual(calls()[0])
    await waitFor(() => expect(screen.getByTestId('where').textContent).toBe('/projects/dst/work-items/new-root'))
  })

  it('after a lost answer that turns out refused, starts over', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    vi.mocked(moveTask).mockRejectedValueOnce(
      new ApiError({ status: 409, code: 'conflict', message: 'Something changed since the preview.' }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    await screen.findByRole('heading', { name: 'That move request wasn’t carried out' })
    fireEvent.click(screen.getByRole('button', { name: 'Start over' }))
    expect(screen.getByLabelText('Project')).toBeTruthy()
  })

  it('asks for a fresh preview, and allows nothing until one arrives', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new ApiError({ status: 409, code: 'conflict', message: 'Something changed since the preview.' }))
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    const again = await screen.findByRole('button', { name: 'Preview again' })
    expect(confirmButton().disabled).toBe(true)

    // The refresh is slow, then fails: the old plan is gone and nothing can be sent.
    let fail!: (error: unknown) => void
    vi.mocked(previewMove).mockReturnValueOnce(new Promise<MovePlan>((_, reject) => (fail = reject)))
    fireEvent.click(again)
    await screen.findByText('Checking what carries over…')
    expect(screen.queryByLabelText('What happens to Risk on SRC-1')).toBeNull()
    expect(confirmButton().disabled).toBe(true)
    await act(async () => fail(new ApiError({ status: 403, code: 'forbidden', message: 'You can’t see tasks in DST.' })))
    await screen.findByText('You can’t see tasks in DST.')
    expect(confirmButton().disabled).toBe(true)

    // Trying again brings a new preview, with no choices carried over.
    vi.mocked(previewMove).mockResolvedValueOnce({ ...plan, snapshot: 'snapshot-2' })
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await screen.findByRole('heading', { name: 'Needs a choice' })
    expect(where('Follows', 'SRC-2').value).toBe('')
    expect(confirmButton().disabled).toBe(true)
  })

  it('starts the review over when the task changes or a new preview replaces the old one', async () => {
    const { client, rerender } = renderHost()
    await openReview()
    fireEvent.change(where('Follows', 'SRC-2'), { target: { value: 'leave' } })
    expect(where('Follows', 'SRC-2').value).toBe('leave')

    // Another preview arrives in the background.
    act(() => {
      client.setQueryData(['task-move-preview', 'src', 'root', 1, 'dst'], { ...plan, snapshot: 'snapshot-b' })
    })
    await waitFor(() => expect(where('Follows', 'SRC-2').value).toBe(''))

    // The task itself changes while the dialog is open.
    fireEvent.change(where('Follows', 'SRC-2'), { target: { value: 'leave' } })
    rerender(2)
    await waitFor(() => expect(previewMove).toHaveBeenCalledTimes(2))
    await screen.findByRole('heading', { name: 'Needs a choice' })
    expect(where('Follows', 'SRC-2').value).toBe('')
  })

  it('goes back, closes on Escape and gives focus back to what opened it', async () => {
    renderHost()
    const opener = screen.getByRole('button', { name: 'Move to project…' })
    opener.focus()
    await openReview()
    fireEvent.change(where('Follows', 'SRC-2'), { target: { value: 'leave' } })
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    fireEvent.click(screen.getByRole('button', { name: 'Review the move' }))
    await screen.findByRole('heading', { name: 'Needs a choice' })
    expect(where('Follows', 'SRC-2').value).toBe('')
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(document.activeElement).toBe(opener)
    expect(moveTask).not.toHaveBeenCalled()
  })

  it('keeps a lost move only for the person who sent it, until anyone signs in or out', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    const { client, rerender } = renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    expect(screen.getByText(/reloading or signing out forgets it/)).toBeTruthy()
    fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[0])

    // Someone else in this tab never sees it, and can't send it.
    rerender(1, 'actor-b')
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByRole('option', { name: 'Destination (DST)' })
    expect(screen.queryByRole('heading', { name: 'Did SRC-1 move to DST?' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    // Its sender still does, until a session change forgets it.
    rerender(1, 'actor-a')
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[0])
    act(() => clearAuthenticatedQueryCache(client))
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByLabelText('Project')
    expect(screen.queryByRole('heading', { name: 'Did SRC-1 move to DST?' })).toBeNull()
    expect(moveTask).toHaveBeenCalledTimes(1)
  })

  it('treats an unreadable answer, even a 200, as unknown', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new ApiError({ status: 200, code: 'invalid_response', message: 'The server response was not valid.' }))
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    expect(screen.queryByRole('heading', { name: 'That move request wasn’t carried out' })).toBeNull()
  })

  it('stays unsure when a check is refused for lost access', async () => {
    vi.mocked(moveTask).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Did SRC-1 move to DST?' })
    for (const status of [401, 403, 404]) {
      vi.mocked(moveTask).mockRejectedValueOnce(new ApiError({ status, code: 'forbidden', message: 'You can’t change tasks there.' }))
      fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
      await screen.findByText('Last answer: You can’t change tasks there.')
      expect(screen.getByRole('heading', { name: 'Did SRC-1 move to DST?' })).toBeTruthy()
    }
    expect(moveTask).toHaveBeenCalledTimes(4)
    expect(calls().every((call) => call[3] === calls()[0][3])).toBe(true)
  })

  it('sends one move while it is on its way, even when the task changes underneath', async () => {
    let answer!: (done: MoveDone) => void
    vi.mocked(moveTask).mockReturnValueOnce(new Promise<MoveDone>((resolve) => (answer = resolve)))
    const { rerender } = renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Moving SRC-1 to DST…' })
    rerender(2)
    expect(screen.getByRole('heading', { name: 'Moving SRC-1 to DST…' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Move 2 tasks' })).toBeNull()
    // Closing and reopening still shows it on its way.
    fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[0])
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByRole('heading', { name: 'Moving SRC-1 to DST…' })
    expect(moveTask).toHaveBeenCalledTimes(1)
    await act(async () => answer(done))
    await waitFor(() => expect(screen.getByTestId('where').textContent).toBe('/projects/dst/work-items/new-root'))
    expect(moveTask).toHaveBeenCalledTimes(1)
  })

  it('lets an answer that comes after signing out change nothing', async () => {
    let fail!: (error: unknown) => void
    vi.mocked(moveTask).mockReturnValueOnce(new Promise<MoveDone>((_, reject) => (fail = reject)))
    const { client } = renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Moving SRC-1 to DST…' })
    act(() => clearAuthenticatedQueryCache(client))
    await act(async () => fail(new TypeError('Failed to fetch')))
    expect(screen.queryByRole('heading', { name: 'Did SRC-1 move to DST?' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByLabelText('Project')
    expect(screen.queryByRole('heading', { name: 'Did SRC-1 move to DST?' })).toBeNull()
  })

  it('after a move finishes with the dialog closed, reads both projects again and stays on the page', async () => {
    let answer!: (done: MoveDone) => void
    vi.mocked(moveTask).mockReturnValueOnce(new Promise<MoveDone>((resolve) => (answer = resolve)))
    const { client } = renderHost()
    const invalidate = vi.spyOn(client, 'invalidateQueries')
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await screen.findByRole('heading', { name: 'Moving SRC-1 to DST…' })
    fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[0])
    await act(async () => answer(done))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['project', 'src', 'work-items'] })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['project', 'dst', 'work-items'] })
    expect(screen.getByTestId('where').textContent).toBe('/projects/src/work-items/root')
    // Opening it again says where it went, and goes there only if asked.
    fireEvent.click(screen.getByRole('button', { name: 'Move to project…' }))
    await screen.findByRole('heading', { name: 'SRC-1 moved to DST-1' })
    expect(screen.getByTestId('where').textContent).toBe('/projects/src/work-items/root')
    fireEvent.click(screen.getByRole('button', { name: 'Open DST-1' }))
    await waitFor(() => expect(screen.getByTestId('where').textContent).toBe('/projects/dst/work-items/new-root'))
    expect(moveTask).toHaveBeenCalledTimes(1)
  })

  it('follows a task that was already moved elsewhere to where it really is, and says the choices were not applied', async () => {
    vi.mocked(moveTask).mockResolvedValueOnce({
      moved: [{ from: 'SRC-1', to: 'CCC-4', id: 'c-copy', title: '', url: 'http://localhost/projects/ccc/work-items/c-copy' }],
      created: false,
      notes: ['This task had already been moved, so this request wasn’t carried out and its choices weren’t applied.'],
    })
    const { client } = renderHost()
    const invalidate = vi.spyOn(client, 'invalidateQueries')
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    // Asked for DST; it went to CCC before: that's where it opens, not a made-up DST page.
    await waitFor(() => expect(screen.getByTestId('where').textContent).toBe('/projects/ccc/work-items/c-copy'))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['project', 'ccc', 'work-items'] })
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: ['project', 'dst', 'work-items'] })
    const toasts = screen.getByTestId('toasts').textContent ?? ''
    expect(toasts).toContain('SRC-1 was already moved to CCC-4')
    expect(toasts).toContain('its choices weren’t applied')
  })

  it('opens nothing when the answer has no link to a task page', async () => {
    vi.mocked(moveTask).mockResolvedValueOnce({
      moved: [{ from: 'SRC-1', to: 'CCC-4', id: 'c-copy', title: '', url: '' }],
      created: false,
    })
    renderHost()
    await openReview()
    fireEvent.click(screen.getByRole('button', { name: 'Leave all unresolved' }))
    fireEvent.click(confirmButton())
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(screen.getByTestId('where').textContent).toBe('/projects/src/work-items/root')
  })
})
