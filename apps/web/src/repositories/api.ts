import { requestJson } from '../api/client'

export type Repository = {
  id: string
  githubRepositoryId: number
  owner: string
  name: string
  cloneUrl: string
  defaultBranch: string
  enabled: boolean
  trusted: boolean
  version: number
  preparationState: 'pending' | 'ready' | 'failed'
  preparationFailureCategory: string | null
  archivedAt: string | null
  capabilities: { manage: boolean }
}

export async function listRepositories(projectId: string): Promise<{
  repositories: Repository[]
  canManage: boolean
}> {
  const response = await requestJson<{
    repositories?: Repository[]
    capabilities?: { manage?: boolean }
  }>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/repositories`,
  )
  return {
    repositories: Array.isArray(response.repositories) ? response.repositories : [],
    canManage: response.capabilities?.manage === true,
  }
}

export function registerRepository(
  projectId: string,
  input: {
    githubRepositoryId: number
    owner: string
    name: string
    cloneUrl: string
    defaultBranch: string
    trustAcknowledged: boolean
    idempotencyKey: string
  },
) {
  const { idempotencyKey, ...json } = input
  return requestJson<Repository>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/repositories`,
    { method: 'POST', idempotencyKey, json },
  )
}

export function prepareRepository(
  projectId: string,
  repositoryId: string,
  idempotencyKey: string,
) {
  return requestJson<Repository>(repositoryActionPath(projectId, repositoryId, 'prepare'), {
    method: 'POST',
    idempotencyKey,
    json: {},
  })
}

export function archiveRepository(
  projectId: string,
  repositoryId: string,
  expectedVersion: number,
  idempotencyKey: string,
) {
  return requestJson<Repository>(repositoryActionPath(projectId, repositoryId, 'archive'), {
    method: 'POST',
    idempotencyKey,
    json: { expectedVersion },
  })
}

function repositoryActionPath(projectId: string, repositoryId: string, action: string) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/repositories/${encodeURIComponent(repositoryId)}/${action}`
}
