import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { CatalogModel, Harness } from '../../execution/api'
import { WorkspaceModelLists } from './ModelLists'

const organizationId = '019fb600-0000-7000-8000-000000000201'
const base = `/api/v1/organizations/${organizationId}/model-catalog`

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('WorkspaceModelLists', () => {
  it('lets a workspace admin hide a model from everyone, keeping its other edits', async () => {
    const edited = () => harness({ ...model('demo-small', 'Demo small'), edited: true, overrides: { contextWindow: 64_000 } })
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const method = init?.method ?? 'GET'
      const path = new URL(String(input), 'http://localhost').pathname
      if (path === base && method === 'GET') return json({ harnesses: [edited()] })
      if (path === `${base}/demo/models/demo-small` && method === 'PUT') {
        expect(new Headers(init?.headers).get('idempotency-key')).toBe('test-idempotency-key')
        expect(JSON.parse(String(init?.body))).toEqual({ contextWindow: 64_000, hidden: true })
        const updated = edited()
        updated.catalog.models[0] = { ...updated.catalog.models[0], hidden: true }
        return json(updated)
      }
      throw new Error(`unexpected request: ${method} ${path}`)
    })
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('crypto', { randomUUID: () => 'test-idempotency-key' })

    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
        <MemoryRouter>
          <WorkspaceModelLists organizationId={organizationId} />
        </MemoryRouter>
      </QueryClientProvider>,
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Hide Demo small' }))

    expect(await screen.findByRole('button', { name: 'Show Demo small' })).toBeVisible()
    // The default model can't be hidden, only replaced as the default.
    expect(screen.queryByRole('button', { name: 'Hide Demo large' })).not.toBeInTheDocument()
  })
})

function json(value: unknown) {
  return new Response(JSON.stringify(value), { status: 200, headers: { 'content-type': 'application/json' } })
}

function model(id: string, displayName: string, isDefault = false): CatalogModel {
  return {
    id,
    displayName,
    isDefault,
    description: null,
    contextWindow: null,
    reasoningLevels: ['low', 'medium'],
    defaultReasoning: 'low',
    speeds: [{ id: 'standard', name: 'Standard' }],
    defaultSpeed: 'standard',
    prices: null,
    hidden: false,
    source: 'adapter',
    edited: false,
    overrides: {},
  }
}

function harness(first: CatalogModel): Harness {
  return {
    kind: 'demo',
    displayName: 'Demo harness',
    description: 'A built-in harness for tests.',
    runsOn: 'local_host',
    features: { concurrentRuns: true, modelListing: true },
    concurrency: { defaultPerAccount: 2, maxPerAccount: 8 },
    loginMethods: [{ kind: 'api_key', label: 'Demo key' }],
    runOptions: { sandboxModes: [], approvalPolicies: [], maxAttempts: 1 },
    catalog: { models: [first, model('demo-large', 'Demo large', true)], refreshedAt: null },
    canEditCatalog: true,
  }
}
