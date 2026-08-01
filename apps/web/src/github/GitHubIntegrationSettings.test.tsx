import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { listRepositories } from '../repositories/api'
import {
  getGitHubAutomationSettings,
  getGitHubStatus,
  syncGitHubInstallation,
  updateGitHubAutomationSettings,
} from './api'
import { GitHubIntegrationSettings } from './GitHubIntegrationSettings'

vi.mock('../repositories/api', () => ({ listRepositories: vi.fn() }))
vi.mock('./api', () => ({
  getGitHubAutomationSettings: vi.fn(),
  getGitHubStatus: vi.fn(),
  syncGitHubInstallation: vi.fn(),
  updateGitHubAutomationSettings: vi.fn(),
}))

describe('GitHubIntegrationSettings', () => {
  beforeEach(() => vi.clearAllMocks())

  it('renders only bounded public installation status', async () => {
    mockAutomationSettings()
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
    expect(
      await screen.findByLabelText('Move explicitly linked eligible work to Verification when merged'),
    ).toBeChecked()
    expect(
      screen.getByLabelText('Move explicitly linked work to Review when a pull request opens'),
    ).not.toBeChecked()
  })

  it('updates project-owned automation with optimistic versioning', async () => {
    mockAutomationSettings()
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
          metadata: 'read', contents: 'write', pullRequests: 'write', checks: 'read', actions: null,
        },
        subscribedEvents: ['pull_request'],
        state: 'available',
        enabled: true,
        version: 1,
        lastAttemptAt: '2026-08-01T08:00:00Z',
        lastSuccessAt: '2026-08-01T08:00:00Z',
        failureCategory: null,
        repositories: [],
      },
      capabilities: { manage: true },
    })
    vi.mocked(updateGitHubAutomationSettings).mockResolvedValue({
      projectId: '019fbc80-0000-7000-8000-000000000003',
      pullRequestOpenEnabled: true,
      pullRequestMergeEnabled: true,
      version: 8,
      updatedAt: '2026-08-01T08:01:00Z',
    })

    renderSettings()
    fireEvent.click(await screen.findByLabelText(
      'Move explicitly linked work to Review when a pull request opens',
    ))

    await waitFor(() => {
      expect(updateGitHubAutomationSettings).toHaveBeenCalledWith(
        '019fbc80-0000-7000-8000-000000000003',
        {
          expectedVersion: 7,
          pullRequestOpenEnabled: true,
          pullRequestMergeEnabled: true,
        },
      )
    })
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

  it('verifies a configured installation against explicitly selected repositories', async () => {
    const projectId = '019fbc80-0000-7000-8000-000000000003'
    const repositoryId = '019fbc80-0000-7000-8000-000000000002'
    vi.mocked(getGitHubStatus).mockResolvedValue({
      configured: true,
      apiVersion: '2026-03-10',
      installation: null,
      capabilities: { manage: true },
    })
    vi.mocked(listRepositories).mockResolvedValue({
      repositories: [
        {
          id: repositoryId,
          githubRepositoryId: 123456,
          owner: 'Mischa-dev',
          name: 'agent-workspace-m0-spike',
          cloneUrl: 'https://github.com/Mischa-dev/agent-workspace-m0-spike.git',
          defaultBranch: 'main',
          enabled: true,
          trusted: true,
          version: 1,
          preparationState: 'ready',
          preparationFailureCategory: null,
          archivedAt: null,
          capabilities: { manage: true },
        },
      ],
      canManage: true,
    })
    vi.mocked(syncGitHubInstallation).mockResolvedValue({
      configured: true,
      apiVersion: '2026-03-10',
      installation: null,
      capabilities: { manage: true },
    })

    renderSettings()

    fireEvent.change(await screen.findByLabelText('GitHub installation ID'), {
      target: { value: '150232852' },
    })
    fireEvent.click(screen.getByLabelText('Mischa-dev/agent-workspace-m0-spike'))
    fireEvent.click(screen.getByRole('button', { name: 'Verify installation' }))

    await waitFor(() => {
      expect(syncGitHubInstallation).toHaveBeenCalledWith(projectId, {
        installationId: 150232852,
        repositoryIds: [repositoryId],
      })
    })
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

function mockAutomationSettings() {
  vi.mocked(getGitHubAutomationSettings).mockResolvedValue({
    projectId: '019fbc80-0000-7000-8000-000000000003',
    pullRequestOpenEnabled: false,
    pullRequestMergeEnabled: true,
    version: 7,
    updatedAt: '2026-08-01T08:00:00Z',
  })
}
