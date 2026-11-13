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

/** The apps a setup link writes its steps for. */
export type SetupApp = 'claude_code' | 'codex' | 'other'

/** A one-time link that hands an agent its token, with the steps for its app. */
export type SetupLink = {
  url: string
  app: SetupApp
  expiresAt: string
}

export type ExternalAgentMutation = {
  connection: ExternalAgentConnection
  oneTimeCredential: OneTimeCredential | null
  /** Set instead of the token when the agent fetches it through a setup link. */
  setupLink?: SetupLink | null
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
    /** Hand the token over through a one-time setup link for this app. */
    setupApp?: SetupApp
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

/** A new token behind a one-time setup link for `app`; the old token stops working. */
export function createExternalAgentSetupLink(
  projectId: string,
  connectionId: string,
  expectedVersion: number,
  app: SetupApp,
) {
  return requestJson<ExternalAgentMutation>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents/${encodeURIComponent(connectionId)}/setup-link`,
    { method: 'POST', json: { expectedVersion, app } },
  )
}

/** What the person pastes into the agent's app. */
export function setupMessage(handle: string, url: string) {
  return `Set yourself up as @${handle} in Agent Workspace: fetch ${url} once and follow the steps it gives you. The link works once, within 30 minutes.`
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

export type TestWakeupResult = {
  ok: boolean
  status: number | null
  error: string | null
  elapsedMs: number
  deliveryId: string
}

/** Send the runner a signed test wakeup now, and say what it answered. */
export function testExternalAgentWebhook(projectId: string, connectionId: string) {
  return requestJson<TestWakeupResult>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-agents/${encodeURIComponent(connectionId)}/webhook/test`,
    { method: 'POST' },
  )
}
