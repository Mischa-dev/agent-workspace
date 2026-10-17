import {
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import {
  projectAId,
  projectBId,
  threadId,
  projectA,
  timeline,
  WebSocketStub,
  jsonResponse,
  installStandardApi,
  installAppShellLifecycle,
} from '../test/appHarness'
import { App } from './App'

describe('App Thread access and capabilities', () => {
  installAppShellLifecycle()

  it('does not remember an active child while browsing archived-parent history', async () => {
    const archivedChildId = '019fb600-0000-7000-8000-000000000014'
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${archivedChildId}?includeArchived=true`,
    )
    let navigationWrites = 0
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${archivedChildId}/timeline`)) {
        return jsonResponse({
          ...timeline,
          conversation: {
            ...timeline.conversation,
            id: archivedChildId,
            archivedAt: null,
          },
        })
      }
      if (
        path.endsWith(`/projects/${projectAId}/navigation`) &&
        init?.method === 'PUT'
      ) {
        navigationWrites += 1
        return jsonResponse(projectA.navigation)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([path]) =>
          String(path).includes(`/conversations/${archivedChildId}/timeline`),
        ),
      ).toBe(true),
    )
    expect(navigationWrites).toBe(0)
  })

  it('rechecks an expired session when the realtime connection closes', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    let expired = false
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (
        expired &&
        String(input).includes(`/conversations/${threadId}/timeline`)
      ) {
        return jsonResponse(
          {
            error: {
              code: 'authentication_required',
              message: 'Sign in to continue.',
            },
          },
          401,
        )
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(WebSocketStub.instances).toHaveLength(1)
    expired = true
    WebSocketStub.instances[0].dispatchEvent(new Event('close'))

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    expect(
      screen.queryByText('<img src=x onerror=alert(1)>'),
    ).not.toBeInTheDocument()
  })

  it('redirects a cross-Project Thread response without exposing its state or actions', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const mismatchedTimeline = {
      ...timeline,
      conversation: {
        ...timeline.conversation,
        projectId: projectBId,
        name: 'Private Project B Thread',
      },
      entries: [
        {
          ...timeline.entries[0],
          message: {
            ...timeline.entries[0].message,
            plainText: 'Project B private content',
          },
        },
      ],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(mismatchedTimeline)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectAId}`),
    )
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Agent Workspace',
      }),
    ).toBeVisible()
    expect(screen.queryByText('Private Project B Thread')).not.toBeInTheDocument()
    expect(screen.queryByText('Project B private content')).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Allow once' }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Stop run' }),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Describe the change for the agent')).not.toBeInTheDocument()
    expect(screen.queryByRole('radio', { name: 'Run an agent' })).not.toBeInTheDocument()
    // Realtime is authenticated at the browser-session boundary, not scoped
    // to a resource that a mismatched response could cause us to subscribe to.
    expect(WebSocketStub.instances).toHaveLength(1)
    expect(
      fetchMock.mock.calls.some(
        ([path]) =>
          String(path).includes('/agent-runs') ||
          String(path).includes('/agent-approvals'),
      ),
    ).toBe(false)
  })

  it.each([
    {
      label: 'a different Conversation ID',
      conversation: {
        ...timeline.conversation,
        id: '019fb600-0000-7000-8000-00000000000c',
      },
    },
    {
      label: 'a Channel kind at the Thread route',
      conversation: { ...timeline.conversation, kind: 'channel' },
    },
  ])('rejects $label even inside the selected Project', async ({ conversation }) => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse({ ...timeline, conversation })
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectAId}`),
    )
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Agent Workspace',
      }),
    ).toBeVisible()
    expect(screen.queryByText('M0 walking skeleton')).not.toBeInTheDocument()
  })

  it('returns a remotely unavailable Thread to its last verified parent Channel', async () => {
    const parentChannelId = '019fb600-0000-7000-8000-00000000000d'
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    let timelineReads = 0
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        timelineReads += 1
        if (timelineReads > 1) {
          return jsonResponse(
            {
              error: {
                code: 'not_found',
                message: 'Conversation not found.',
              },
            },
            404,
          )
        }
        return jsonResponse({
          ...timeline,
          conversation: {
            ...timeline.conversation,
            parentChannelId,
          },
        })
      }
      if (path.includes(`/conversations/${parentChannelId}/timeline`)) {
        return new Promise<Response>(() => {})
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    WebSocketStub.instances[0].dispatchEvent(
      new MessageEvent('message', {
        data: JSON.stringify({
          event_id: '019fb600-0000-7000-8000-00000000000e',
          event_cursor: 14,
          version: 1,
          project_id: projectAId,
          type: 'conversation.archived',
          occurred_at: '2026-07-31T15:02:00Z',
          payload: { conversationId: threadId, version: 2 },
        }),
      }),
    )

    await waitFor(() =>
      expect(window.location.pathname).toBe(
        `/projects/${projectAId}/channels/${parentChannelId}`,
      ),
    )
  })

  it('returns an initially unavailable Thread to the Project overview', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(
          {
            error: {
              code: 'not_found',
              message: 'Conversation not found.',
            },
          },
          404,
        )
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectAId}`),
    )
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Agent Workspace',
      }),
    ).toBeVisible()
  })

  it('keeps Viewer execution controls read-only when capabilities are missing', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const readOnlyTimeline = {
      ...timeline,
      capabilities: {},
      activeRun: timeline.activeRun
        ? { ...timeline.activeRun, capabilities: {} }
        : null,
      approvals: timeline.approvals.map((approval) => ({
        ...approval,
        capabilities: {},
      })),
    }
    const viewerThreadProject = {
      ...projectA,
      role: 'viewer',
      capabilities: {
        view: true,
        manage: false,
        archive: false,
        navigationWrite: false,
      },
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      const method = init?.method ?? 'GET'
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(readOnlyTimeline)
      }
      if (path.startsWith('/api/v1/projects?') && method === 'GET') {
        return jsonResponse({ projects: [viewerThreadProject] })
      }
      if (path.endsWith(`/projects/${projectAId}`)) {
        return jsonResponse(viewerThreadProject)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(screen.getByText('Allow the isolated file change?')).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Allow once' }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Deny' }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Stop run' }),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Describe the change for the agent')).not.toBeInTheDocument()
    expect(screen.queryByRole('radio', { name: 'Run an agent' })).not.toBeInTheDocument()
  })

  it('fails Worker invocation closed when its timeline capability is absent', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const idleReadOnlyTimeline = {
      ...timeline,
      activeRun: null,
      approvals: [],
      capabilities: {},
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(idleReadOnlyTimeline)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Start run' }),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Describe the change for the agent')).not.toBeInTheDocument()
    expect(screen.queryByRole('radio', { name: 'Run an agent' })).not.toBeInTheDocument()
  })

  it('shows Worker invocation only when the server explicitly grants it', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const invokableTimeline = {
      ...timeline,
      activeRun: null,
      approvals: [],
      capabilities: { workerInvoke: true },
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(invokableTimeline)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    // Without message access the thread opens straight into the run composer.
    expect(
      await screen.findByLabelText('Describe the change for the agent'),
    ).toBeVisible()
    expect(screen.queryByRole('radio', { name: 'Run an agent' })).not.toBeInTheDocument()
    expect(
      screen.getByRole('button', { name: 'Start run' }),
    ).toBeDisabled()
  })
})
