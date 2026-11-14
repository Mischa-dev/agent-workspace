// What the app learns at start (H1.41): where the API and the live
// connection answer. The build has no origin in it, so the same `dist/` works
// served by the server itself or from a static host.
//
// The app first asks its own origin for `/client-config`. Served by the
// server, that is the server's answer, with no API origin: the app stays on
// the origin it was loaded from. On a static host it is a small file the
// operator put next to `index.html`, `{"apiOrigin": "https://api.example.com"}`,
// and the app then asks that origin for the rest.

import { setApiOrigins } from '../api/client'

export type ClientConfig = {
  apiOrigin: string | null
  liveOrigin: string | null
  signIn?: { methods?: string[] }
  signup?: string
  features?: Record<string, boolean>
  version?: string
}

const TIMEOUT_MS = 4000

function origin(value: unknown): string | null {
  if (typeof value !== 'string' || !value) return null
  try {
    const url = new URL(value)
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.origin : null
  } catch {
    return null
  }
}

async function read(url: string, init: RequestInit): Promise<Record<string, unknown> | null> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS)
  try {
    const response = await fetch(url, { ...init, headers: { accept: 'application/json' }, signal: controller.signal })
    if (!response.ok) return null
    // A static host may serve the file with any type, and a missing one as
    // the app's index.html: whatever doesn't parse is no answer.
    const body: unknown = JSON.parse(await response.text())
    return body && typeof body === 'object' ? (body as Record<string, unknown>) : null
  } catch {
    return null
  } finally {
    clearTimeout(timer)
  }
}

let loaded: ClientConfig = { apiOrigin: null, liveOrigin: null }

/** The configuration read at start. */
export function clientConfig(): ClientConfig {
  return loaded
}

/**
 * Reads the configuration and points the API client at the right origins.
 * Never throws: with no answer (offline, or an older server) the app keeps
 * to its own origin, as it always has.
 */
export async function loadClientConfig(): Promise<ClientConfig> {
  const local = await read('/client-config', { credentials: 'same-origin' })
  const pointer = origin(local?.apiOrigin)
  const elsewhere = pointer && pointer !== window.location.origin ? pointer : null
  const remote = elsewhere ? await read(`${elsewhere}/client-config`, { credentials: 'omit', mode: 'cors' }) : null
  const answer = remote ?? local ?? {}
  loaded = {
    ...(answer as Partial<ClientConfig>),
    apiOrigin: elsewhere,
    liveOrigin: origin(answer.liveOrigin) ?? origin(local?.liveOrigin),
  }
  setApiOrigins({ api: loaded.apiOrigin, live: loaded.liveOrigin })
  return loaded
}
