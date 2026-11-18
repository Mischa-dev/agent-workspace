import { act, render, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { AuthenticatedSessionProvider, type AuthenticatedSession } from '../auth/session'
import { announceRunFinished } from '../realtime/runFinished'
import type { RealtimeEnvelope } from '../realtime/realtimeModel'
import { clearToasts, useToasts } from '../ui'
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

let shown: ReturnType<typeof useToasts> = []
function Toasts() {
  shown = useToasts()
  return null
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthenticatedSessionProvider session={session}>
        <RunFinishedToasts />
        <Toasts />
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
    await waitFor(() => expect(shown).toHaveLength(1))
    expect(shown[0]).toMatchObject({
      title: 'AWD-1 Fix the export finished',
      tone: 'success',
      action: { label: 'Open' },
      link: { label: 'Pull request #7', href: 'https://example.test/pr/7' },
    })
  })

  it('stays quiet for someone else’s run and on the run’s own page', async () => {
    vi.mocked(viewRun).mockResolvedValue(run({ requestedByActorId: 'someone-else' }))
    renderAt('/w/acme/inbox')
    act(() => announceRunFinished(finished('run-1')))
    await waitFor(() => expect(viewRun).toHaveBeenCalled())
    expect(shown).toHaveLength(0)

    vi.mocked(viewRun).mockClear()
    renderAt('/w/acme/runs/run-2')
    act(() => announceRunFinished(finished('run-2')))
    expect(shown).toHaveLength(0)
  })
})
