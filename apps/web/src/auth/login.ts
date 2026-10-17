// Password sign-in, kept apart from session.ts so it loads with the sign-in
// page rather than with every page.
import { requestJson } from '../api/client'
import { normalizeSessionResponse, type SessionResponse } from './session'

/**
 * A password sign-in that still needs a two-factor code. It is signed out in
 * every other respect: no user, no cookies, nothing cached.
 */
export type TwoFactorChallenge = SessionResponse & {
  authenticated: false
  user: null
  twoFactorRequired: true
  challenge: string
}

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' ? (value as WireRecord) : {}
}

export function isTwoFactorChallenge(value: unknown): value is TwoFactorChallenge {
  return (
    asRecord(value).twoFactorRequired === true &&
    typeof asRecord(value).challenge === 'string'
  )
}

/** A login response: a session, or a challenge for the second step. Never both. */
export function normalizeLoginResponse(value: unknown): SessionResponse | TwoFactorChallenge {
  const record = asRecord(value)
  if (record.twoFactorRequired === true) {
    const challenge = typeof record.challenge === 'string' ? record.challenge.trim() : ''
    // The challenge is an opaque server token: printable ASCII, bounded.
    if (!challenge || challenge.length > 4096 || !/^[\x21-\x7e]+$/.test(challenge)) {
      return { authenticated: false, user: null, organizations: [] }
    }
    return { authenticated: false, user: null, organizations: [], twoFactorRequired: true, challenge }
  }
  return normalizeSessionResponse(value)
}

export async function login(loginName: string, password: string) {
  return normalizeLoginResponse(
    await requestJson<unknown>('/api/v1/auth/login', {
      method: 'POST',
      json: { loginName, password },
    }),
  )
}

/** The second step: a 6-digit code from the authenticator app, or a recovery code. */
export async function loginTwoFactor(challenge: string, code: string) {
  return normalizeSessionResponse(
    await requestJson<unknown>('/api/v1/auth/login/two-factor', {
      method: 'POST',
      json: { challenge, code },
    }),
  )
}
