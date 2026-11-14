import { ApiError, type ApiErrorBody } from './errors'

const unsafeMethods = new Set(['POST', 'PUT', 'PATCH', 'DELETE'])
const csrfCookieNames = ['__Host-agent_workspace_csrf', 'agent_workspace_csrf']

export type ApiRequestOptions = Omit<RequestInit, 'body'> & {
  body?: BodyInit | null
  json?: unknown
  idempotencyKey?: string
}

function readCookie(name: string): string | null {
  if (typeof document === 'undefined') return null
  for (const part of document.cookie.split(';')) {
    const [candidate, ...value] = part.trim().split('=')
    if (candidate === name) {
      try {
        return decodeURIComponent(value.join('='))
      } catch {
        // Cookies are ambient, user-controlled input. A malformed percent
        // escape must not prevent every state-changing API request from being
        // sent; treat that cookie as absent and let the server reject a
        // request that has no valid CSRF token.
        return null
      }
    }
  }
  return null
}

export function readCsrfToken(): string | null {
  for (const name of csrfCookieNames) {
    const value = readCookie(name)
    if (value) return value
  }
  return null
}

// Where the API and the live connection answer (H1.41). Null: the origin the
// app was loaded from. Set once at start from `/client-config` when the app
// is served from a static host on another origin of the same site.
let apiOrigin: string | null = null
let liveOrigin: string | null = null
// The API's request token, which a page on another origin can't read from
// the API's cookie: fetched from `/api/v1/auth/csrf` when first needed.
let crossOriginCsrf: Promise<string | null> | null = null

export function setApiOrigins(origins: { api: string | null; live: string | null }) {
  apiOrigin = origins.api
  liveOrigin = origins.live
  crossOriginCsrf = null
}

/** An API path as the browser should ask for it. */
export function apiUrl(path: string): string {
  return apiOrigin && path.startsWith('/') ? `${apiOrigin}${path}` : path
}

/** The live connection's URL for `path`: wss on an https origin. */
export function liveUrl(path: string): string {
  const origin = liveOrigin ?? apiOrigin ?? window.location.origin
  return `${origin.replace(/^http/, 'ws')}${path}`
}

/** The origin agents and apps reach this server at, for showing to people. */
export function apiOriginForDisplay(): string {
  return apiOrigin ?? window.location.origin
}

function crossOriginCsrfToken(): Promise<string | null> {
  crossOriginCsrf ??= fetch(apiUrl('/api/v1/auth/csrf'), {
    credentials: 'include',
    headers: { accept: 'application/json' },
  })
    .then((response) => (response.ok ? response.json() : null))
    .then((body: { csrfToken?: string | null } | null) => body?.csrfToken ?? null)
    .catch(() => null)
  return crossOriginCsrf
}

function dispatchAuthenticationRequired() {
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event('agent-workspace:authentication-required'))
  }
}

export async function requestJson<T>(
  path: string,
  options: ApiRequestOptions = {},
): Promise<T> {
  try {
    return await sendJson<T>(path, options)
  } catch (error) {
    // Signing in or out gives the API a new request token; a page on
    // another origin fetches it again and tries once more.
    if (apiOrigin && error instanceof ApiError && error.code === 'csrf_token_invalid') {
      crossOriginCsrf = null
      return sendJson<T>(path, options)
    }
    throw error
  }
}

async function sendJson<T>(path: string, options: ApiRequestOptions): Promise<T> {
  const { idempotencyKey, json, ...requestOptions } = options
  const method = (requestOptions.method ?? 'GET').toUpperCase()
  const headers = new Headers(requestOptions.headers)
  headers.set('accept', 'application/json')

  let body = requestOptions.body
  if (json !== undefined) {
    headers.set('content-type', 'application/json')
    body = JSON.stringify(json)
  }

  if (unsafeMethods.has(method)) {
    const csrfToken = apiOrigin ? await crossOriginCsrfToken() : readCsrfToken()
    if (csrfToken) headers.set('x-csrf-token', csrfToken)
  }
  if (idempotencyKey) headers.set('idempotency-key', idempotencyKey)

  const response = await fetch(apiUrl(path), {
    ...requestOptions,
    method,
    body,
    credentials: apiOrigin ? 'include' : 'same-origin',
    headers,
  })
  const requestId = response.headers.get('x-request-id')
  const contentType = response.headers.get('content-type') ?? ''
  const isJson = contentType.includes('application/json')
  const payload = isJson
    ? ((await response.json().catch(() => null)) as
        | { error?: ApiErrorBody }
        | T
        | null)
    : null

  if (!response.ok) {
    const bodyError =
      payload && typeof payload === 'object' && 'error' in payload
        ? payload.error
        : undefined
    const code = bodyError?.code ?? 'request_failed'
    if (response.status === 401 && code === 'authentication_required') {
      dispatchAuthenticationRequired()
    }
    throw new ApiError({
      status: response.status,
      code,
      message: bodyError?.message ?? `Request failed (${response.status}).`,
      details: bodyError?.details,
      requestId,
    })
  }

  if (response.status === 204) return undefined as T
  if (!isJson || payload === null) {
    throw new ApiError({
      status: response.status,
      code: 'invalid_response',
      message: 'The server returned an invalid response.',
      requestId,
    })
  }
  return payload as T
}
