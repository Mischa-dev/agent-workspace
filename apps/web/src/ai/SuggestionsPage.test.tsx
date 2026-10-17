import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { AuthenticatedSessionProvider, type AuthenticatedSession } from '../auth/session'
import type { Suggestion } from './api'
import { SuggestionsPage } from './PlanningPages'

vi.mock('../shell/AppShell', () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => children,
}))

vi.mock('../projects/hooks', () => ({
  useProject: () => ({
    isPending: false,
    isError: false,
    data: { id: 'p1', organizationId: 'o1', key: 'ACME', name: 'Acme', archivedAt: null, role: 'member' },
  }),
}))

const session: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: 'u1',
    actorId: 'a1',
    actorKind: 'human',
    loginName: 'owner',
    displayName: 'Owner',
    capabilities: { createProject: true },
  },
  organizations: [],
}

const task = (id: string, key: string, title: string) => ({
  id,
  projectId: 'p1',
  key,
  title,
  statusName: 'Todo',
  statusCategory: 'unstarted',
  finished: false,
  archived: false,
  href: `/projects/p1/work-items/${id}`,
})

const suggestion: Suggestion = {
  id: 's1',
  kind: 'blocks',
  task: task('t2', 'ACME-2', 'Ship the page'),
  other: task('t1', 'ACME-1', 'Build the API'),
  value: {},
  confidence: 0.93,
  reason: 'The page calls the API.',
  sources: [{ kind: 'message', messageId: 'm1', conversationId: 'c1', conversationName: 'launch', href: '/projects/p1/channels/c1' }],
  status: 'pending',
  origin: 'analyze',
  personal: true,
  provider: 'fake',
  model: 'fake-quick',
  createdAt: '2026-10-16T20:00:00Z',
  decidedAt: null,
}

function json(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } }))
}

describe('SuggestionsPage', () => {
  const calls: { method: string; url: string }[] = []

  beforeEach(() => {
    calls.length = 0
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input)
        const method = (init?.method ?? 'GET').toUpperCase()
        calls.push({ method, url })
        if (url.endsWith('/ai')) {
          return json({ configured: true, problem: null, canEdit: true, pendingSuggestions: 1, runs: [], provider: { id: 'fake', displayName: 'Fake' } })
        }
        if (url.includes('/ai/suggestions/s1/accept')) return json({ suggestion: { ...suggestion, status: 'accepted' } })
        if (url.includes('/ai/suggestions')) {
          const decided = calls.some((call) => call.url.includes('/accept'))
          return json({ suggestions: decided ? [] : [suggestion], nextCursor: null, pendingCount: decided ? 0 : 1, canEdit: true })
        }
        return json({ error: { code: 'not_found', message: 'Not found' } }, 404)
      }),
    )
  })

  afterEach(() => vi.unstubAllGlobals())

  it('shows what a suggestion would change, why, and accepts it from the keyboard', async () => {
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AuthenticatedSessionProvider session={session}>
          <MemoryRouter initialEntries={['/projects/p1/suggestions']}>
            <Routes>
              <Route element={<SuggestionsPage />} path="/projects/:projectId/suggestions" />
            </Routes>
          </MemoryRouter>
        </AuthenticatedSessionProvider>
      </QueryClientProvider>,
    )

    const card = (await screen.findByText('The page calls the API.')).closest('li') as HTMLElement
    expect(card.textContent).toContain('ACME-1')
    expect(card.textContent).toContain('blocks')
    expect(card.textContent).toContain('93% sure')
    expect(screen.getByText('Only you')).toBeTruthy()
    expect(screen.getByRole('link', { name: /From launch/ }).getAttribute('href')).toBe('/projects/p1/channels/c1')

    card.focus()
    fireEvent.keyDown(card, { key: 'a' })
    await waitFor(() => expect(calls.some((call) => call.method === 'POST' && call.url.endsWith('/ai/suggestions/s1/accept'))).toBe(true))
    expect(await screen.findByText('Nothing to review')).toBeTruthy()
  })
})
