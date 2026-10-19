import { isApiError } from '../api/errors'

/** A plain sentence for a failed change, and whether fresh data would fix it. */
export function describeError(error: unknown): { message: string; stale: boolean } {
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
