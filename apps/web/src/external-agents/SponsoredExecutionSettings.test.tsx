import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import axe from 'axe-core'

import { SponsoredExecutionSettings } from './SponsoredExecutionSettings'

const projectId = '019fd200-0000-7000-8000-000000000001'
const requestId = '019fd200-0000-7000-8000-000000000002'
const approvalId = '019fd200-0000-7000-8000-000000000003'
const hostilePrompt = '<img src=x onerror=alert(1)> fix the bounded fixture'

function response(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }),
  )
}

function renderSettings() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
          },
        })
      }
    >
      <SponsoredExecutionSettings projectId={projectId} />
    </QueryClientProvider>,
  )
}

describe('SponsoredExecutionSettings', () => {
  beforeEach(() => {
    document.cookie = 'agent_workspace_csrf=csrf-sponsored; Path=/'
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    document.cookie = 'agent_workspace_csrf=; Max-Age=0; Path=/'
  })

  it('renders hostile prompts inert and explicitly queues only after approval', async () => {
    const writes: RequestInit[] = []
    let resolved = false
    vi.stubGlobal(
      'fetch',
      vi.fn((_input: RequestInfo | URL, init?: RequestInit) => {
        if (init?.method === 'POST') {
          writes.push(init)
          resolved = true
          return response({ id: approvalId, status: 'resolved' })
        }
        return response({
          requests: resolved
            ? []
            : [
                {
                  id: requestId,
                  approvalId,
                  requestingActorId:
                    '019fd200-0000-7000-8000-000000000004',
                  requesterDisplayName: 'Hermes teammate',
                  requesterHandle: 'hermes',
                  conversationId:
                    '019fd200-0000-7000-8000-000000000005',
                  repositoryId:
                    '019fd200-0000-7000-8000-000000000006',
                  prompt: hostilePrompt,
                  status: 'pending',
                  version: 1,
                  agentRunId: null,
                  expiresAt: '2026-08-13T07:00:00Z',
                  createdAt: '2026-08-12T07:00:00Z',
                },
              ],
        })
      }),
    )

    const rendered = renderSettings()
    expect(await screen.findByText(hostilePrompt)).toBeVisible()
    expect(document.querySelector('img')).toBeNull()
    expect(
      (
        await axe.run(rendered.container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])

    fireEvent.click(screen.getByRole('button', { name: 'Approve and queue' }))
    await waitFor(() => expect(writes).toHaveLength(1))
    expect(JSON.parse(String(writes[0].body))).toEqual({ decision: 'approve' })
    const headers = new Headers(writes[0].headers)
    expect(headers.get('x-csrf-token')).toBe('csrf-sponsored')
    expect(headers.get('idempotency-key')).toBe(
      `sponsored-${requestId}-1-approve`,
    )
    expect(
      await screen.findByText('No run requests are waiting for you'),
    ).toBeVisible()
  })
})
