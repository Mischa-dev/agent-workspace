import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { JoinPage } from './JoinPage'

const invitationToken = `aw_inv_${'a'.repeat(43)}`
const signedOutSession = {
  authenticated: false,
  user: null,
  organizations: [],
}
const authenticatedSession = {
  authenticated: true,
  user: {
    id: '019fb700-0000-7000-8000-000000000001',
    actorId: '019fb700-0000-7000-8000-000000000002',
    actorKind: 'human',
    loginName: 'new.human',
    displayName: 'New Human',
    capabilities: { createProject: false },
  },
  organizations: [
    {
      id: '019fb700-0000-7000-8000-000000000003',
      name: 'Mischa Personal',
      slug: 'mischa-personal',
      role: 'member',
    },
  ],
}
const resolvedInvitation = {
  organizationName: 'Mischa Personal',
  projectName: '<img src=x onerror=alert(1)>',
  role: 'member',
  expiresAt: '2026-08-02T12:00:00Z',
}

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { 'content-type': 'application/json' },
    }),
  )
}

function renderJoin() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route element={<JoinPage />} path="/join" />
          <Route element={<h1>Workspace home</h1>} path="/home" />
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>,
  )
}

describe('invitation acceptance', () => {
  beforeEach(() => {
    window.history.replaceState({}, '', `/join#token=${invitationToken}`)
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    document.cookie = 'agent_workspace_csrf=; Max-Age=0; Path=/'
  })

  it('removes the token fragment before resolving and creates a new identity', async () => {
    const requests: Array<{ path: string; init?: RequestInit }> = []
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        requests.push({ path, init })
        if (path.endsWith('/api/v1/auth/session')) {
          return jsonResponse(signedOutSession)
        }
        if (path.endsWith('/api/v1/project-invitations/resolve')) {
          expect(window.location.pathname).toBe('/join')
          expect(window.location.hash).toBe('')
          return jsonResponse(resolvedInvitation)
        }
        if (path.endsWith('/api/v1/project-invitations/accept-new')) {
          return jsonResponse(authenticatedSession)
        }
        throw new Error(`Unexpected request: ${path}`)
      }),
    )

    const rendered = renderJoin()
    expect(await screen.findByText(resolvedInvitation.projectName)).toBeVisible()
    expect(document.querySelector('img')).toBeNull()
    expect(
      (
        await axe.run(rendered.container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
    fireEvent.change(screen.getByLabelText('Display name'), {
      target: { value: 'New Human' },
    })
    fireEvent.change(screen.getByLabelText('Login name'), {
      target: { value: 'new.human' },
    })
    fireEvent.change(screen.getByLabelText('Project handle'), {
      target: { value: 'new_human' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.change(screen.getByLabelText('Confirm password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create account and join' }))

    expect(
      await screen.findByRole('heading', { name: 'Workspace home' }),
    ).toBeVisible()
    const acceptance = requests.find(({ path }) => path.endsWith('/accept-new'))
    expect(JSON.parse(String(acceptance?.init?.body))).toEqual({
      token: invitationToken,
      loginName: 'new.human',
      displayName: 'New Human',
      handle: 'new_human',
      password: 'correct horse battery staple',
    })
    expect(requests.every(({ path }) => !path.includes(invitationToken))).toBe(true)
  })

  it('accepts for the signed-in Actor with CSRF and no identity override', async () => {
    document.cookie = 'agent_workspace_csrf=csrf-test; Path=/'
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        if (path.endsWith('/api/v1/auth/session')) {
          return jsonResponse(authenticatedSession)
        }
        if (path.endsWith('/api/v1/project-invitations/resolve')) {
          return jsonResponse(resolvedInvitation)
        }
        if (path.endsWith('/api/v1/project-invitations/accept-existing')) {
          expect(new Headers(init?.headers).get('x-csrf-token')).toBe('csrf-test')
          expect(JSON.parse(String(init?.body))).toEqual({
            token: invitationToken,
            handle: 'existing_human',
          })
          return jsonResponse(authenticatedSession)
        }
        throw new Error(`Unexpected request: ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)

    renderJoin()
    await screen.findByText(resolvedInvitation.projectName)
    await waitFor(() => {
      expect(screen.queryByLabelText('Login name')).not.toBeInTheDocument()
    })
    fireEvent.change(screen.getByLabelText('Project handle'), {
      target: { value: 'existing_human' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Accept invitation' }))

    expect(
      await screen.findByRole('heading', { name: 'Workspace home' }),
    ).toBeVisible()
  })

  it('fails closed without a fragment token', async () => {
    window.history.replaceState({}, '', '/join')
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      void input
      return jsonResponse(signedOutSession)
    })
    vi.stubGlobal('fetch', fetchMock)

    renderJoin()

    const invalidMessage = await screen.findByText(
      'This invitation is invalid, expired, or has already been used.',
    )
    expect(invalidMessage.closest('[role="alert"]')).not.toBeNull()
    expect(
      fetchMock.mock.calls.some(([input]) => String(input).endsWith('/resolve')),
    ).toBe(false)
  })
})
