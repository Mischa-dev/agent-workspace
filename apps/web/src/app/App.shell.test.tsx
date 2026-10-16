import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
import axe from 'axe-core'
import { describe, expect, it, vi } from 'vitest'

import {
  projectAId,
  projectBId,
  threadId,
  authenticatedSession,
  signedOutSession,
  project,
  projectA,
  projectB,
  jsonResponse,
  installStandardApi,
  installAppShellLifecycle,
} from '../test/appHarness'
import { App } from './App'

describe('App shell and Project navigation', () => {
  installAppShellLifecycle()

  it('renders server-backed rail, attention labels, search, and accessible drawers', async () => {
    installStandardApi()
    const { container } = render(<App />)

    expect(
      await screen.findByRole('heading', { name: /Welcome back/ }),
    ).toBeVisible()
    expect(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    ).toBeVisible()
    const searchButton = screen.getByRole('button', { name: 'Search and commands' })
    fireEvent.click(searchButton)
    const search = screen.getByRole('combobox', { name: 'Command menu' })
    expect(search).toHaveFocus()
    fireEvent.change(search, { target: { value: 'beta' } })
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByRole('option', { name: /^Beta Project/ })).toBeVisible()
    fireEvent.keyDown(dialog, { key: 'Escape' })
    await waitFor(() => expect(searchButton).toHaveFocus())

    const projectsButton = screen.getByRole('button', { name: 'Agent Workspace: open navigation' })
    fireEvent.click(projectsButton)
    expect(screen.getByRole('navigation', { name: 'Projects' })).toHaveAttribute(
      'data-mobile-open',
      'true',
    )
    fireEvent.keyDown(document, { key: 'Escape' })
    await waitFor(() => expect(projectsButton).toHaveFocus())
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
  })

  it('finishes the Project rail when a hard-route location sync cancels its cold read', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000006'
    const postgresTimestampProjects = [
      {
        ...projectA,
        navigation: {
          ...projectA.navigation,
          lastOpenedAt: '2026-07-31 06:00:00.123456+00',
        },
      },
      {
        ...projectB,
        navigation: {
          ...projectB.navigation,
          lastOpenedAt: '2026-07-31 05:00:00.123456+00',
        },
      },
      project(projectCId, 'GAMMA', 'Gamma Project', {
        navigation: {
          location: { kind: 'project_overview' },
          href: `/projects/${projectCId}`,
          pinned: false,
          lastOpenedAt: '2026-07-31 04:00:00.123456+00',
          version: 1,
        },
      }),
    ]
    window.history.replaceState({}, '', `/projects/${projectBId}`)
    let projectListReads = 0
    let releaseInitialList: ((response: Response) => void) | undefined
    const initialListRead = new Promise<Response>((resolve) => {
      releaseInitialList = resolve
    })
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            projectListReads += 1
            if (projectListReads === 1) return initialListRead
            releaseInitialList?.(
              new Response(JSON.stringify({ projects: postgresTimestampProjects }), {
                headers: { 'content-type': 'application/json' },
              }),
            )
            return jsonResponse({ projects: postgresTimestampProjects })
          }
          if (path.endsWith(`/projects/${projectBId}`)) {
            return jsonResponse(projectB)
          }
          if (
            path.endsWith(`/projects/${projectBId}/navigation`) &&
            method === 'GET'
          ) {
            return jsonResponse(projectB.navigation)
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'Beta Project' }),
    ).toBeVisible()
    expect(
      await screen.findByRole('link', { name: 'Gamma Project' }),
    ).toBeVisible()
    expect(screen.queryByLabelText('Loading Projects')).not.toBeInTheDocument()
    expect(projectListReads).toBe(2)
  })

  it('restores independent A to B to A locations after a client reload', async () => {
    installStandardApi()
    const firstRender = render(<App />)
    const projectALink = await screen.findByRole('link', {
      name: 'Agent Workspace, 3 items need attention',
    })
    fireEvent.click(projectALink)
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(
      `/projects/${projectAId}/threads/${threadId}`,
    )

    fireEvent.click(screen.getByRole('link', { name: 'Home' }))
    fireEvent.click(
      await within(screen.getByRole('navigation', { name: 'Projects' })).findByRole('link', {
        name: 'Beta Project',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Beta Project' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(`/projects/${projectBId}`)

    fireEvent.click(screen.getByRole('link', { name: 'Home' }))
    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()

    firstRender.unmount()
    installStandardApi(signedOutSession)
    window.history.replaceState({}, '', '/sign-in')
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
    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(
      `/projects/${projectAId}/threads/${threadId}`,
    )
  })

  it('serializes a navigation conflict retry with the newest expected version', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000088'
    const projectC = project(projectCId, 'GAMMA', 'Gamma Project')
    window.history.replaceState({}, '', `/projects/${projectCId}/settings`)
    let navigationReads = 0
    const navigationWrites: Array<Record<string, unknown>> = []
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            return jsonResponse({ projects: [projectC] })
          }
          if (path.endsWith(`/projects/${projectCId}`)) {
            return jsonResponse(projectC)
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'GET'
          ) {
            navigationReads += 1
            return jsonResponse({
              ...projectC.navigation,
              version: navigationReads,
            })
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'PUT'
          ) {
            const body = JSON.parse(String(init?.body)) as Record<string, unknown>
            navigationWrites.push(body)
            if (navigationWrites.length === 1) {
              return jsonResponse(
                {
                  error: {
                    code: 'version_conflict',
                    details: { currentVersion: 2 },
                    message: 'Navigation changed.',
                  },
                },
                409,
              )
            }
            return jsonResponse({
              ...projectC.navigation,
              location: { kind: 'settings' },
              href: `/projects/${projectCId}/settings`,
              version: 3,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { name: 'Project settings' }),
    ).toBeVisible()
    await waitFor(() => expect(navigationWrites).toHaveLength(2))
    expect(navigationWrites.map((write) => write.expectedVersion)).toEqual([
      1, 2,
    ])
    expect(navigationWrites[1]).toMatchObject({
      location: { kind: 'settings' },
    })
    expect(navigationWrites[1]).not.toHaveProperty('actorId')
  })

  it('announces a terminal remembered-location failure and offers retry', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000089'
    const projectC = project(projectCId, 'DELTA', 'Delta Project')
    window.history.replaceState({}, '', `/projects/${projectCId}/settings`)
    let navigationReads = 0
    let navigationWrites = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            return jsonResponse({ projects: [projectC] })
          }
          if (path.endsWith(`/projects/${projectCId}`)) {
            return jsonResponse(projectC)
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'GET'
          ) {
            navigationReads += 1
            return jsonResponse({
              ...projectC.navigation,
              version: navigationReads === 1 ? 1 : 2,
            })
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'PUT'
          ) {
            navigationWrites += 1
            if (navigationWrites === 1) {
              return jsonResponse(
                {
                  error: {
                    code: 'version_conflict',
                    details: { currentVersion: 2 },
                    message: 'Navigation changed.',
                  },
                },
                409,
              )
            }
            if (navigationWrites === 2) {
              return jsonResponse(
                {
                  error: {
                    code: 'internal_error',
                    message: 'Navigation could not be saved.',
                  },
                },
                500,
              )
            }
            return jsonResponse({
              ...projectC.navigation,
              location: { kind: 'settings' },
              href: `/projects/${projectCId}/settings`,
              version: 3,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('Navigation could not be saved.')
    expect(navigationWrites).toBe(2)
    fireEvent.click(
      within(alert).getByRole('button', { name: 'Try saving again' }),
    )
    await waitFor(() => expect(navigationWrites).toBe(3))
    await waitFor(() =>
      expect(
        screen.queryByText('Navigation could not be saved.'),
      ).not.toBeInTheDocument(),
    )
  })
})
