import { requestJson } from '../api/client'

export type RunDefaults = { model?: string | null; reasoning?: string | null; speed?: string | null }

export type ProfileTemplate = {
  id: string
  name: string
  handle: string
  avatar: string
  color: string
  description: string
  instructions: string
  runDefaults: RunDefaults
}

export type ProfileHarness = {
  kind: string
  displayName: string
  /** The harness takes instructions as instructions; otherwise they're added to the prompt. */
  takesInstructions: boolean
  takesAllowedTools: boolean
}

export type AgentProfile = {
  id: string
  projectId: string
  actorId: string
  name: string
  handle: string
  template: string | null
  avatar: string | null
  color: string | null
  description: string | null
  harness: string | null
  harnessName: string | null
  runDefaults: RunDefaults
  instructions: string
  allowedTools: string[]
  environmentId: string | null
  environmentName: string | null
  enabled: boolean
  version: number
  createdAt: string
  updatedAt: string
}

export type ProfileList = {
  profiles: AgentProfile[]
  templates: ProfileTemplate[]
  harnesses: ProfileHarness[]
  canManage: boolean
  /** False until the project has a default agent to copy run access from. */
  ready: boolean
}

export type ProfileInput = {
  template?: string
  name?: string
  handle?: string
  avatar?: string
  color?: string
  description?: string
  /** Null goes back to the project's default harness. */
  harness?: string | null
  runDefaults?: RunDefaults
  instructions?: string
  allowedTools?: string[]
  environmentId?: string | null
  enabled?: boolean
  version?: number
}

export type PresenceState = 'idle' | 'working' | 'waiting'

export type AgentPresence = {
  actorId: string
  name: string
  handle: string
  kind: 'worker' | 'agent'
  profileId: string | null
  avatar: string | null
  color: string | null
  description: string | null
  harness: string | null
  state: PresenceState
  active: number
  since: string | null
  conversationId: string | null
  threadName: string | null
  runId: string | null
  taskId: string | null
  taskKey: string | null
  taskTitle: string | null
}

export const agentKeys = {
  profiles: (projectId: string) => ['project', projectId, 'agent-profiles'] as const,
  presence: (projectId: string) => ['project', projectId, 'agents'] as const,
}

/** Presence changes without an event of its own, so it's polled. */
export const PRESENCE_REFRESH_MS = 15_000

const projectPath = (projectId: string, rest: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/${rest}`

export function listProfiles(projectId: string) {
  return requestJson<ProfileList>(projectPath(projectId, 'agent-profiles'))
}

export function createProfile(projectId: string, input: ProfileInput) {
  return requestJson<AgentProfile>(projectPath(projectId, 'agent-profiles'), {
    method: 'POST',
    json: input,
    idempotencyKey: crypto.randomUUID(),
  })
}

export function updateProfile(projectId: string, profileId: string, input: ProfileInput & { version: number }) {
  return requestJson<AgentProfile>(projectPath(projectId, `agent-profiles/${encodeURIComponent(profileId)}`), {
    method: 'PATCH',
    json: input,
    idempotencyKey: crypto.randomUUID(),
  })
}

export function listPresence(projectId: string) {
  return requestJson<{ agents: AgentPresence[] }>(projectPath(projectId, 'agents'))
}
