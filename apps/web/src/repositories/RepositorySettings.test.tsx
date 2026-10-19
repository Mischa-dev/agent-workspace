import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { RepositorySettings } from './RepositorySettings'

const projectId = '019fb600-0000-7000-8000-000000000201'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('RepositorySettings', () => {
  it('requires explicit trust and registers only the normalized GitHub HTTPS identity', async () => {
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      if (!init?.method || init.method === 'GET') {
        return jsonResponse({ repositories: [], capabilities: { manage: true } })
      }
      const body = JSON.parse(String(init.body)) as Record<string, unknown>
      expect(body).toMatchObject({
        githubRepositoryId: 123456,
        owner: 'Mischa-dev',
        name: 'agent-workspace',
        cloneUrl: 'https://github.com/Mischa-dev/agent-workspace.git',
        defaultBranch: 'main',
        trustAcknowledged: true,
      })
      expect(new Headers(init.headers).get('idempotency-key')).toBe('repository-key')
      return jsonResponse(repository({ preparationState: 'pending' }))
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('crypto', { randomUUID: () => 'repository-key' })

    renderSettings()

    const submit = await screen.findByRole('button', { name: 'Register repository' })
    expect(submit).toBeDisabled()
    fireEvent.change(screen.getByLabelText('GitHub repository ID'), {
      target: { value: '123456' },
    })
    fireEvent.change(screen.getByLabelText('Owner'), {
      target: { value: 'Mischa-dev' },
    })
    fireEvent.change(screen.getByLabelText('Repository'), {
      target: { value: 'agent-workspace' },
    })
    fireEvent.click(
      screen.getByLabelText(/I trust this repository/i),
    )
    expect(submit).toBeEnabled()
    fireEvent.click(submit)

    expect(
      await screen.findByRole('heading', { name: 'Mischa-dev/agent-workspace' }),
    ).toBeVisible()
    expect(screen.getByRole('button', { name: 'Prepare for runs' })).toBeEnabled()
    expect(screen.queryByText(/mirror\.git/)).not.toBeInTheDocument()
  })

  it('keeps non-managers read-only while exposing safe preparation state', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        jsonResponse({
          repositories: [
            repository({
              preparationState: 'failed',
              preparationFailureCategory: 'git_prepare_failed',
              capabilities: { manage: false },
            }),
          ],
          capabilities: { manage: false },
        }),
      ),
    )

    renderSettings()

    expect(
      await screen.findByRole('heading', { name: 'Mischa-dev/agent-workspace' }),
    ).toBeVisible()
    expect(screen.getByText(/git prepare failed/i)).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Prepare for runs' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Archive repository' })).not.toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Register GitHub repository' })).not.toBeInTheDocument()
  })
})

function renderSettings() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <RepositorySettings projectId={projectId} />
    </QueryClientProvider>,
  )
}

function repository(overrides: Record<string, unknown> = {}) {
  return {
    id: '019fb600-0000-7000-8000-000000000202',
    githubRepositoryId: 123456,
    owner: 'Mischa-dev',
    name: 'agent-workspace',
    cloneUrl: 'https://github.com/Mischa-dev/agent-workspace.git',
    defaultBranch: 'main',
    enabled: true,
    trusted: true,
    version: 1,
    preparationState: 'ready',
    preparationFailureCategory: null,
    archivedAt: null,
    capabilities: { manage: true },
    ...overrides,
  }
}

function jsonResponse(value: unknown) {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  })
}
