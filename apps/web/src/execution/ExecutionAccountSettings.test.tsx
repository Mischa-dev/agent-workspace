import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ExecutionAccountSettings } from './ExecutionAccountSettings'
import type { Harness, ProviderAccount } from './api'

const projectId = '019fb600-0000-7000-8000-000000000101'
const accountId = '019fb600-0000-7000-8000-000000000102'
const profileId = '019fb600-0000-7000-8000-000000000103'
const base = `/api/v1/projects/${projectId}`

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('ExecutionAccountSettings', () => {
  it('connects an account for the harness and uses it in the project', async () => {
    let accounts: ProviderAccount[] = []
    const fetchMock = stubFetch((method, path, body) => {
      if (path === `${base}/harnesses`) return { harnesses: [harness()] }
      if (path === `${base}/provider-accounts` && method === 'GET') return { accounts }
      if (path === `${base}/provider-accounts` && method === 'POST') {
        expect(body).toEqual({ adapterKind: 'demo' })
        return account({ preferred: false, profileId: null })
      }
      if (path === `${base}/provider-accounts/${accountId}/use`) {
        accounts = [account()]
        return accounts[0]
      }
      if (path === `${base}/provider-accounts/${accountId}`) return accountStatus()
      return undefined
    })

    renderSettings()
    expect(await screen.findByText(/never shared with anyone else/i)).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Connect Demo harness' }))

    expect(await screen.findByRole('heading', { name: 'My demo account' })).toBeVisible()
    expect(screen.getByText('Used here')).toBeVisible()
    expect(fetchMock.mock.calls.some(([, init]) => init?.method === 'POST')).toBe(true)
  })

  it('offers only the sign-in methods the harness lists and shows a device code', async () => {
    stubFetch((_method, path) => {
      if (path === `${base}/harnesses`) {
        return {
          harnesses: [
            harness({
              loginMethods: [
                { kind: 'device_code', label: 'Use device code' },
                { kind: 'api_key', label: 'Demo key' },
              ],
            }),
          ],
        }
      }
      if (path === `${base}/provider-accounts`) return { accounts: [account()] }
      if (path.endsWith('/login/device-code')) {
        return {
          ceremonyId: '019fb600-0000-7000-8000-000000000104',
          kind: 'device_code',
          state: 'pending',
          expiresAtUnixSeconds: 2_000_000_000,
          authUrl: null,
          verificationUrl: 'https://example.com/device',
          userCode: 'ABCD-EFGH',
        }
      }
      if (path === `${base}/provider-accounts/${accountId}`) return accountStatus()
      return undefined
    })

    renderSettings()
    expect(await screen.findByRole('heading', { name: 'My demo account' })).toBeVisible()
    expect(screen.queryByRole('button', { name: /browser/i })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Use device code' }))

    expect(await screen.findByText('ABCD-EFGH')).toBeVisible()
    expect(screen.getByRole('link', { name: 'Open device sign-in' })).toHaveAttribute('href', 'https://example.com/device')
    expect(screen.getByRole('button', { name: 'Cancel sign-in' })).toBeEnabled()
  })

  it('clears an API key from the form before refreshing the account', async () => {
    const secret = 'sk-test-browser-secret'
    let authenticated = false
    stubFetch((_method, path) => {
      if (path === `${base}/harnesses`) return { harnesses: [harness()] }
      if (path === `${base}/provider-accounts`) return { accounts: [account()] }
      if (path.endsWith('/login/api-key')) {
        authenticated = true
        return {
          ceremonyId: '019fb600-0000-7000-8000-000000000105',
          kind: 'api_key',
          state: 'completed',
          expiresAtUnixSeconds: 2_000_000_000,
          authUrl: null,
          verificationUrl: null,
          userCode: null,
        }
      }
      if (path === `${base}/provider-accounts/${accountId}`) {
        return authenticated
          ? { ...accountStatus(), accountState: 'authenticated', account: { accountType: 'demo', email: null } }
          : accountStatus()
      }
      return undefined
    })

    renderSettings()
    const input = await screen.findByLabelText('Demo key')
    fireEvent.change(input, { target: { value: secret } })
    fireEvent.click(screen.getByRole('button', { name: 'Use API key' }))

    expect(input).toHaveValue('')
    await waitFor(() => expect(screen.getByText('Signed in with demo.')).toBeVisible())
    expect(screen.queryByText(secret)).not.toBeInTheDocument()
    // Signed in, the account's run defaults come from the harness catalog.
    expect(screen.getByLabelText('Model')).toHaveTextContent('Demo large')
  })

  it('lets a workspace admin hide a model from everyone', async () => {
    stubFetch((method, path, body) => {
      if (path === `${base}/harnesses`) return { harnesses: [harness({ canEditCatalog: true })] }
      if (path === `${base}/provider-accounts`) return { accounts: [] }
      if (path === `${base}/harnesses/demo/models/demo-small` && method === 'PUT') {
        expect(body).toEqual({ hidden: true })
        const updated = harness({ canEditCatalog: true })
        updated.catalog.models[0] = { ...updated.catalog.models[0], hidden: true }
        return updated
      }
      return undefined
    })

    renderSettings()
    fireEvent.click(await screen.findByRole('button', { name: 'Hide Demo small' }))
    expect(await screen.findByRole('button', { name: 'Show Demo small' })).toBeVisible()
  })
})

function stubFetch(handle: (method: string, path: string, body: unknown) => unknown) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = init?.method ?? 'GET'
    const path = new URL(String(input), 'http://localhost').pathname
    if (method !== 'GET') {
      expect(new Headers(init?.headers).get('idempotency-key')).toBe('test-idempotency-key')
    }
    const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined
    const value = handle(method, path, body)
    if (value === undefined) throw new Error(`unexpected request: ${method} ${path}`)
    return new Response(JSON.stringify(value), { status: 200, headers: { 'content-type': 'application/json' } })
  })
  vi.stubGlobal('fetch', fetchMock)
  vi.stubGlobal('crypto', { randomUUID: () => 'test-idempotency-key' })
  return fetchMock
}

function renderSettings() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <ExecutionAccountSettings projectId={projectId} />
    </QueryClientProvider>,
  )
}

function harness(change: Partial<Harness> = {}): Harness {
  const speeds = [{ id: 'standard', name: 'Standard' }]
  const model = {
    description: null,
    contextWindow: null,
    reasoningLevels: ['low', 'medium'],
    defaultReasoning: 'low',
    speeds,
    defaultSpeed: 'standard',
    prices: null,
    hidden: false,
    source: 'adapter' as const,
    edited: false,
    overrides: {},
  }
  return {
    kind: 'demo',
    displayName: 'Demo harness',
    description: 'A built-in harness for tests.',
    runsOn: 'local_host',
    features: { concurrentRuns: true, modelListing: true },
    concurrency: { defaultPerAccount: 2, maxPerAccount: 8 },
    loginMethods: [{ kind: 'api_key', label: 'Demo key' }],
    runOptions: { sandboxModes: [], approvalPolicies: [], maxAttempts: 1 },
    catalog: {
      models: [
        { ...model, id: 'demo-small', displayName: 'Demo small', isDefault: false },
        { ...model, id: 'demo-large', displayName: 'Demo large', isDefault: true },
      ],
      refreshedAt: null,
    },
    canEditCatalog: false,
    ...change,
  }
}

function account(change: Partial<ProviderAccount> = {}): ProviderAccount {
  return {
    id: accountId,
    adapterKind: 'demo',
    harnessName: 'Demo harness',
    name: 'My demo account',
    status: 'authentication_required',
    accountState: 'authentication_required',
    maxConcurrentRuns: 2,
    runDefaults: {},
    lastHealthAt: null,
    version: 1,
    profileId,
    preferred: true,
    activeRuns: 0,
    ...change,
  }
}

function accountStatus() {
  return {
    accountId,
    profileId,
    accountState: 'authentication_required',
    providerAvailable: true,
    account: null,
    rateLimits: null,
    ceremony: null,
  }
}
