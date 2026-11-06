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
      await screen.findByRole('heading', { name: /^(Welcome|Good (morning|afternoon|evening)), / }),
    ).toBeVisible()
    expect(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 unread',
      }),
    ).toBeVisible()
    const searchButton = screen.getByRole('button', { name: 'Search and commands' })
    fireEvent.click(searchButton)
    // The menu loads on first open.
    const search = await screen.findByRole('combobox', { name: 'Command menu' })
    // The box can show before the menu's effect gives it focus.
    await waitFor(() => expect(search).toHaveFocus())
    fireEvent.change(search, { target: { value: 'beta' } })
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByRole('option', { name: /^Beta Project/ })).toBeVisible()
    fireEvent.keyDown(dialog, { key: 'Escape' })
    await waitFor(() => expect(searchButton).toHaveFocus())

    const projectsButton = screen.getByRole('button', { name: 'Home: open navigation' })
    fireEvent.click(projectsButton)
    expect(screen.getByRole('navigation', { name: 'Spaces' })).toHaveAttribute(
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
      name: 'Agent Workspace, 3 unread',
    })
    fireEvent.click(projectALink)
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(
      `/projects/${projectAId}/threads/${threadId}`,
    )

    fireEvent.click(within(screen.getByRole('navigation', { name: 'Spaces' })).getByRole('link', { name: /^Home/ }))
    fireEvent.click(
      await within(screen.getByRole('navigation', { name: 'Spaces' })).findByRole('link', {
        name: 'Beta Project',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Beta Project' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(`/projects/${projectBId}`)

    fireEvent.click(within(screen.getByRole('navigation', { name: 'Spaces' })).getByRole('link', { name: /^Home/ }))
    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 unread',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()

    firstRender.unmount()
    installStandardApi(signedOutSession)
    window.history.replaceState({}, '', '/sign-in')
    render(<App />)
    fireEvent.change(await screen.findByLabelText('Username'), {
      target: { value: 'mischa' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(
      await screen.findByRole('heading', { name: /^(Welcome|Good (morning|afternoon|evening)), / }),
    ).toBeVisible()
    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 unread',
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
    const base = project(projectCId, 'GAMMA', 'Gamma Project')
    // Last seen on its work items, so opening the overview saves a new place.
    const projectC = {
      ...base,
      navigation: { ...base.navigation, location: { kind: 'work_items' as const }, href: `/projects/${projectCId}/work-items` },
    }
    window.history.replaceState({}, '', `/projects/${projectCId}`)
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
              location: { kind: 'project_overview' },
              href: `/projects/${projectCId}`,
              version: 3,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'Gamma Project' }),
    ).toBeVisible()
    await waitFor(() => expect(navigationWrites).toHaveLength(2))
    expect(navigationWrites.map((write) => write.expectedVersion)).toEqual([
      1, 2,
    ])
    expect(navigationWrites[1]).toMatchObject({
      location: { kind: 'project_overview' },
    })
    expect(navigationWrites[1]).not.toHaveProperty('actorId')
  })

  it('announces a terminal remembered-location failure and offers retry', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000089'
    const base = project(projectCId, 'DELTA', 'Delta Project')
    // Last seen on its work items, so opening the overview saves a new place.
    const projectC = {
      ...base,
      navigation: { ...base.navigation, location: { kind: 'work_items' as const }, href: `/projects/${projectCId}/work-items` },
    }
    window.history.replaceState({}, '', `/projects/${projectCId}`)
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
              location: { kind: 'project_overview' },
              href: `/projects/${projectCId}`,
              version: 3,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    // The overview's other panels aren't stubbed and show their own alerts.
    const alert = (await screen.findByText('Your place here wasn’t saved.')).closest<HTMLElement>('[role="alert"]')!
    expect(navigationWrites).toBe(2)
    fireEvent.click(
      within(alert).getByRole('button', { name: 'Try again' }),
    )
    await waitFor(() => expect(navigationWrites).toBe(3))
    await waitFor(() =>
      expect(
        screen.queryByText('Your place here wasn’t saved.'),
      ).not.toBeInTheDocument(),
    )
  })
})
