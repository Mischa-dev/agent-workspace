import { requestJson } from '../../api/client'

export type HarnessTransport = 'command' | 'runner'

export type CustomHarness = {
  id: string
  kind: string
  name: string
  transport: HarnessTransport
  /** The manifest as it was registered. */
  manifest: Record<string, unknown>
  /** What the server made of it; null when it no longer loads. */
  resolved: { displayName?: string; description?: string; defaultModels?: { id: string; displayName?: string }[] } | null
  command: string[] | null
  secretNames: string[]
  runnerActorId: string | null
  runnerName: string | null
  enabled: boolean
  version: number
  createdAt: string
  updatedAt: string
}

export type HarnessList = {
  harnesses: CustomHarness[]
  canManage: boolean
  /** False until the server owner sets AGENT_WORKSPACE_COMMAND_HARNESS_DIRS. */
  commandsAllowed: boolean
  runners: { actorId: string; displayName: string }[]
}

export type HarnessCreate = {
  name: string
  transport: HarnessTransport
  manifest: unknown
  command?: string[]
  secretNames?: string[]
  runnerActorId?: string
}

export type HarnessUpdate = {
  version: number
  name?: string
  manifest?: unknown
  command?: string[]
  secretNames?: string[]
  runnerActorId?: string
  enabled?: boolean
}

export const harnessKeys = {
  list: (organizationId: string) => ['organization', organizationId, 'harnesses'] as const,
}

const base = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/harnesses`

export function listHarnesses(organizationId: string) {
  return requestJson<HarnessList>(base(organizationId))
}

export function createHarness(organizationId: string, input: HarnessCreate) {
  return requestJson<CustomHarness>(base(organizationId), { method: 'POST', json: input, idempotencyKey: crypto.randomUUID() })
}

export function updateHarness(organizationId: string, harnessId: string, input: HarnessUpdate) {
  return requestJson<CustomHarness>(`${base(organizationId)}/${encodeURIComponent(harnessId)}`, {
    method: 'PATCH',
    json: input,
    idempotencyKey: crypto.randomUUID(),
  })
}

export function removeHarness(organizationId: string, harnessId: string) {
  return requestJson<void>(`${base(organizationId)}/${encodeURIComponent(harnessId)}`, { method: 'DELETE', idempotencyKey: crypto.randomUUID() })
}

export const STARTER_MANIFEST = `{
  "name": "My agent",
  "description": "What this agent is for.",
  "models": [{ "id": "my-model", "displayName": "My model" }],
  "features": { "approvals": false, "questions": false, "commands": true },
  "maxConcurrentRuns": 1
}`

/** Lines (or, for names, commas) into a clean list. Command arguments keep their inner spaces. */
export function splitLines(text: string, commas = false) {
  return text
    .split(commas ? /[\n,]/ : /\n/)
    .map((part) => part.trim())
    .filter(Boolean)
}

/** The manifest text as JSON, or a readable reason it isn't an object. */
export function parseManifest(text: string): { value: Record<string, unknown> } | { error: string } {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch (error) {
    return { error: `The manifest isn’t valid JSON: ${error instanceof Error ? error.message : 'check it'}.` }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return { error: 'The manifest must be a JSON object.' }
  return { value: value as Record<string, unknown> }
}
