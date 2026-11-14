// Where the API and the live connection answer (H1.41). The build has no
// origin in it, so the same `dist/` works served by the server itself or from
// a static host.
//
// The app first asks its own origin for `/client-config`. Served by the
// server, that is the server's answer, with no API origin: the app stays on
// the origin it was loaded from. On a static host it is a small file the
// operator put next to `index.html`, `{"apiOrigin": "https://api.example.com"}`,
// and the app then asks that origin for the rest. With no answer (offline,
// or an older server) the app keeps to its own origin, as it always has.

import { origins } from '../api/client'

type Answer = { apiOrigin?: unknown; liveOrigin?: unknown } | null

// A static host may serve the file with any type, and a missing one as the
// app's index.html: whatever doesn't parse is no answer.
const read = (url: string): Promise<Answer> =>
  fetch(url, { credentials: 'omit', signal: AbortSignal.timeout(4000) })
    .then((response) => (response.ok ? response.text() : 'null'))
    .then((text) => JSON.parse(text) as Answer)
    .catch(() => null)

function origin(value: unknown): string | null {
  try {
    const url = new URL(value as string)
    return url.protocol.startsWith('http') ? url.origin : null
  } catch {
    return null
  }
}

/** Reads the configuration and points the API client at it. Never throws. */
export async function load(): Promise<{ apiOrigin: string | null; liveOrigin: string | null }> {
  const local = await read('/client-config')
  let api = origin(local?.apiOrigin)
  if (api === window.location.origin) api = null
  const answer = (api && (await read(`${api}/client-config`))) || local
  const live = origin(answer?.liveOrigin) ?? origin(local?.liveOrigin)
  origins.api = api ?? undefined
  origins.live = live ?? api ?? undefined
  origins.csrf = api ? csrfFrom(api) : undefined
  return { apiOrigin: api, liveOrigin: live }
}

/**
 * The API's request token for a page on another origin, which can't read
 * the API's cookie: read from `/api/v1/auth/csrf` before each write, so it
 * is the current one after signing in or out.
 */
const csrfFrom = (api: string) => (): Promise<string | null> =>
  fetch(`${api}/api/v1/auth/csrf`, { credentials: 'include', headers: { accept: 'application/json' } })
    .then((response) => (response.ok ? response.json() : null))
    .then((body: { csrfToken?: string | null } | null) => body?.csrfToken ?? null)
    .catch(() => null)
