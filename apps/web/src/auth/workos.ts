import { requestJson } from '../api/client'

export type WorkOsStatus = { enabled: boolean; signupEnabled: boolean; googleEnabled: boolean; linked: boolean; email: string | null }
/** Sign-in's own query keys. */
export const authKeys = {
  workOsStatus: ['workos-status'] as const,
  passwordResetAvailable: ['password-reset-available'] as const,
  passwordReset: (token: string) => ['password-reset', token] as const,
  verifyEmail: (token: string) => ['verify-email', token] as const,
}
export function workOsStatus() { return requestJson<WorkOsStatus>('/api/v1/auth/workos') }
export async function startWorkOs(options: { link?: boolean; username?: string; provider?: 'GoogleOAuth' } = {}) {
  const result = await requestJson<{ url: string }>('/api/v1/auth/workos/start', { method: 'POST', json: options })
  window.location.assign(result.url)
}
