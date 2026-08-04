import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ExecutionAccountSettings } from './ExecutionAccountSettings'

const projectId = '019fb600-0000-7000-8000-000000000101'
const profileId = '019fb600-0000-7000-8000-000000000102'
const actorId = '019fb600-0000-7000-8000-000000000103'

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('ExecutionAccountSettings', () => {
  it('creates one isolated personal profile when none is assigned', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      const method = init?.method ?? 'GET'
      if (url.endsWith('/execution-profiles') && method === 'GET') {
        return jsonResponse({ profiles: [] })
      }
      if (url.endsWith('/execution-profiles') && method === 'POST') {
        expect(new Headers(init?.headers).get('idempotency-key')).toBe(
          'test-idempotency-key',
        )
        expect(init?.body).toBe(JSON.stringify({}))
        return jsonResponse(profile('Personal Codex', actorId))
      }
      if (url.endsWith('/account')) return jsonResponse(accountStatus())
      throw new Error(`unexpected request: ${method} ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('crypto', { randomUUID: () => 'test-idempotency-key' })

    renderSettings()
    expect(
      await screen.findByText(/never shared with another Project member/i),
    ).toBeVisible()
    fireEvent.click(
      screen.getByRole('button', { name: 'Create personal profile' }),
    )

    expect(
      await screen.findByRole('heading', { name: 'Personal Codex' }),
    ).toBeVisible()
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })

  it('shows only the Actor-owned profile and completes a device-code ceremony', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url.endsWith('/execution-profiles')) {
        return jsonResponse({
          profiles: [
            profile('Personal Codex', actorId),
            profile('Another Actor', '019fb600-0000-7000-8000-000000000199'),
          ],
        })
      }
      if (url.endsWith('/account/login/device-code')) {
        expect(init?.method).toBe('POST')
        expect(new Headers(init?.headers).get('idempotency-key')).toBe('test-idempotency-key')
        return jsonResponse({
          ceremonyId: '019fb600-0000-7000-8000-000000000104',
          kind: 'device_code',
          state: 'pending',
          expiresAtUnixSeconds: 2_000_000_000,
          authUrl: null,
          verificationUrl: 'https://example.com/device',
          userCode: 'ABCD-EFGH',
        })
      }
      if (url.endsWith('/account')) return jsonResponse(accountStatus())
      throw new Error(`unexpected request: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('crypto', { randomUUID: () => 'test-idempotency-key' })

    renderSettings()

    expect(await screen.findByRole('heading', { name: 'Personal Codex' })).toBeVisible()
    expect(screen.queryByText('Another Actor')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Use device code' }))

    expect(await screen.findByText('ABCD-EFGH')).toBeVisible()
    expect(screen.getByRole('link', { name: 'Open device sign-in' })).toHaveAttribute(
      'href',
      'https://example.com/device',
    )
    expect(screen.getByRole('button', { name: 'Cancel sign-in' })).toBeEnabled()
  })

  it('clears an API key from the form before refreshing authenticated state', async () => {
    const secret = 'sk-test-browser-secret'
    let authenticated = false
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.endsWith('/execution-profiles')) {
        return jsonResponse({ profiles: [profile('Personal Codex', actorId)] })
      }
      if (url.endsWith('/account/login/api-key')) {
        authenticated = true
        return jsonResponse({
          ceremonyId: '019fb600-0000-7000-8000-000000000105',
          kind: 'api_key',
          state: 'completed',
          expiresAtUnixSeconds: 2_000_000_000,
          authUrl: null,
          verificationUrl: null,
          userCode: null,
        })
      }
      if (url.endsWith('/account')) {
        return jsonResponse(
          authenticated
            ? {
                ...accountStatus(),
                accountState: 'authenticated',
                account: { accountType: 'chatgpt', email: null },
              }
            : accountStatus(),
        )
      }
      throw new Error(`unexpected request: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('crypto', { randomUUID: () => 'test-idempotency-key' })

    renderSettings()
    const input = await screen.findByLabelText('OpenAI API key')
    fireEvent.change(input, { target: { value: secret } })
    fireEvent.click(screen.getByRole('button', { name: 'Use API key' }))

    expect(input).toHaveValue('')
    await waitFor(() => expect(screen.getByText('Signed in with chatgpt.')).toBeVisible())
    expect(screen.queryByText(secret)).not.toBeInTheDocument()
  })
})

function renderSettings() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <ExecutionAccountSettings actorId={actorId} projectId={projectId} />
    </QueryClientProvider>,
  )
}

function profile(name: string, ownerId: string) {
  return {
    id: profileId,
    name,
    status: 'authentication_required',
    accountState: 'authentication_required',
    owner: { id: ownerId, displayName: name },
    provider: { kind: 'codex_app_server', binaryVersion: '0.146.0' },
  }
}

function accountStatus() {
  return {
    profileId,
    accountState: 'authentication_required',
    providerAvailable: true,
    account: null,
    rateLimits: null,
    ceremony: null,
  }
}

function jsonResponse(value: unknown) {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  })
}
