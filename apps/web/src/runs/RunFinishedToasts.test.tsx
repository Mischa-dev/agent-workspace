import { act, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { AuthenticatedSessionProvider, type AuthenticatedSession } from '../auth/session'
import { announceRunFinished } from '../realtime/runFinished'
import type { RealtimeEnvelope } from '../realtime/realtimeModel'
import { ToastViewport, clearToasts } from '../ui'
import { viewRun } from './api'
import RunFinishedToasts from './RunFinishedToasts'

vi.mock('./api', () => ({ viewRun: vi.fn() }))

const me = 'actor-me'
const session: AuthenticatedSession = {
  authenticated: true,
  user: { id: 'user-me', actorId: me, actorKind: 'human', loginName: 'me', displayName: 'Me', capabilities: { createProject: true } },
  organizations: [],
}

const finished = (runId: string) =>
  ({ type: 'run.changed', payload: { runId, change: 'finished', state: 'done' } }) as unknown as RealtimeEnvelope

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthenticatedSessionProvider session={session}>
        <RunFinishedToasts />
        <ToastViewport />
      </AuthenticatedSessionProvider>
    </MemoryRouter>,
  )
}

const run = (overrides: Record<string, unknown>) =>
  ({
    id: 'run-1',
    state: 'done',
    requestedByActorId: me,
    agentName: 'Builder',
    taskKey: 'AWD-1',
    taskTitle: 'Fix the export',
    threadName: null,
    title: 'Run',
    pullRequestUrl: 'https://example.test/pr/7',
    pullRequestNumber: 7,
    ...overrides,
  }) as never

afterEach(() => {
  clearToasts()
  vi.mocked(viewRun).mockReset()
})

describe('RunFinishedToasts', () => {
  it('says a run you asked for finished, with Open and its pull request', async () => {
    vi.mocked(viewRun).mockResolvedValue(run({}))
    renderAt('/w/acme/inbox')
    act(() => announceRunFinished(finished('run-1')))
    expect(await screen.findByText('AWD-1 Fix the export finished')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Open' })).toBeVisible()
    expect(screen.getByRole('link', { name: 'Pull request #7' })).toHaveAttribute('href', 'https://example.test/pr/7')
  })

  it('stays quiet for someone else’s run and on the run’s own page', async () => {
    vi.mocked(viewRun).mockResolvedValue(run({ requestedByActorId: 'someone-else' }))
    renderAt('/w/acme/inbox')
    act(() => announceRunFinished(finished('run-1')))
    await waitFor(() => expect(viewRun).toHaveBeenCalled())
    expect(screen.queryByText(/finished/)).toBeNull()

    vi.mocked(viewRun).mockClear()
    renderAt('/w/acme/runs/run-2')
    act(() => announceRunFinished(finished('run-2')))
    expect(screen.queryByText(/finished/)).toBeNull()
  })
})
