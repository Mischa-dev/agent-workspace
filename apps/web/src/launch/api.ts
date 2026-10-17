import { requestJson } from '../api/client'

/** Where a value came from: set on this launch, the project, you, the workspace, or the built-in default. */
export type SettingSource = 'launch' | 'project' | 'user' | 'workspace' | 'default'

export type StartWhen = 'now' | 'at' | 'window_reset' | 'blockers_done'

export type LaunchState = 'queued' | 'started' | 'failed' | 'cancelled'

export type Environment = {
  id: string
  name: string
  harness: string
  harnessName: string
  runsOn: 'local_host' | 'cloud' | null
  providerEnvironmentId: string | null
  repositoryId: string | null
  branch: string | null
  setupNotes: string
  isDefault: boolean
  lastCheckedAt: string | null
  lastCheckOk: boolean | null
  lastCheckMessage: string | null
  version: number
  createdAt: string
  updatedAt: string
}

export type HarnessChoice = {
  kind: string
  displayName: string
  runsOn: 'local_host' | 'cloud'
  needsProviderEnvironment: boolean
}

export type EnvironmentList = {
  environments: Environment[]
  defaultEnvironmentId: string | null
  defaultSource: SettingSource
  harnesses: HarnessChoice[]
  canManage: boolean
}

export type EnvironmentInput = {
  name: string
  harness: string
  providerEnvironmentId?: string | null
  repositoryId?: string | null
  branch?: string | null
  setupNotes?: string
  makeDefault?: boolean
  expectedVersion?: number
}

export type LaunchTask = { id: string; key: string; title: string }

export type Launch = {
  id: string
  workItem: LaunchTask | null
  conversationId: string | null
  requestedByActorId: string
  requestedByName: string
  source: string
  environmentId: string | null
  environmentName: string | null
  harness: string | null
  harnessName: string | null
  startWhen: StartWhen
  startAt: string | null
  state: LaunchState
  statusReason: string | null
  agentRunId: string | null
  runState: string | null
  position: number
  createdAt: string
  startedAt: string | null
  canCancel: boolean
}

export type ModelChoice = {
  id: string
  displayName: string
  reasoningLevels: string[]
  speeds: string[]
  isDefault: boolean
}

export type HarnessPlan = {
  kind: string
  displayName: string
  runsOn: 'local_host' | 'cloud'
  maxAttempts: number
  models: ModelChoice[]
}

export type AccountChoice = {
  id: string
  name: string
  harness: string
  running: number
  maxConcurrentRuns: number
}

export type SettingChoice = { value: unknown; source: SettingSource }

export type Blocker = { id: string; projectId: string; key: string; title: string }

export type LaunchPlan = {
  workItem: LaunchTask
  blockers: Blocker[]
  environments: Environment[]
  defaultEnvironmentId: string | null
  environmentId: string | null
  harness: string | null
  harnesses: HarnessPlan[]
  accounts: AccountChoice[]
  settings: Partial<Record<'run.model' | 'run.reasoning' | 'run.speed' | 'run.attempts', SettingChoice>>
  prompt: string
  templateSource: SettingSource
  launches: Launch[]
  threadId: string | null
}

export type LaunchOverrides = {
  harness?: string
  model?: string
  reasoning?: string
  speed?: string
  attempts?: number
}

export type LaunchInput = {
  workItemIds: string[]
  environmentId?: string
  overrides?: LaunchOverrides
  providerAccountId?: string
  prompt?: string
  startWhen?: StartWhen
  startAt?: string
  queue?: boolean
  dialog?: boolean
}

export type LaunchList = { queued: Launch[]; recent: Launch[] }

export type AgentSession = {
  id: string
  conversationId: string
  agentActorId: string
  agentName: string
  source: string
  harness: string
  harnessName: string
  externalRef: string
  url: string | null
  state: 'active' | 'idle' | 'ended' | 'cancelled'
  agentRunId: string | null
  lastActivityAt: string
  createdAt: string
}

export type SettingWrite = { setting: { key: string; value: unknown; source: SettingSource } }

export type EffectiveSetting = { key: string; value: unknown; source: SettingSource; default: unknown }

export type EffectiveSettings = {
  settings: EffectiveSetting[]
  capabilities: { manageWorkspace: boolean; manageProject: boolean; manageOwn: boolean }
}

const projectPath = (projectId: string, rest: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/${rest}`

export function getLaunchPlan(projectId: string, workItemId: string, choice: { environmentId?: string; harness?: string } = {}) {
  const query = new URLSearchParams()
  if (choice.environmentId) query.set('environmentId', choice.environmentId)
  if (choice.harness) query.set('harness', choice.harness)
  const suffix = query.size ? `?${query.toString()}` : ''
  return requestJson<LaunchPlan>(projectPath(projectId, `work-items/${encodeURIComponent(workItemId)}/launch${suffix}`))
}

export function createLaunches(projectId: string, input: LaunchInput, idempotencyKey: string) {
  return requestJson<{ launches: Launch[] }>(projectPath(projectId, 'launches'), { method: 'POST', json: input, idempotencyKey })
}

export function listLaunches(projectId: string) {
  return requestJson<LaunchList>(projectPath(projectId, 'launches'))
}

export function cancelLaunch(projectId: string, launchId: string) {
  return requestJson<Launch>(projectPath(projectId, `launches/${encodeURIComponent(launchId)}/cancel`), {
    method: 'POST',
    json: {},
    idempotencyKey: crypto.randomUUID(),
  })
}

export function listEnvironments(projectId: string) {
  return requestJson<EnvironmentList>(projectPath(projectId, 'environments'))
}

export function createEnvironment(projectId: string, input: EnvironmentInput) {
  return requestJson<Environment>(projectPath(projectId, 'environments'), { method: 'POST', json: input, idempotencyKey: crypto.randomUUID() })
}

export function updateEnvironment(projectId: string, environmentId: string, input: EnvironmentInput) {
  return requestJson<Environment>(projectPath(projectId, `environments/${encodeURIComponent(environmentId)}`), {
    method: 'PUT',
    json: input,
    idempotencyKey: crypto.randomUUID(),
  })
}

export function environmentAction(projectId: string, environmentId: string, action: 'archive' | 'default') {
  return requestJson<EnvironmentList>(projectPath(projectId, `environments/${encodeURIComponent(environmentId)}/${action}`), {
    method: 'POST',
    json: {},
    idempotencyKey: crypto.randomUUID(),
  })
}

export function checkEnvironment(projectId: string, environmentId: string) {
  return requestJson<{ ok: boolean; message: string; environment: Environment }>(
    projectPath(projectId, `environments/${encodeURIComponent(environmentId)}/check`),
    { method: 'POST', json: {}, idempotencyKey: crypto.randomUUID() },
  )
}

export function listAgentSessions(conversationId: string) {
  return requestJson<{ sessions: AgentSession[] }>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/agent-sessions`)
}

export function getProjectSettings(projectId: string) {
  return requestJson<EffectiveSettings>(`/api/v1/settings/effective?projectId=${encodeURIComponent(projectId)}`)
}

export function setProjectSetting(projectId: string, key: string, value: unknown) {
  return requestJson<SettingWrite>(`/api/v1/settings/values/${encodeURIComponent(key)}`, {
    method: 'PUT',
    json: { scope: 'project', projectId, value },
    idempotencyKey: crypto.randomUUID(),
  })
}

export function resetProjectSetting(projectId: string, key: string) {
  return requestJson<SettingWrite>(`/api/v1/settings/values/${encodeURIComponent(key)}/reset`, {
    method: 'POST',
    json: { scope: 'project', projectId },
    idempotencyKey: crypto.randomUUID(),
  })
}
