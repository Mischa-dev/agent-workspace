import { requestJson } from '../api/client'

export type WorkOsStatus = { enabled: boolean; signupEnabled: boolean; googleEnabled: boolean; linked: boolean; email: string | null }
export function workOsStatus() { return requestJson<WorkOsStatus>('/api/v1/auth/workos') }
export async function startWorkOs(options: { link?: boolean; username?: string; provider?: 'GoogleOAuth' } = {}) {
  const result = await requestJson<{ url: string }>('/api/v1/auth/workos/start', { method: 'POST', json: options })
  window.location.assign(result.url)
}
