import {
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import axe from 'axe-core'
import { describe, expect, it } from 'vitest'

import {
  projectAId,
  threadId,
  timeline,
  jsonResponse,
  installStandardApi,
  installAppShellLifecycle,
} from '../test/appHarness'
import { App } from './App'

describe('App Thread runs, approvals and GitHub', () => {
  installAppShellLifecycle()

  it('preserves the hosted Thread timeline, hostile-output escaping, and approval flow', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const fetchMock = installStandardApi()
    const { container } = render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Show details' }))
    expect(await screen.findByRole('heading', { name: 'Details' })).toBeVisible()
    expect(
      screen.getByText('<img src=x onerror=alert(1)>'),
    ).toBeVisible()
    expect(container.querySelector('img')).toBeNull()
    expect(
      screen.getByRole('button', { name: 'Stop run' }),
    ).toBeVisible()
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: 'Allow once' }))
    await waitFor(() => {
      expect(
        fetchMock.mock.calls.some(([path]) =>
          String(path).includes('/agent-approvals/approval-1/respond'),
        ),
      ).toBe(true)
    })
  })

  it('submits a bounded provider question answer instead of an approval decision', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const questionTimeline = {
      ...timeline,
      entries: timeline.entries.map((entry) =>
        entry.workerEvent?.kind === 'approval_requested'
          ? {
              ...entry,
              workerEvent: {
                ...entry.workerEvent,
                kind: 'question_requested',
                summary: 'The Worker needs a release name.',
              },
            }
          : entry,
      ),
      approvals: [
        {
          ...timeline.approvals[0],
          kind: 'question' as const,
          summary: 'What should the release be called?',
          options: [],
        },
      ],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(questionTimeline)
      }
      if (path.includes('/agent-approvals/approval-1/respond')) {
        return jsonResponse({
          ...questionTimeline.approvals[0],
          status: 'resolved',
        })
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { name: 'Codex has a question' }),
    ).toBeVisible()
    fireEvent.change(screen.getByLabelText('Your answer'), {
      target: { value: 'Aurora' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send answer' }))
    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes('/agent-approvals/approval-1/respond'),
      )
      expect(request).toBeDefined()
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({ answer: 'Aurora' })
    })
    expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull()
  })

  it('retries the latest terminal run with a stable explicit command', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const terminalRun = {
      ...timeline.activeRun,
      state: 'failed',
      failureReason: 'The service restarted; the worktree was preserved.',
      capabilities: { interrupt: false, retry: true },
    }
    const terminalTimeline = {
      ...timeline,
      activeRun: null,
      latestTerminalRun: terminalRun,
      approvals: [],
      entries: [
        ...timeline.entries,
        {
          id: 'entry-failed',
          sequence: 3,
          kind: 'worker_event',
          createdAt: '2026-07-31T04:02:00Z',
          message: null,
          workerEvent: {
            id: 'event-failed',
            agentRunId: 'run-1',
            kind: 'run_failed',
            summary: 'The run failed and its worktree was preserved.',
            payload: {},
          },
        },
      ],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (path.includes('/agent-runs/run-1/retry')) {
        return jsonResponse({
          runId: 'run-2',
          state: 'queued',
          repositoryId: 'repo-1',
          repositoryWorkspaceId: null,
          branch: null,
          baseCommit: null,
          queuePosition: 1,
        }, 202)
      }
      if (path.includes('/agent-runs/run-1/acknowledge-failure')) {
        return jsonResponse({})
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Dismiss failure' }),
    )
    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes('/agent-runs/run-1/acknowledge-failure'),
      )
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({
        eventId: 'event-failed',
      })
    })
    expect(
      screen.queryByRole('button', { name: 'Dismiss failure' }),
    ).toBeNull()
    fireEvent.click(
      screen.getByRole('button', { name: 'Run again' }),
    )
    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes('/agent-runs/run-1/retry'),
      )
      expect(new Headers(request?.[1]?.headers).get('idempotency-key')).toBe(
        'retry-run-1',
      )
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({})
    })
  })

  it('queues publication for the exact terminal run through the Project-scoped boundary', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const terminalTimeline = {
      ...timeline,
      activeRun: null,
      latestTerminalRun: {
        ...timeline.activeRun,
        state: 'completed',
        capabilities: { interrupt: false, retry: false, publish: true },
      },
      approvals: [],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (
        path.includes(
          `/projects/${projectAId}/agent-runs/run-1/publish`,
        )
      ) {
        return jsonResponse({ jobId: 'publish-job-1', state: 'queued' }, 202)
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Publish branch' }),
    )

    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes(
          `/projects/${projectAId}/agent-runs/run-1/publish`,
        ),
      )
      expect(request).toBeDefined()
      expect(request?.[1]?.method).toBe('POST')
      expect(
        new Headers(request?.[1]?.headers).get('idempotency-key'),
      ).toMatch(/^[0-9a-f-]{36}$/i)
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({})
    })
  })

  it('creates a pull request only from the exact published run and explicit Work Item links', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const terminalTimeline = {
      ...timeline,
      linkedWorkItems: [
        {
          id: '019fbc00-0000-7000-8000-000000000001',
          key: 'AW-42',
          title: 'Ship exact publication',
          statusCategory: 'in_progress',
          archivedAt: null,
          href: `/projects/${projectAId}/work-items/AW-42`,
        },
      ],
      activeRun: null,
      latestTerminalRun: {
        ...timeline.activeRun,
        state: 'completed',
        publishedCommit: '0123456789012345678901234567890123456789',
        capabilities: {
          interrupt: false,
          retry: false,
          publish: false,
          createPullRequest: true,
        },
      },
      approvals: [],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (
        path.includes(
          `/projects/${projectAId}/agent-runs/run-1/pull-request`,
        )
      ) {
        return jsonResponse({ requestId: 'pull-request-1', state: 'queued' }, 202)
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Create pull request' }),
    )

    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes(
          `/projects/${projectAId}/agent-runs/run-1/pull-request`,
        ),
      )
      expect(request).toBeDefined()
      expect(request?.[1]?.method).toBe('POST')
      expect(
        new Headers(request?.[1]?.headers).get('idempotency-key'),
      ).toMatch(/^[0-9a-f-]{36}$/i)
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({
        title: 'AW-42: Ship exact publication',
        body: 'Created from an explicitly published Agent Workspace run.',
        workItemIds: ['019fbc00-0000-7000-8000-000000000001'],
      })
    })
  })

  it('renders canonical GitHub state and queues an explicit pull request refresh', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const pullRequestId = '019fbc00-0000-7000-8000-000000000099'
    const terminalTimeline = {
      ...timeline,
      activeRun: null,
      latestTerminalRun: {
        ...timeline.activeRun,
        state: 'completed',
        pullRequestId,
        pullRequestNumber: 7,
        pullRequestUrl:
          'https://github.com/Mischa-dev/agent-workspace-m0-spike/pull/7',
        pullRequestTitle: 'Canonical review state',
        pullRequestState: 'open',
        pullRequestDraft: false,
        pullRequestChecksState: 'success',
        pullRequestReviewState: 'approved',
        pullRequestMergeabilityState: 'ready',
        pullRequestSyncState: 'fresh',
        pullRequestSyncFailureCategory: null,
        capabilities: {
          interrupt: false,
          retry: false,
          publish: false,
          createPullRequest: false,
          refreshPullRequest: true,
        },
      },
      approvals: [],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (
        path.includes(
          `/projects/${projectAId}/pull-requests/${pullRequestId}/refresh`,
        )
      ) {
        return jsonResponse({ jobId: 'refresh-1', state: 'queued' }, 202)
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    expect(await screen.findByText('Checks passed')).toBeVisible()
    expect(screen.getByText(/review approved · ready/)).toBeVisible()
    fireEvent.click(
      screen.getByRole('button', { name: 'Refresh GitHub status' }),
    )

    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes(
          `/projects/${projectAId}/pull-requests/${pullRequestId}/refresh`,
        ),
      )
      expect(request?.[1]?.method).toBe('POST')
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({})
    })
  })
})
