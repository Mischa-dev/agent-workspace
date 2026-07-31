import { requestJson } from '../api/client'

export type ExecutionProfile = {
  id: string
  name: string
  status: string
  accountState: string
  owner: {
    id: string
    displayName: string
  }
  provider: {
    kind: string
    binaryVersion: string
  }
}

export type AccountStatus = {
  profileId: string
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
  kind: 'browser' | 'device_code' | 'api_key'
  state: 'starting' | 'pending' | 'completed'
  expiresAtUnixSeconds: number
  authUrl: string | null
  verificationUrl: string | null
  userCode: string | null
}

export async function listExecutionProfiles(
  projectId: string,
): Promise<ExecutionProfile[]> {
  const response = await requestJson<{ profiles: ExecutionProfile[] }>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/execution-profiles`,
  )
  return Array.isArray(response.profiles) ? response.profiles : []
}

export function getAccountStatus(projectId: string, profileId: string) {
  return requestJson<AccountStatus>(accountPath(projectId, profileId))
}

export function startAccountLogin(
  projectId: string,
  profileId: string,
  kind: 'browser' | 'device-code',
  idempotencyKey: string,
) {
  return requestJson<LoginCeremony>(
    `${accountPath(projectId, profileId)}/login/${kind}`,
    { method: 'POST', idempotencyKey, json: {} },
  )
}

export function loginWithApiKey(
  projectId: string,
  profileId: string,
  apiKey: string,
  idempotencyKey: string,
) {
  return requestJson<LoginCeremony>(
    `${accountPath(projectId, profileId)}/login/api-key`,
    { method: 'POST', idempotencyKey, json: { apiKey } },
  )
}

export function cancelAccountLogin(
  projectId: string,
  profileId: string,
  idempotencyKey: string,
) {
  return requestJson<AccountStatus>(
    `${accountPath(projectId, profileId)}/login/cancel`,
    { method: 'POST', idempotencyKey, json: {} },
  )
}

export function logoutAccount(
  projectId: string,
  profileId: string,
  idempotencyKey: string,
) {
  return requestJson<AccountStatus>(
    `${accountPath(projectId, profileId)}/logout`,
    { method: 'POST', idempotencyKey, json: {} },
  )
}

function accountPath(projectId: string, profileId: string) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/execution-profiles/${encodeURIComponent(profileId)}/account`
}
