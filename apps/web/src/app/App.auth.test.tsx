import {
  fireEvent,
  render,
  screen,
  within,
} from '@testing-library/react'
import axe from 'axe-core'
import { describe, expect, it, vi } from 'vitest'

import type { SessionResponse } from '../auth/session'
import {
  projectAId,
  user,
  authenticatedSession,
  signedOutSession,
  projectA,
  projectB,
  jsonResponse,
  installStandardApi,
  installAppShellLifecycle,
} from '../test/appHarness'
import { App } from './App'

describe('App sign-in and session boundary', () => {
  installAppShellLifecycle()

  it('redirects a signed-out protected route to the sign-in boundary', async () => {
    installStandardApi(signedOutSession)
    const { container } = render(<App />)

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    expect(
      screen.getByText(/link WorkOS from Account settings/i),
    ).toBeVisible()
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
  })

  it('signs in without writing credentials or tokens to browser storage', async () => {
    window.history.replaceState({}, '', '/sign-in?returnTo=%2Fhome')
    const storageSpy = vi.spyOn(Storage.prototype, 'setItem')
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        if (path.endsWith('/auth/session')) return jsonResponse(signedOutSession)
        if (path.endsWith('/auth/login')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?')) {
          return jsonResponse({ projects: [projectA] })
        }
        throw new Error(`Unexpected request: ${String(init?.method)} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    render(<App />)

    fireEvent.change(await screen.findByLabelText('Login name'), {
      target: { value: 'mischa' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect(
      await screen.findByRole('heading', { name: /Welcome back/ }),
    ).toBeVisible()
    const loginCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith('/auth/login'),
    )
    expect(loginCall?.[1]).toEqual(
      expect.objectContaining({
        credentials: 'same-origin',
        method: 'POST',
      }),
    )
    expect(storageSpy).not.toHaveBeenCalled()
    storageSpy.mockRestore()
  })

  it('signs out through the CSRF-protected session action', async () => {
    const fetchMock = installStandardApi()
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Account: Mischa Nelson' }),
    )
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    const logoutCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith('/auth/logout'),
    )
    expect(logoutCall?.[1]).toEqual(
      expect.objectContaining({
        credentials: 'same-origin',
        method: 'POST',
      }),
    )
    expect(new Headers(logoutCall?.[1]?.headers).get('x-csrf-token')).toBe(
      'test-csrf',
    )
  })

  it('drops prior Actor caches after an automatic 401 before another login', async () => {
    const nextActorSession: SessionResponse = {
      ...authenticatedSession,
      user: {
        ...user,
        id: '019fb600-0000-7000-8000-00000000000a',
        actorId: '019fb600-0000-7000-8000-00000000000b',
        loginName: 'next',
        displayName: 'Next Actor',
      },
    }
    let nextActor = false
    let projectListReads = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.endsWith('/auth/login')) {
            nextActor = true
            return jsonResponse(nextActorSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            projectListReads += 1
            return jsonResponse({
              projects: nextActor ? [projectB] : [projectA],
            })
          }
          if (
            path.endsWith(`/projects/${projectAId}/navigation`) &&
            method === 'GET'
          ) {
            return jsonResponse(
              {
                error: {
                  code: 'authentication_required',
                  message: 'Sign in to continue.',
                },
              },
              401,
            )
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    )
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()

    fireEvent.change(screen.getByLabelText('Login name'), {
      target: { value: 'next' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect(
      await screen.findByRole('heading', { name: /Welcome back, Next Actor/ }),
    ).toBeVisible()
    expect(
      await within(await screen.findByRole('navigation', { name: 'Projects' })).findByRole('link', {
        name: 'Beta Project',
      }),
    ).toBeVisible()
    expect(
      screen.queryByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    ).not.toBeInTheDocument()
    expect(projectListReads).toBeGreaterThanOrEqual(2)
  })
})
