import { requestJson } from '../api/client'
import { callOp } from '../api/ops'

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

/** The repository as the space's list shows it, after a write that answers with a short form. */
async function readRepository(projectId: string, written: Record<string, unknown>): Promise<Repository> {
  const { repositories } = await listRepositories(projectId)
  const repository = repositories.find((candidate) => candidate.id === written.id)
  if (!repository) throw new Error('That repository is no longer in this space.')
  return repository
}

export async function registerRepository(
  projectId: string,
  input: {
    githubRepositoryId: number
    owner: string
    name: string
    defaultBranch: string
    trustAcknowledged: boolean
    idempotencyKey: string
  },
) {
  const { idempotencyKey, ...fields } = input
  const { repository } = await callOp('repository_create', { projectId, ...fields }, { requestId: idempotencyKey })
  return readRepository(projectId, repository)
}

export async function prepareRepository(
  projectId: string,
  repositoryId: string,
  idempotencyKey: string,
) {
  const { repository } = await callOp('repository_prepare', { projectId, repository: repositoryId }, { requestId: idempotencyKey })
  return readRepository(projectId, repository)
}

export async function archiveRepository(
  projectId: string,
  repositoryId: string,
  expectedVersion: number,
  idempotencyKey: string,
) {
  const { repository } = await callOp(
    'repository_archive',
    { projectId, repository: repositoryId },
    { requestId: idempotencyKey, expectedVersion },
  )
  return readRepository(projectId, repository)
}
