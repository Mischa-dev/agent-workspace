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
  runtimeKind: string
  runtimeVersion: string
  scopes: ExternalAgentScope[]
  status: 'active' | 'disabled' | 'removed'
  version: number
  lastUsedAt: string | null
  lastErrorCategory: string | null
  disabledAt: string | null
  removedAt: string | null
  createdAt: string
  updatedAt: string
  webhookUrl?: string | null
  lastWakeupAt?: string | null
  lastWakeupError?: string | null
  /** `inbox`: the old inbox.available wakeups, until the webhook is saved again. */
  wakeupFormat?: 'typed' | 'inbox'
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
  /** Shown once, when the agent was created with a webhook. */
  webhookSecret?: string | null
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
    /** Where to send wakeups; sets the runner up in one step. */
    webhookUrl?: string
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

/** Set (or clear, with null) where wakeups are sent. Returns a new signing secret. */
export function setExternalAgentWebhook(
  projectId: string,
  connectionId: string,
  url: string | null,
) {
  return requestJson<{
    connection: ExternalAgentConnection
    webhookSecret: string | null
  }>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents/${encodeURIComponent(connectionId)}/webhook`,
    { method: 'POST', json: { url } },
  )
}
