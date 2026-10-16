import { requestJson } from '../../api/client'

export type SignInMethod = 'password' | 'workos' | 'invitation'

export type AccountSession = {
  id: string
  createdAt: string
  lastSeenAt: string
  expiresAt: string
  userAgent: string | null
  current: boolean
  signInMethod: SignInMethod
}

export type SecurityEvent = {
  id: string
  action: string
  occurredAt: string
  outcome: string
  channel: string | null
  clientName: string | null
}

export type AccountSecurity = {
  hasPassword: boolean
  twoFactor: {
    available: boolean
    enabled: boolean
    pending: boolean
    recoveryCodesLeft: number
    enabledAt: string | null
  }
  recentEvents: SecurityEvent[]
}

export type TwoFactorSetup = { secret: string; otpauthUri: string; issuer: string; accountName: string }

export type TokenScope = 'read' | 'write'

export type ApiToken = {
  id: string
  projectId: string
  projectName: string
  projectKey: string
  name: string
  tokenPrefix: string
  scopes: TokenScope[]
  status: 'active' | 'revoked' | 'expired'
  createdAt: string
  lastUsedAt: string | null
  expiresAt: string
  revokedAt: string | null
  version: number
}

export type ApiTokenProject = { id: string; name: string; key: string; canWrite: boolean }

export const accountKeys = {
  sessions: ['account', 'sessions'] as const,
  security: ['account', 'security'] as const,
  apiTokens: ['account', 'api-tokens'] as const,
}

const post = <T>(path: string, json: unknown = {}) => requestJson<T>(path, { method: 'POST', json })

export const changePassword = (currentPassword: string, newPassword: string) =>
  post<void>('/api/v1/account/password', { currentPassword, newPassword })

export const listSessions = () => requestJson<{ sessions: AccountSession[] }>('/api/v1/account/sessions')

export const revokeSession = (sessionId: string) =>
  post<void>(`/api/v1/account/sessions/${encodeURIComponent(sessionId)}/revoke`)

export const revokeOtherSessions = () => post<{ revoked: number }>('/api/v1/account/sessions/revoke-others')

export const getSecurity = () => requestJson<AccountSecurity>('/api/v1/account/security')

export const startTwoFactorSetup = (currentPassword: string) =>
  post<TwoFactorSetup>('/api/v1/account/two-factor/setup', { currentPassword })

export const confirmTwoFactor = (code: string) =>
  post<{ recoveryCodes: string[] }>('/api/v1/account/two-factor/confirm', { code })

export const regenerateRecoveryCodes = (code: string) =>
  post<{ recoveryCodes: string[] }>('/api/v1/account/two-factor/recovery-codes', { code })

export const disableTwoFactor = (currentPassword: string, code: string) =>
  post<void>('/api/v1/account/two-factor/disable', { currentPassword, code })

export const listApiTokens = () =>
  requestJson<{ tokens: ApiToken[]; projects: ApiTokenProject[] }>('/api/v1/account/api-tokens')

export const createApiToken = (input: { projectId: string; name: string; scopes: TokenScope[]; expiresInDays: number }) =>
  post<{ token: string; credential: ApiToken }>('/api/v1/account/api-tokens', input)

export const revokeApiToken = (token: ApiToken) =>
  post<ApiToken>(`/api/v1/account/api-tokens/${encodeURIComponent(token.id)}/revoke`, { expectedVersion: token.version })

export const setDefaultWorkspace = (organizationId: string) =>
  requestJson<void>('/api/v1/account/default-workspace', { method: 'PUT', json: { organizationId } })

/** "Firefox on macOS" from a user agent string; null when there's nothing to go on. */
export function describeDevice(userAgent: string | null): string | null {
  if (!userAgent) return null
  const ua = userAgent
  const browser = /Edg\//.test(ua)
    ? 'Edge'
    : /OPR\/|Opera/.test(ua)
      ? 'Opera'
      : /Firefox\//.test(ua)
        ? 'Firefox'
        : /Chrome\/|CriOS\//.test(ua)
          ? 'Chrome'
          : /Safari\//.test(ua)
            ? 'Safari'
            : /curl\//i.test(ua)
              ? 'curl'
              : null
  const system = /iPhone/.test(ua)
    ? 'iPhone'
    : /iPad/.test(ua)
      ? 'iPad'
      : /Android/.test(ua)
        ? 'Android'
        : /Mac OS X|Macintosh/.test(ua)
          ? 'macOS'
          : /Windows/.test(ua)
            ? 'Windows'
            : /CrOS/.test(ua)
              ? 'ChromeOS'
              : /Linux/.test(ua)
                ? 'Linux'
                : null
  if (browser && system) return `${browser} on ${system}`
  if (browser || system) return browser ?? system
  return ua.length > 48 ? `${ua.slice(0, 47)}…` : ua
}
