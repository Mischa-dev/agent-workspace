import type { IconName } from '../ui'

/**
 * The places you opened last (channels, threads, DMs, tasks, pages), kept in
 * this browser per person so the command menu can offer them with nothing
 * typed. Losing them costs nothing, so storage failures are ignored.
 */

export type RecentPlace = {
  path: string
  title: string
  at: number
}

const LIMIT = 12
const keyFor = (actorId: string) => `aw.recent-places.${actorId}`

/** Pages that are already one keystroke away, or that aren't a place. */
const SKIPPED = [/^\/home\/?$/, /^\/inbox\/?$/, /^\/later\/?$/, /^\/my-work\/?$/, /^\/search\/?$/, /^\/settings(\/|$)/, /^\/messages\/?$/, /^\/projects\/?$/, /^\/projects\/new\/?$/, /^\/dev\//, /^\/welcome\/?$/, /^\/new-task\/?$/, /^\/share\/?$/]

/** Addresses whose title comes from the page once it has loaded. */
const NAMED_BY_PAGE = [
  /\/channels\/[^/]+\/?$/,
  /\/threads\/[^/]+\/?$/,
  /^\/messages\/(?!agents)[^/]+\/?$/,
  /\/work-items\/[^/]+\/?$/,
  /\/pages\/[^/]+\/?$/,
  /^\/runs\/[^/]+\/?$/,
  /^\/whiteboards\/[^/]+\/?$/,
]

/** Whether a page is worth remembering, given its title and whether the page named itself. */
export function isRememberedPlace(path: string, title: string, namedByPage: boolean) {
  if (!title.trim()) return false
  if (SKIPPED.some((pattern) => pattern.test(path))) return false
  // Before the page loads, its title is only "Channel" or "Task": wait for the real one.
  if (!namedByPage && NAMED_BY_PAGE.some((pattern) => pattern.test(path))) return false
  return true
}

/** Puts `place` first, without duplicates, keeping the newest `LIMIT`. */
export function withPlace(places: RecentPlace[], place: RecentPlace): RecentPlace[] {
  return [place, ...places.filter((existing) => existing.path !== place.path)].slice(0, LIMIT)
}

export function readRecentPlaces(actorId: string): RecentPlace[] {
  try {
    const raw = window.localStorage.getItem(keyFor(actorId))
    const parsed: unknown = raw ? JSON.parse(raw) : []
    if (!Array.isArray(parsed)) return []
    return parsed.filter(
      (entry): entry is RecentPlace =>
        Boolean(entry) &&
        typeof entry === 'object' &&
        typeof (entry as RecentPlace).path === 'string' &&
        (entry as RecentPlace).path.startsWith('/') &&
        typeof (entry as RecentPlace).title === 'string' &&
        typeof (entry as RecentPlace).at === 'number',
    )
  } catch {
    return []
  }
}

export function rememberPlace(actorId: string, place: RecentPlace) {
  try {
    window.localStorage.setItem(keyFor(actorId), JSON.stringify(withPlace(readRecentPlaces(actorId), place)))
  } catch {
    // Private windows and full storage just don't remember.
  }
}

export function forgetRecentPlaces(actorId: string) {
  try {
    window.localStorage.removeItem(keyFor(actorId))
  } catch {
    // Nothing to forget.
  }
}

/** An icon for the kind of place an address is. */
export function placeIcon(path: string): IconName {
  if (/\/channels\//.test(path)) return 'hash'
  if (/\/threads\//.test(path)) return 'messages'
  if (/^\/messages\//.test(path)) return 'message'
  if (/\/work-items\/[^/]+/.test(path)) return 'checklist'
  if (/\/pages\//.test(path)) return 'list'
  if (/\/collections\//.test(path)) return 'table'
  if (/^\/whiteboards\//.test(path)) return 'whiteboard'
  if (/^\/runs\//.test(path)) return 'activity'
  return 'clock'
}
