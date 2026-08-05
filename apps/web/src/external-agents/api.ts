import { requestJson } from '../api/client'

export type ExternalAgentRole = 'member' | 'viewer'
export type ExternalAgentScope =
  | 'read'
  | 'write'
  | 'events'
  | 'approval_respond'
  | 'worker_request'

export type ExternalAgentConnection = {
  id: string
  projectId: string
  actorId: string
  connectionName: string
  displayName: string
  handle: string
  role: ExternalAgentRole
  runtimeKind: 'hermes'
  runtimeVersion: '0.20.0'
  scopes: ExternalAgentScope[]
  status: 'active' | 'disabled' | 'removed'
  version: number
  lastUsedAt: string | null
  lastErrorCategory: string | null
  disabledAt: string | null
  removedAt: string | null
  createdAt: string
  updatedAt: string
}

export type OneTimeCredential = {
  id: string
  token: string
  tokenPrefix: string
  expiresAt: string
}

export type ExternalAgentMutation = {
  connection: ExternalAgentConnection
  oneTimeCredential: OneTimeCredential | null
}

export function listExternalAgents(projectId: string) {
  return requestJson<{ connections: ExternalAgentConnection[] }>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents`,
  )
}

export function createExternalAgent(
  projectId: string,
  input: {
    connectionName: string
    displayName: string
    handle: string
    role: ExternalAgentRole
    scopes: ExternalAgentScope[]
  },
) {
  return requestJson<ExternalAgentMutation>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents`,
    { method: 'POST', json: input },
  )
}

export function updateExternalAgent(
  projectId: string,
  connectionId: string,
  input: {
    expectedVersion: number
    role?: ExternalAgentRole
    enabled?: boolean
    scopes?: ExternalAgentScope[]
  },
) {
  return requestJson<ExternalAgentMutation>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents/${encodeURIComponent(connectionId)}`,
    { method: 'PATCH', json: input },
  )
}

export function rotateExternalAgentCredential(
  projectId: string,
  connectionId: string,
  expectedVersion: number,
) {
  return requestJson<ExternalAgentMutation>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents/${encodeURIComponent(connectionId)}/credentials/rotate`,
    { method: 'POST', json: { expectedVersion } },
  )
}

export function removeExternalAgent(
  projectId: string,
  connectionId: string,
  expectedVersion: number,
) {
  return requestJson<ExternalAgentMutation>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents/${encodeURIComponent(connectionId)}/remove`,
    { method: 'POST', json: { expectedVersion } },
  )
}
