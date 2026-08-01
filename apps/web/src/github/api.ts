import { requestJson } from '../api/client'

export type GitHubInstallationRepository = {
  id: string
  githubRepositoryId: number
  owner: string
  name: string
  accessState: 'selected' | 'missing'
  lastVerifiedAt: string | null
}

export type GitHubInstallation = {
  id: string
  appId: number
  installationId: number
  accountId: number
  accountLogin: string
  targetType: 'User' | 'Organization'
  repositorySelection: 'selected'
  permissions: {
    metadata: 'read'
    contents: 'write'
    pullRequests: 'write'
    checks: 'read'
    actions: 'read' | null
  }
  subscribedEvents: string[]
  state: 'available' | 'unavailable' | 'revoked' | 'misconfigured'
  enabled: boolean
  version: number
  lastAttemptAt: string
  lastSuccessAt: string | null
  failureCategory: string | null
  repositories: GitHubInstallationRepository[]
}

export type GitHubStatus = {
  configured: boolean
  apiVersion: string
  installation: GitHubInstallation | null
  capabilities: { manage: boolean }
}

export type GitHubAutomationSettings = {
  projectId: string
  pullRequestOpenEnabled: boolean
  pullRequestMergeEnabled: boolean
  version: number
  updatedAt: string
}

export function getGitHubStatus(projectId: string) {
  return requestJson<GitHubStatus>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/integrations/github`,
  )
}

export function syncGitHubInstallation(
  projectId: string,
  input: { installationId: number; repositoryIds: string[] },
) {
  return requestJson<GitHubStatus>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/integrations/github/sync`,
    { method: 'POST', json: input },
  )
}

export function getGitHubAutomationSettings(projectId: string) {
  return requestJson<GitHubAutomationSettings>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/integrations/github/automation`,
  )
}

export function updateGitHubAutomationSettings(
  projectId: string,
  input: {
    expectedVersion: number
    pullRequestOpenEnabled: boolean
    pullRequestMergeEnabled: boolean
  },
) {
  return requestJson<GitHubAutomationSettings>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/integrations/github/automation`,
    { method: 'PUT', idempotencyKey: crypto.randomUUID(), json: input },
  )
}
