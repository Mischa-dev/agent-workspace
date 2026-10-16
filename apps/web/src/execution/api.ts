import { requestJson } from '../api/client'

/** The settings a run uses. Every field is optional at launch; the server
 * fills the rest from the account's defaults and the harness catalog, and
 * freezes the result on the run. */
export type RunSettings = {
  harness?: string
  model?: string
  reasoning?: string
  speed?: string
  attempts?: number
  sandbox?: string
  approvalPolicy?: string
  options?: Record<string, unknown>
}

export type LoginKind = 'browser' | 'device_code' | 'api_key'

export type SpeedOption = { id: string; name: string }

export type ModelPrices = {
  currency: string
  inputPerMillion: number
  outputPerMillion: number
  cachedInputPerMillion?: number | null
}

export type CatalogModel = {
  id: string
  displayName: string
  description?: string | null
  contextWindow?: number | null
  reasoningLevels: string[]
  defaultReasoning?: string | null
  speeds: SpeedOption[]
  defaultSpeed?: string | null
  prices?: ModelPrices | null
  hidden: boolean
  isDefault: boolean
  source: 'provider' | 'manual' | 'adapter'
  edited: boolean
  overrides: ModelOverrides
}

export type ModelOverrides = {
  displayName?: string
  description?: string
  contextWindow?: number
  reasoningLevels?: string[]
  defaultReasoning?: string
  speeds?: SpeedOption[]
  defaultSpeed?: string
  prices?: ModelPrices
  hidden?: boolean
  isDefault?: boolean
}

/** What a harness adapter publishes about itself, plus this workspace's
 * model catalog for it. The settings pages are drawn from this, so a new
 * harness needs no web changes. */
export type Harness = {
  kind: string
  displayName: string
  description: string
  runsOn: 'local_host' | 'cloud'
  features: Record<string, boolean>
  concurrency: { defaultPerAccount: number; maxPerAccount: number }
  loginMethods: { kind: LoginKind; label: string }[]
  runOptions: { sandboxModes: string[]; approvalPolicies: string[]; maxAttempts: number }
  catalog: { models: CatalogModel[]; refreshedAt: string | null }
  canEditCatalog: boolean
}

export type ProviderAccount = {
  id: string
  adapterKind: string
  harnessName: string | null
  name: string
  status: string
  accountState: string
  maxConcurrentRuns: number
  runDefaults: RunSettings
  lastHealthAt: string | null
  version: number
  /** The profile using this account in the project, if it's used here. */
  profileId: string | null
  preferred: boolean
  activeRuns: number
}

export type AccountStatus = {
  accountId: string
  profileId: string | null
  accountState: string
  providerAvailable: boolean
  account: null | {
    accountType: string
    email: string | null
  }
  rateLimits: null | {
    primary: RateLimitWindow | null
    secondary: RateLimitWindow | null
  }
  ceremony: LoginCeremony | null
}

export type RateLimitWindow = {
  usedPercent: number
  resetsAt: number | null
  windowDurationMinutes: number | null
}

export type LoginCeremony = {
  ceremonyId: string
  kind: LoginKind
  state: 'starting' | 'pending' | 'completed'
  expiresAtUnixSeconds: number
  authUrl: string | null
  verificationUrl: string | null
  userCode: string | null
}

const project = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}`
const accountPath = (projectId: string, accountId: string) =>
  `${project(projectId)}/provider-accounts/${encodeURIComponent(accountId)}`

export async function listHarnesses(projectId: string): Promise<Harness[]> {
  const response = await requestJson<{ harnesses: Harness[] }>(`${project(projectId)}/harnesses`)
  return Array.isArray(response.harnesses) ? response.harnesses : []
}

export async function listProviderAccounts(projectId: string): Promise<ProviderAccount[]> {
  const response = await requestJson<{ accounts: ProviderAccount[] }>(`${project(projectId)}/provider-accounts`)
  return Array.isArray(response.accounts) ? response.accounts : []
}

export function createProviderAccount(projectId: string, adapterKind: string, idempotencyKey: string) {
  return requestJson<ProviderAccount>(`${project(projectId)}/provider-accounts`, {
    method: 'POST',
    idempotencyKey,
    json: { adapterKind },
  })
}

export function updateProviderAccount(
  projectId: string,
  accountId: string,
  change: { expectedVersion: number; name?: string; maxConcurrentRuns?: number; runDefaults?: RunSettings },
  idempotencyKey: string,
) {
  return requestJson<ProviderAccount>(accountPath(projectId, accountId), {
    method: 'PATCH',
    idempotencyKey,
    json: change,
  })
}

/** Uses the account in this project and makes it the one your runs here pick first. */
export function useProviderAccount(projectId: string, accountId: string, idempotencyKey: string) {
  return requestJson<ProviderAccount>(`${accountPath(projectId, accountId)}/use`, {
    method: 'POST',
    idempotencyKey,
    json: {},
  })
}

export function getAccountStatus(projectId: string, accountId: string) {
  return requestJson<AccountStatus>(accountPath(projectId, accountId))
}

export function startAccountLogin(
  projectId: string,
  accountId: string,
  kind: 'browser' | 'device_code',
  idempotencyKey: string,
) {
  const segment = kind === 'device_code' ? 'device-code' : 'browser'
  return requestJson<LoginCeremony>(`${accountPath(projectId, accountId)}/login/${segment}`, {
    method: 'POST',
    idempotencyKey,
    json: {},
  })
}

export function loginWithApiKey(projectId: string, accountId: string, apiKey: string, idempotencyKey: string) {
  return requestJson<LoginCeremony>(`${accountPath(projectId, accountId)}/login/api-key`, {
    method: 'POST',
    idempotencyKey,
    json: { apiKey },
  })
}

export function cancelAccountLogin(projectId: string, accountId: string, idempotencyKey: string) {
  return requestJson<AccountStatus>(`${accountPath(projectId, accountId)}/login/cancel`, {
    method: 'POST',
    idempotencyKey,
    json: {},
  })
}

export function logoutAccount(projectId: string, accountId: string, idempotencyKey: string) {
  return requestJson<AccountStatus>(`${accountPath(projectId, accountId)}/logout`, {
    method: 'POST',
    idempotencyKey,
    json: {},
  })
}

export function refreshCatalog(projectId: string, kind: string, idempotencyKey: string) {
  return requestJson<Harness>(`${project(projectId)}/harnesses/${encodeURIComponent(kind)}/catalog/refresh`, {
    method: 'POST',
    idempotencyKey,
    json: {},
  })
}

export function saveModel(
  projectId: string,
  kind: string,
  modelId: string,
  overrides: ModelOverrides,
  idempotencyKey: string,
) {
  return requestJson<Harness>(modelPath(projectId, kind, modelId), {
    method: 'PUT',
    idempotencyKey,
    json: overrides,
  })
}

export function removeModel(projectId: string, kind: string, modelId: string, idempotencyKey: string) {
  return requestJson<Harness>(modelPath(projectId, kind, modelId), {
    method: 'DELETE',
    idempotencyKey,
  })
}

function modelPath(projectId: string, kind: string, modelId: string) {
  return `${project(projectId)}/harnesses/${encodeURIComponent(kind)}/models/${encodeURIComponent(modelId)}`
}

/** The models people can pick for a harness: its catalog without hidden ones. */
export function pickableModels(harness: Harness | undefined): CatalogModel[] {
  return harness?.catalog.models.filter((model) => !model.hidden) ?? []
}

/** Reads a run's frozen settings from an API record, dropping anything malformed. */
export function runSettingsFrom(value: unknown): RunSettings | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  const text = (key: string) => (typeof record[key] === 'string' ? (record[key] as string) : undefined)
  const settings: RunSettings = {
    harness: text('harness'),
    model: text('model'),
    reasoning: text('reasoning'),
    speed: text('speed'),
  }
  return Object.values(settings).some(Boolean) ? settings : null
}
