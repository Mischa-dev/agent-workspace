import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { JoinAgentList } from './JoinAgentList'

const accountId = '019fd100-0000-7000-8000-00000000000a'

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } }),
  )
}

function renderSection() {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <JoinAgentList projectId="019fd100-0000-7000-8000-000000000001" />
    </QueryClientProvider>,
  )
}

describe('JoinAgentList', () => {
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('adds an agent you manage elsewhere and drops it from the list', async () => {
    const added: unknown[] = []
    let joined = false
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        if (path.includes('/ops/account_workspace_add')) {
          added.push(JSON.parse(String(init?.body)))
          joined = true
          return jsonResponse({ outcome: 'ok', value: { accountId, status: 'asked', requestId: 'r1' } })
        }
        return jsonResponse({
          outcome: 'ok',
          value: {
            accounts: joined
              ? []
              : [{ accountId, kind: 'agent', displayName: 'Rook', handle: 'rook', handleTaken: false }],
          },
        })
      }),
    )
    renderSection()
    expect(await screen.findByText('Rook')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    await waitFor(() => expect(added).toEqual([{ account: accountId }]))
    await waitFor(() => expect(screen.queryByText('Rook')).not.toBeInTheDocument())
  })

  it('shows nothing when there is nobody to add', async () => {
    vi.stubGlobal('fetch', vi.fn(() => jsonResponse({ outcome: 'ok', value: { accounts: [] } })))
    const { container } = renderSection()
    await waitFor(() => expect(container).toBeEmptyDOMElement())
  })
})
