// Drafts and unsent messages kept in this browser, so a reload, a closed tab
// or a dropped connection on a phone never loses what someone typed. They stay
// on this device only, expire after a while, and are cleared on sign-out.

import { isApiError } from '../api/errors'

const PREFIX = 'aw-local:'
const MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000

type Stored<T> = { savedAt: number; value: T }

function storage(): Storage | null {
  try {
    return typeof window === 'undefined' ? null : window.localStorage
  } catch {
    return null
  }
}

export function readLocal<T>(key: string): T | null {
  const store = storage()
  if (!store) return null
  try {
    const raw = store.getItem(PREFIX + key)
    if (!raw) return null
    const parsed = JSON.parse(raw) as Stored<T>
    if (!parsed || typeof parsed.savedAt !== 'number' || Date.now() - parsed.savedAt > MAX_AGE_MS) {
      store.removeItem(PREFIX + key)
      return null
    }
    return parsed.value
  } catch {
    return null
  }
}

/** Save `value` under `key`, or remove it when `value` is null. */
export function writeLocal<T>(key: string, value: T | null) {
  const store = storage()
  if (!store) return
  try {
    if (value === null) store.removeItem(PREFIX + key)
    else store.setItem(PREFIX + key, JSON.stringify({ savedAt: Date.now(), value } satisfies Stored<T>))
  } catch {
    // Full or blocked storage (private browsing): the draft lives only in the page.
  }
}

/** Forget every draft and unsent message on this device, for signing out. */
export function clearLocalDrafts() {
  const store = storage()
  if (!store) return
  try {
    const keys: string[] = []
    for (let index = 0; index < store.length; index += 1) {
      const key = store.key(index)
      if (key?.startsWith(PREFIX)) keys.push(key)
    }
    for (const key of keys) store.removeItem(key)
  } catch {
    // Nothing to clear when storage is unavailable.
  }
}

/**
 * True when a request failed because the server couldn't be reached (offline,
 * a dropped connection, the server restarting), so sending it again later can
 * work. A refusal from the server itself is not one.
 */
export function isConnectionError(error: unknown) {
  // A gateway's error page (no API error body) while the server is down or restarting.
  if (isApiError(error)) return [502, 503, 504].includes(error.status) && error.code === 'request_failed'
  if (typeof navigator !== 'undefined' && navigator.onLine === false) return true
  return error instanceof TypeError
}

/** Calls `retry` when the browser comes back online, and every `everyMs` while there is something to retry. */
export function onReconnect(retry: () => void, everyMs = 20_000) {
  const online = () => retry()
  const visible = () => {
    if (document.visibilityState === 'visible') retry()
  }
  window.addEventListener('online', online)
  document.addEventListener('visibilitychange', visible)
  const timer = window.setInterval(() => {
    if (navigator.onLine !== false) retry()
  }, everyMs)
  return () => {
    window.removeEventListener('online', online)
    document.removeEventListener('visibilitychange', visible)
    window.clearInterval(timer)
  }
}
