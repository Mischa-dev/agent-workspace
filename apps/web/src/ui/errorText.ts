import { ApiError, isApiError } from '../api/errors'
import type { OpErrorOutcome } from '../api/generated/operations'

/** A plain sentence for a failed change, and whether fresh data would fix it. */
export function describeError(error: unknown): { message: string; stale: boolean } {
  const refused = opErrorSentence(error)
  if (refused) return refused
  if (isApiError(error)) {
    // Only a version clash means the screen is out of date; other 409s
    // (for example "interrupt active runs first") explain themselves.
    if (error.code.includes('conflict')) {
      return { message: 'Someone changed this first. You’re now looking at the latest version.', stale: true }
    }
    if (error.status === 409) return { message: error.message, stale: false }
    if (error.status === 403) return { message: 'You don’t have permission to do that here.', stale: false }
    if (error.status === 404) return { message: 'It’s no longer there. It may have been removed.', stale: true }
    if (error.status === 429) return { message: 'That’s a lot at once. Wait a moment and try again.', stale: false }
    if (error.status >= 500) return { message: 'The server had a problem. Try again in a moment.', stale: false }
    return { message: error.message, stale: false }
  }
  if (isOffline(error)) return { message: 'You seem to be offline. Check your connection and try again.', stale: false }
  return { message: error instanceof Error && error.message ? error.message : 'Something went wrong. Try again.', stale: false }
}

/**
 * A plain sentence for something that didn't load. Only blames the connection
 * when it's the connection, so a missing page or a server fault says so.
 */
export function describeLoadError(error: unknown): string {
  if (isApiError(error)) {
    if (error.status === 401) return 'You’ve been signed out. Sign in again to see this.'
    if (error.status === 403) return 'You don’t have access to this.'
    if (error.status === 404) return 'It’s not there anymore. It may have been moved or removed.'
    if (error.status === 429) return 'Too many requests at once. Wait a moment and try again.'
    if (error.status >= 500) return 'The server had a problem. Try again in a moment.'
    return error.message || 'Something went wrong. Try again.'
  }
  if (isOffline(error)) return 'You seem to be offline. Check your connection and try again.'
  return 'Something went wrong. Try again.'
}

function isOffline(error: unknown) {
  return error instanceof TypeError || (typeof navigator !== 'undefined' && navigator.onLine === false)
}

/**
 * Each outcome as a sentence. Typed against the generated outcome list, so
 * an outcome the server adds fails the typecheck here until it has one.
 */
export const OUTCOME_SENTENCES: Record<OpErrorOutcome, string> = {
  unauthenticated: 'You’ve been signed out. Sign in again and try once more.',
  not_found: 'It’s no longer there. It may have been removed.',
  invalid_request: 'Something in that isn’t right. Check it and try again.',
  permission_denied: 'You don’t have permission to do that here.',
  conflict: 'Someone changed this first. You’re now looking at the latest version.',
  rate_limited: 'That’s a lot at once. Wait a moment and try again.',
  limit_reached: 'This workspace has reached a limit. Ask an owner about raising it.',
  workspace_suspended: 'This workspace is paused, so changes are off for now.',
  too_large: 'That’s too large to save. Make it smaller and try again.',
  temporarily_unavailable: 'The server had a problem. Try again in a moment.',
}

/** An operation's refusal carries its envelope's `reason` (api/client.ts). */
function fromOperation(error: ApiError) {
  return typeof error.details === 'object' && error.details !== null && 'reason' in error.details
}

function isOutcome(code: string): code is OpErrorOutcome {
  return Object.prototype.hasOwnProperty.call(OUTCOME_SENTENCES, code)
}

/**
 * A refused operation as a sentence and whether fresh data would fix it, or
 * null when the error isn't an operation's. The server's own message wins
 * where it says more than the outcome does (a field to fix, a permission
 * to ask for).
 */
export function opErrorSentence(error: unknown): { message: string; stale: boolean } | null {
  if (!(error instanceof ApiError) || !isOutcome(error.code) || !fromOperation(error)) return null
  const stale = error.code === 'conflict' || error.code === 'not_found'
  const explains = error.code === 'invalid_request' || error.code === 'permission_denied' || error.code === 'limit_reached'
  return { message: explains && error.message ? error.message : OUTCOME_SENTENCES[error.code], stale }
}
