import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { SessionResponse } from '../auth/session'
import {
  projectBId,
  adminOrganizationId,
  ownerOrganizationId,
  memberOrganizationId,
  authenticatedSession,
  project,
  projectA,
  projectB,
  jsonResponse,
  installStandardApi,
  installAppShellLifecycle,
} from '../test/appHarness'
import { App } from './App'

describe('App Project creation and settings', () => {
  installAppShellLifecycle()

  it('keeps one idempotency key while retrying Project creation', async () => {
    window.history.replaceState({}, '', '/projects/new')
    let createAttempts = 0
    const idempotencyKeys: Array<string | null> = []
    const created = project(
      '019fb600-0000-7000-8000-000000000099',
      'TRI',
      'Trident',
    )
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({ projects: [] })
        }
        if (path.endsWith('/projects') && method === 'POST') {
          idempotencyKeys.push(
            new Headers(init?.headers).get('idempotency-key'),
          )
          createAttempts += 1
          return createAttempts === 1
            ? jsonResponse(
                {
                  error: {
                    code: 'internal_error',
                    message: 'The request could not be completed.',
                  },
                },
                500,
              )
            : jsonResponse(created, 201)
        }
        if (path.endsWith(`/${created.id}/switch`)) {
          return jsonResponse({
            project: created,
            navigation: created.navigation,
            href: `/projects/${created.id}`,
          })
        }
        if (path.endsWith(`/${created.id}`)) return jsonResponse(created)
        if (path.endsWith(`/${created.id}/navigation`)) {
          return jsonResponse(created.navigation)
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    render(<App />)

    fireEvent.change(await screen.findByLabelText('Project name'), {
      target: { value: 'Trident' },
    })
    fireEvent.change(screen.getByLabelText('Project key'), {
      target: { value: 'tri' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create Project' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The request could not be completed.',
    )
    fireEvent.click(screen.getByRole('button', { name: 'Create Project' }))

    await waitFor(() => expect(createAttempts).toBe(2))
    expect(idempotencyKeys[0]).toBeTruthy()
    expect(idempotencyKeys[1]).toBe(idempotencyKeys[0])
  })

  it('offers Project creation only in Owner and Admin Organizations', async () => {
    window.history.replaceState({}, '', '/projects/new')
    const sessionWithMixedRoles: SessionResponse = {
      ...authenticatedSession,
      organizations: [
        {
          id: memberOrganizationId,
          name: 'Member Organization',
          slug: 'member',
          role: 'member',
        },
        {
          id: adminOrganizationId,
          name: 'Admin Organization',
          slug: 'admin',
          role: 'admin',
        },
        {
          id: ownerOrganizationId,
          name: 'Owner Organization',
          slug: 'owner',
          role: 'owner',
        },
      ],
    }
    installStandardApi(sessionWithMixedRoles)
    render(<App />)

    const selector = await screen.findByLabelText('Organization')
    expect(selector).toHaveValue(adminOrganizationId)
    expect(
      within(selector).queryByRole('option', {
        name: 'Member Organization',
      }),
    ).not.toBeInTheDocument()
    expect(
      within(selector).getByRole('option', { name: 'Admin Organization' }),
    ).toBeVisible()
    expect(
      within(selector).getByRole('option', { name: 'Owner Organization' }),
    ).toBeVisible()
  })

  it('blocks direct Project creation when no eligible Organization exists', async () => {
    window.history.replaceState({}, '', '/projects/new')
    installStandardApi({
      ...authenticatedSession,
      organizations: [
        {
          id: memberOrganizationId,
          name: 'Member Organization',
          slug: 'member',
          role: 'member',
        },
      ],
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', {
        name: 'Project creation unavailable',
      }),
    ).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Create Project' }),
    ).not.toBeInTheDocument()
  })

  it('leads out of Project settings through the Project tile', async () => {
    // An older client saved settings as Beta's last place.
    const rememberedSettings = {
      ...projectB.navigation,
      location: { kind: 'settings' },
      href: `/projects/${projectBId}/settings`,
    }
    window.history.replaceState({}, '', `/settings/projects/${projectBId}/general`)
    const navigationWrites: string[] = []
    const fetchMock = installStandardApi()
    const standard = fetchMock.getMockImplementation()!
    fetchMock.mockImplementation(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({
            projects: [projectA, { ...projectB, navigation: rememberedSettings }],
          })
        }
        if (path.endsWith(`/projects/${projectBId}`)) {
          return jsonResponse({ ...projectB, navigation: rememberedSettings })
        }
        if (path.endsWith(`/projects/${projectBId}/navigation`)) {
          if (method !== 'GET') navigationWrites.push(String(init?.body))
          return jsonResponse(rememberedSettings)
        }
        if (path.endsWith(`/projects/${projectBId}/switch`) && method === 'POST') {
          return jsonResponse({
            project: projectB,
            navigation: rememberedSettings,
            href: rememberedSettings.href,
          })
        }
        return standard(input, init)
      },
    )
    render(<App />)

    expect(await screen.findByRole('heading', { level: 1, name: 'General' })).toBeVisible()
    fireEvent.click(
      within(screen.getByRole('navigation', { name: 'Projects' })).getByRole('link', {
        name: /^Beta Project/,
      }),
    )
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${projectBId}`))
    // Settings never save themselves as the place to come back to.
    expect(navigationWrites.filter((body) => body.includes('"settings"'))).toEqual([])
  })

  it('requires the exact Project key and archives with its expected version', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectBId}/settings`,
    )
    let archived = false
    const fetchMock = installStandardApi()
    fetchMock.mockImplementation(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({
            projects: archived ? [projectA] : [projectA, projectB],
          })
        }
        if (path.endsWith(`/projects/${projectBId}`)) {
          return jsonResponse(projectB)
        }
        if (path.endsWith(`/projects/${projectBId}/navigation`)) {
          return jsonResponse({
            ...projectB.navigation,
            location: { kind: 'settings' },
            href: `/projects/${projectBId}/settings`,
          })
        }
        if (
          path.endsWith(`/projects/${projectBId}/archive`) &&
          method === 'POST'
        ) {
          archived = true
          return jsonResponse({
            ...projectB,
            archivedAt: '2026-07-31T07:00:00Z',
            version: 2,
          })
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    render(<App />)

    const archiveButton = await screen.findByRole('button', {
      name: 'Archive Project',
    })
    expect(archiveButton).toBeDisabled()
    fireEvent.change(screen.getByLabelText(/Type BETA to confirm/), {
      target: { value: 'BETA' },
    })
    fireEvent.click(archiveButton)

    expect(
      await screen.findByText(/Beta Project was archived/),
    ).toBeVisible()
    const archiveCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith(`/${projectBId}/archive`),
    )
    expect(archiveCall?.[1]?.body).toBe(JSON.stringify({ expectedVersion: 1 }))
    expect(new Headers(archiveCall?.[1]?.headers).get('x-csrf-token')).toBe(
      'test-csrf',
    )
    expect(
      screen.queryByRole('link', { name: 'Beta Project' }),
    ).not.toBeInTheDocument()
  })

  it.each([
    {
      code: 'version_conflict',
      message: 'Project changed. Reload before archiving.',
      details: { currentVersion: 2 },
    },
    {
      code: 'project_has_active_runs',
      message: 'Interrupt active runs before archiving.',
    },
  ])('keeps archive conflicts recoverable for $code', async ({
    code,
    details,
    message,
  }) => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectBId}/settings`,
    )
    let archiveAttempts = 0
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) {
          return jsonResponse(authenticatedSession)
        }
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({ projects: [projectA, projectB] })
        }
        if (path.endsWith(`/projects/${projectBId}`)) {
          // Someone else changed the Project before the first archive attempt.
          return jsonResponse({
            ...projectB,
            version: code === 'version_conflict' && archiveAttempts > 0 ? 2 : 1,
          })
        }
        if (
          path.endsWith(`/projects/${projectBId}/navigation`) &&
          method === 'GET'
        ) {
          return jsonResponse({
            ...projectB.navigation,
            location: { kind: 'settings' },
            href: `/projects/${projectBId}/settings`,
          })
        }
        if (
          path.endsWith(`/projects/${projectBId}/archive`) &&
          method === 'POST'
        ) {
          archiveAttempts += 1
          return jsonResponse({ error: { code, details, message } }, 409)
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    render(<App />)

    fireEvent.change(
      await screen.findByLabelText(/Type BETA to confirm/),
      { target: { value: 'BETA' } },
    )
    fireEvent.click(screen.getByRole('button', { name: 'Archive Project' }))

    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent(
      code === 'version_conflict' ? 'Someone changed this Project' : message,
    )
    await waitFor(() => expect(alert).toHaveFocus())
    expect(window.location.pathname).toBe(
      `/settings/projects/${projectBId}/general`,
    )
    expect(
      screen.getByRole('link', { name: 'Beta Project' }),
    ).toBeVisible()
    const archiveCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith(`/${projectBId}/archive`),
    )
    expect(archiveCall?.[1]?.body).toBe(
      JSON.stringify({ expectedVersion: 1 }),
    )
    if (code === 'version_conflict') {
      fireEvent.click(
        screen.getByRole('button', { name: 'Archive Project' }),
      )
      await waitFor(() =>
        expect(
          fetchMock.mock.calls.filter(([path]) =>
            String(path).endsWith(`/${projectBId}/archive`),
          ),
        ).toHaveLength(2),
      )
      const archiveCalls = fetchMock.mock.calls.filter(([path]) =>
        String(path).endsWith(`/${projectBId}/archive`),
      )
      expect(archiveCalls[1][1]?.body).toBe(
        JSON.stringify({ expectedVersion: 2 }),
      )
    }
  })

  it('keeps Viewer Project controls read-only', async () => {
    const viewerProject = {
      ...projectB,
      role: 'viewer',
      capabilities: {
        view: true,
        manage: false,
        archive: false,
        navigationWrite: true,
      },
      navigation: {
        ...projectB.navigation,
        location: { kind: 'settings' },
        href: `/projects/${projectBId}/settings`,
      },
    }
    window.history.replaceState({}, '', `/projects/${projectBId}/settings`)
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
            return jsonResponse({ projects: [viewerProject] })
          }
          if (path.endsWith(`/projects/${projectBId}`)) {
            return jsonResponse(viewerProject)
          }
          if (path.endsWith(`/projects/${projectBId}/navigation`)) {
            return jsonResponse(viewerProject.navigation)
          }
          if (
            path.endsWith(`/projects/${projectBId}/repositories`) &&
            method === 'GET'
          ) {
            return jsonResponse({ canManage: false, repositories: [] })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'General' }),
    ).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Archive Project' }),
    ).not.toBeInTheDocument()
    expect(screen.getByText('Viewer')).toBeVisible()
    expect(screen.getByRole('note')).toHaveTextContent('Viewer access is read-only')
    const sections = screen.getByRole('list', { name: 'Project settings' })
    expect(within(sections).queryByRole('link', { name: 'Run requests' })).not.toBeInTheDocument()
    expect(within(sections).queryByRole('link', { name: 'Automation' })).not.toBeInTheDocument()
    expect(within(sections).queryByRole('link', { name: 'Agents' })).not.toBeInTheDocument()
    expect(within(sections).queryByRole('link', { name: 'Members' })).not.toBeInTheDocument()
    fireEvent.click(within(sections).getByRole('link', { name: 'Repositories' }))
    expect(await screen.findByRole('heading', { name: 'Repositories' })).toBeVisible()
    expect(
      screen.queryByRole('heading', { name: 'Accounts for runs' }),
    ).not.toBeInTheDocument()
  })
})
