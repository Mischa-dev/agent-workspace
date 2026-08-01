import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getGitHubStatus } from './api'
import { GitHubIntegrationSettings } from './GitHubIntegrationSettings'

vi.mock('./api', () => ({ getGitHubStatus: vi.fn() }))

describe('GitHubIntegrationSettings', () => {
  beforeEach(() => vi.clearAllMocks())

  it('renders only bounded public installation status', async () => {
    vi.mocked(getGitHubStatus).mockResolvedValue({
      configured: true,
      apiVersion: '2026-03-10',
      installation: {
        id: '019fbc80-0000-7000-8000-000000000001',
        appId: 4442593,
        installationId: 150232852,
        accountId: 777,
        accountLogin: 'Mischa-dev',
        targetType: 'Organization',
        repositorySelection: 'selected',
        permissions: {
          metadata: 'read',
          contents: 'write',
          pullRequests: 'write',
          checks: 'read',
          actions: null,
        },
        subscribedEvents: ['push', 'pull_request'],
        state: 'available',
        enabled: true,
        version: 1,
        lastAttemptAt: '2026-08-01T08:00:00Z',
        lastSuccessAt: '2026-08-01T08:00:00Z',
        failureCategory: null,
        repositories: [
          {
            id: '019fbc80-0000-7000-8000-000000000002',
            githubRepositoryId: 123456,
            owner: 'Mischa-dev',
            name: 'agent-workspace-m0-spike',
            accessState: 'selected',
            lastVerifiedAt: '2026-08-01T08:00:00Z',
          },
        ],
      },
      capabilities: { manage: true },
    })

    renderSettings()

    expect(await screen.findByRole('heading', { name: 'Mischa-dev' })).toBeVisible()
    expect(screen.getByText('available')).toBeVisible()
    expect(screen.getByText(/Metadata read · Contents write/)).toBeVisible()
    expect(
      screen.getByText('Mischa-dev/agent-workspace-m0-spike'),
    ).toBeVisible()
    expect(screen.queryByText(/private key|webhook secret|token value/i)).not.toBeInTheDocument()
  })

  it('shows the operator boundary when host credentials are absent', async () => {
    vi.mocked(getGitHubStatus).mockResolvedValue({
      configured: false,
      apiVersion: '2026-03-10',
      installation: null,
      capabilities: { manage: true },
    })
    renderSettings()
    expect(await screen.findByText('Not configured')).toBeVisible()
    expect(screen.getByText(/host operator must install/i)).toBeVisible()
  })
})

function renderSettings() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <GitHubIntegrationSettings projectId="019fbc80-0000-7000-8000-000000000003" />
    </QueryClientProvider>,
  )
}
