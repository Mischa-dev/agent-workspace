import { act, render, screen, waitFor } from '@testing-library/react'
import {
  QueryClient,
  QueryClientProvider,
} from '@tanstack/react-query'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { SessionBoundary } from './SessionBoundary'
import type { AuthenticatedSession, SessionResponse } from './session'

const authenticatedSession: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: 'user-a',
    actorId: 'actor-a',
    actorKind: 'human',
    loginName: 'actor-a',
    displayName: 'Actor A',
    capabilities: { createProject: false },
  },
  organizations: [],
}

const signedOutSession: SessionResponse = {
  authenticated: false,
  user: null,
  organizations: [],
}

describe('normal session expiry', () => {
  // Signed in, the boundary refetches the session and opens the realtime
  // socket. Keep both off the network: a real connection from jsdom fails
  // differently on each Node version (on Node 22 it rejects outside the test).
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => {})))
    vi.stubGlobal(
      'WebSocket',
      class extends EventTarget {
        readyState = 0
        close() {}
        send() {}
      },
    )
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('clears Actor caches when a 200 session response becomes signed out', async () => {
    const queryClient = new QueryClient()
    queryClient.setQueryData(queryKeys.session, authenticatedSession)
    queryClient.setQueryData(queryKeys.projects, {
      projects: [{ id: 'private-project' }],
    })
    queryClient.setQueryData(
      queryKeys.timeline('private-project', 'private-thread'),
      {
        entries: [{ id: 'private-entry' }],
      },
    )
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/home']}>
          <Routes>
            <Route element={<SessionBoundary />}>
              <Route
                element={<h1>Authenticated Home</h1>}
                path="/home"
              />
            </Route>
            <Route element={<h1>Sign in</h1>} path="/sign-in" />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    )

    expect(
      await screen.findByRole('heading', { name: 'Authenticated Home' }),
    ).toBeVisible()
    act(() => {
      queryClient.setQueryData(queryKeys.session, signedOutSession)
    })

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    await waitFor(() => {
      expect(queryClient.getQueryData(queryKeys.projects)).toBeUndefined()
      expect(
        queryClient.getQueryData(
          queryKeys.timeline('private-project', 'private-thread'),
        ),
      ).toBeUndefined()
    })
  })
})
