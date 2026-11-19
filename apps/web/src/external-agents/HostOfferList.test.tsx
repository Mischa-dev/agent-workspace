import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { HostOfferList } from './HostOfferList'

const hostId = '019fd100-0000-7000-8000-00000000000b'

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } }),
  )
}

function renderSection() {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <HostOfferList />
    </QueryClientProvider>,
  )
}

describe('HostOfferList', () => {
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('starts a host’s offer and links to its page', async () => {
    const started: unknown[] = []
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        if (String(input).includes('/ops/agent_offer_start')) {
          started.push(JSON.parse(String(init?.body)))
          return jsonResponse({
            outcome: 'ok',
            value: { commandId: 'c1', page: 'https://host.example/new?command=c1' },
          })
        }
        return jsonResponse({
          outcome: 'ok',
          value: { offers: [{ hostId, hostName: 'Team host', offer: { id: 'new', title: 'New agent' } }] },
        })
      }),
    )
    renderSection()
    expect(await screen.findByText('New agent')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    await waitFor(() => expect(started).toEqual([{ host: hostId, offerId: 'new' }]))
    expect(await screen.findByRole('link', { name: 'Team host’s page' })).toHaveAttribute(
      'href',
      'https://host.example/new?command=c1',
    )
  })

  it('adds the built-in assistant at once, or says what to set first', async () => {
    const started: unknown[] = []
    let ready = false
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        if (String(input).includes('/ops/agent_offer_start')) {
          started.push(JSON.parse(String(init?.body)))
          return jsonResponse({ outcome: 'ok', value: { accountId: 'a1' } })
        }
        const offer = {
          id: 'assistant',
          title: 'Assistant',
          start: { adds: true },
          ...(ready ? {} : { unavailable: 'Choose a provider first.' }),
        }
        return jsonResponse({ outcome: 'ok', value: { offers: [{ hostId, hostName: 'This server', offer }] } })
      }),
    )
    const first = renderSection()
    expect(await screen.findByText(/Choose a provider first/)).toBeVisible()
    expect(screen.getByRole('button', { name: 'Add' })).toBeDisabled()
    first.unmount()
    ready = true
    renderSection()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Add' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    await waitFor(() => expect(started).toEqual([{ host: hostId, offerId: 'assistant' }]))
  })

  it('shows nothing when no host offers anything', async () => {
    vi.stubGlobal('fetch', vi.fn(() => jsonResponse({ outcome: 'ok', value: { offers: [] } })))
    const { container } = renderSection()
    await waitFor(() => expect(container).toBeEmptyDOMElement())
  })
})
