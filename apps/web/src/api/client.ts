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
      return decodeURIComponent(value.join('='))
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

function dispatchAuthenticationRequired() {
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event('agent-workspace:authentication-required'))
  }
}

export async function requestJson<T>(
  path: string,
  options: ApiRequestOptions = {},
): Promise<T> {
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
    const csrfToken = readCsrfToken()
    if (csrfToken) headers.set('x-csrf-token', csrfToken)
  }
  if (idempotencyKey) headers.set('idempotency-key', idempotencyKey)

  const response = await fetch(path, {
    ...requestOptions,
    method,
    body,
    credentials: 'same-origin',
    headers,
  })
  const requestId = response.headers.get('x-request-id')
  const contentType = response.headers.get('content-type') ?? ''
  const payload = contentType.includes('application/json')
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
  return payload as T
}
