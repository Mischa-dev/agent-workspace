import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import {
  installAppShellLifecycle,
  installStandardApi,
  jsonResponse,
  organization,
  projectA,
  projectAId,
  projectB,
  projectBId,
} from '../test/appHarness'
import { App } from './App'

// AWD-75: a link into a module that's off shows a turned-off page, not a 404,
// and the module's rows leave the navigation. While the person's module
// settings load, a page or shortcut of a module that can be off waits rather
// than show or run first. This is the app hiding things; the server checks
// access on its own.

function effectiveWith(modulesOff: string[], hidden: string[] = []) {
  return {
    source: 'default',
    preset: null,
    hidden,
    layoutHidden: [],
    words: {},
    lead: [],
    showingHidden: false,
    modulesOff,
  }
}

/** These modules off in every layout and project; Projects is off in the projects listed in `projectsOff` (the default: all of them when it's off), as the server says it in `modulesOff`. */
function withModulesOff(
  modulesOff: string[],
  hidden: string[] = [],
  projectsOff = modulesOff.includes('projects') ? [projectAId, projectBId] : [],
) {
  const standard = installStandardApi()
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input)
    if (path.endsWith('/layout')) return jsonResponse({ effective: effectiveWith(modulesOff, hidden) })
    const flagged = (project: typeof projectA) => ({
      ...project,
      modulesOff: [...modulesOff.filter((module) => module !== 'projects'), ...(projectsOff.includes(project.id) ? ['projects'] : [])],
    })
    if (path.startsWith('/api/v1/projects?')) return jsonResponse({ projects: [projectA, projectB].map(flagged) })
    // A single project, opened or switched to, carries it too.
    for (const project of [projectA, projectB]) {
      if (path.endsWith(`/projects/${project.id}`)) return jsonResponse(flagged(project))
      if (path.endsWith(`/projects/${project.id}/switch`)) {
        return jsonResponse({ project: flagged(project), navigation: project.navigation, href: project.navigation.href })
      }
    }
    return standard(input, init)
  })
}

/** Layout requests wait until `answer` is called, with these modules off (or `fail` for an error). */
function withLayoutLater() {
  const standard = installStandardApi()
  const waiting: Array<(response: Promise<Response>) => void> = []
  let settled: (() => Promise<Response>) | null = null
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    if (!String(input).endsWith('/layout')) return standard(input, init)
    if (settled) return settled()
    return new Promise<Response>((resolve) => waiting.push((response) => void response.then(resolve)))
  })
  const settle = async (respond: () => Promise<Response>) => {
    settled = respond
    await act(async () => {
      for (const resolve of waiting.splice(0)) resolve(respond())
    })
  }
  return {
    answer: (modulesOff: string[]) => settle(() => jsonResponse({ effective: effectiveWith(modulesOff) })),
    fail: () => settle(() => Promise.resolve(new Response('{"error":{"code":"unavailable"}}', { status: 503 }))),
  }
}

const pagesPage = () => screen.queryByRole('heading', { level: 1, name: 'Pages' })

describe('modules that are off', () => {
  installAppShellLifecycle()

  it('a link into an off module opens the turned-off page', async () => {
    withModulesOff(['pages'], ['area.pages', 'area.decisions'])
    window.history.replaceState({}, '', `/projects/${projectAId}/pages`)
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'Pages is turned off' })).toBeVisible()
    expect(screen.getByText(/Nothing was deleted/)).toBeVisible()
    expect(screen.queryByRole('heading', { name: 'Page not found' })).toBeNull()
  })

  it('with Projects off, Home has no projects and the rail no project tiles', async () => {
    withModulesOff(['projects', 'channels', 'tasks', 'pages', 'whiteboards', 'code', 'runs', 'automations'])
    window.history.replaceState({}, '', '/home')
    render(<App />)
    expect(await screen.findByRole('heading', { name: /^(Welcome|Good (morning|afternoon|evening)), / })).toBeVisible()
    expect(await screen.findByRole('link', { name: 'Messages' })).toBeVisible()
    expect(screen.queryByRole('heading', { name: 'Recent projects' })).toBeNull()
    expect(screen.queryByRole('group', { name: 'Pinned and recent projects' })?.querySelector('a')).toBeFalsy()
    expect(screen.queryByRole('link', { name: 'New project' })).toBeNull()
  })

  it('the rail follows each project’s own Projects setting, not Home’s', async () => {
    const tile = `[data-rail-project="${projectAId}"]`
    // Projects off at Home but on in Agent Workspace (another workspace, say).
    withModulesOff(['projects'], [], [])
    window.history.replaceState({}, '', '/home')
    const first = render(<App />)
    await waitFor(() => expect(first.container.querySelector(tile)).toBeTruthy())
    first.unmount()
    // Projects on at Home but off in Agent Workspace.
    withModulesOff([], [], [projectAId])
    const second = render(<App />)
    // Once the list has loaded, Home lists Agent Workspace; the rail doesn't.
    await waitFor(() =>
      expect(
        screen
          .getAllByRole('link', { name: /^Agent Workspace/ })
          .some((link) => link.classList.contains('aw-row__link')),
      ).toBe(true),
    )
    expect(second.container.querySelector(tile)).toBeNull()
  })

  it('turning Projects off in Modules takes its project tiles out of the rail', async () => {
    let off = false
    const standard = installStandardApi()
    const modules = () => ({
      organizationId: organization.id,
      canManage: true,
      preset: null,
      presets: [],
      modules: [{ id: 'projects', name: 'Projects', description: 'Projects', switchable: true, on: !off, switchedOn: !off, needs: [] }],
    })
    vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input)
      if (path.endsWith('/layout')) return jsonResponse({ effective: effectiveWith(off ? ['projects'] : []) })
      if (path.endsWith(`/organizations/${organization.id}/modules`)) {
        if (init?.method === 'PUT') off = true
        return jsonResponse(modules())
      }
      if (path.startsWith('/api/v1/projects?')) {
        return jsonResponse({ projects: [projectA, projectB].map((project) => ({ ...project, modulesOff: off ? ['projects'] : [] })) })
      }
      return standard(input, init)
    })
    window.history.replaceState({}, '', '/settings/workspace/modules')
    const { container } = render(<App />)
    const tile = `[data-rail-project="${projectAId}"]`
    await waitFor(() => expect(container.querySelector(tile)).toBeTruthy())
    fireEvent.click(await screen.findByRole('switch', { name: 'Projects' }))
    await waitFor(() => expect(container.querySelector(tile)).toBeNull())
  })

  it('turning Projects off for a role you hold takes its project tiles out of the rail', async () => {
    let off = false
    const standard = installStandardApi()
    const role = () => ({
      id: '019fb600-0000-7000-8000-0000000000c0',
      key: 'member',
      builtIn: true,
      baseRole: 'member',
      name: 'Member',
      description: '',
      permissions: [],
      agentLimit: null,
      defaults: {},
      modulesOff: off ? ['projects'] : [],
      isDefault: true,
      version: off ? 2 : 1,
      updatedAt: '2026-11-01T00:00:00Z',
      workspaceHolders: 0,
      projectHolders: 1,
      capabilities: { edit: true, editPermissions: false, remove: false, assign: true, invite: true },
    })
    vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input)
      if (path.endsWith('/layout')) return jsonResponse({ effective: effectiveWith([]) })
      if (path.endsWith(`/organizations/${organization.id}/roles`)) {
        return jsonResponse({ roles: [role()], modules: [], permissions: [], startingPoints: [], canManage: true, yourPermissions: [] })
      }
      if (path.includes(`/organizations/${organization.id}/roles/`) && init?.method === 'PATCH') {
        off = true
        return jsonResponse(role())
      }
      // You hold Member in Agent Workspace: with Projects off for it, so is
      // that project for you.
      if (path.startsWith('/api/v1/projects?')) {
        return jsonResponse({
          projects: [projectA, projectB].map((project) => ({ ...project, modulesOff: off && project.id === projectAId ? ['projects'] : [] })),
        })
      }
      return standard(input, init)
    })
    window.history.replaceState({}, '', '/settings/workspace/roles')
    const { container } = render(<App />)
    const tile = `[data-rail-project="${projectAId}"]`
    await waitFor(() => expect(container.querySelector(tile)).toBeTruthy())
    fireEvent.click(await screen.findByRole('button', { name: 'Edit' }))
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Projects' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save role' }))
    await waitFor(() => expect(container.querySelector(tile)).toBeNull())
  })

  it('turning a module off for a role you hold reaches a project you had open', async () => {
    let off = false
    const standard = installStandardApi()
    const role = () => ({
      id: '019fb600-0000-7000-8000-0000000000c1',
      key: 'member',
      builtIn: true,
      baseRole: 'member',
      name: 'Member',
      description: '',
      permissions: [],
      agentLimit: null,
      defaults: {},
      modulesOff: off ? ['pages'] : [],
      isDefault: true,
      version: off ? 2 : 1,
      updatedAt: '2026-11-01T00:00:00Z',
      workspaceHolders: 0,
      projectHolders: 1,
      capabilities: { edit: true, editPermissions: false, remove: false, assign: true, invite: true },
    })
    vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input)
      // You hold Member in Agent Workspace, so its layout carries the role's modules.
      if (path.endsWith(`/projects/${projectAId}/layout`)) return jsonResponse({ effective: effectiveWith(off ? ['pages'] : []) })
      if (path.endsWith('/layout')) return jsonResponse({ effective: effectiveWith([]) })
      if (path.endsWith(`/organizations/${organization.id}/roles`)) {
        return jsonResponse({ roles: [role()], modules: [], permissions: [], startingPoints: [], canManage: true, yourPermissions: [] })
      }
      if (path.includes(`/organizations/${organization.id}/roles/`) && init?.method === 'PATCH') {
        off = true
        return jsonResponse(role())
      }
      return standard(input, init)
    })
    const go = (to: string) =>
      act(() => {
        window.history.pushState({}, '', to)
        window.dispatchEvent(new PopStateEvent('popstate'))
      })
    window.history.replaceState({}, '', `/projects/${projectAId}/pages`)
    render(<App />)
    expect(await screen.findByRole('heading', { level: 1, name: 'Pages' })).toBeVisible()
    go('/settings/workspace/roles')
    fireEvent.click(await screen.findByRole('button', { name: 'Edit' }))
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Pages' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save role' }))
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Save role' })).toBeNull())
    go(`/projects/${projectAId}/pages`)
    expect(await screen.findByRole('heading', { name: 'Pages is turned off' })).toBeVisible()
  })

  it('account and workspace settings leave out pages of modules that are off', async () => {
    // The account's Webhooks page and the workspace's Runs and models page
    // (a project has a page of that name too, which follows the project).
    const webhooks = 'a[href="/settings/account/webhooks"]'
    const runs = 'a[href="/settings/workspace/runs"]'
    withModulesOff([])
    window.history.replaceState({}, '', '/settings/account/profile')
    const first = render(<App />)
    await waitFor(() => {
      expect(first.container.querySelector(webhooks)).toBeTruthy()
      expect(first.container.querySelector(runs)).toBeTruthy()
    })
    first.unmount()
    withModulesOff(['automations', 'runs'])
    const second = render(<App />)
    expect(await screen.findByRole('link', { name: 'Profile' })).toBeVisible()
    await waitFor(() => {
      expect(second.container.querySelector(webhooks)).toBeNull()
      expect(second.container.querySelector(runs)).toBeNull()
    })
    // Search doesn't offer it either: Profile still matches, Webhooks doesn't.
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search settings' }), { target: { value: 'webhooks' } })
    expect(second.container.querySelector('a.aw-settings__result[href="/settings/account/webhooks"]')).toBeNull()
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search settings' }), { target: { value: 'profile' } })
    expect(await screen.findByRole('list', { name: 'Matching settings' })).toBeVisible()
    second.unmount()
    // And a link straight to it says Automations is turned off.
    window.history.replaceState({}, '', '/settings/account/webhooks')
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'Automations is turned off' })).toBeVisible()
  })

  it('switching to a project where Projects is off keeps its tile out of the rail', async () => {
    // Projects on at Home, off in Beta.
    withModulesOff([], [], [projectBId])
    window.history.replaceState({}, '', '/home')
    const { container } = render(<App />)
    // Home's recent projects row for Beta, once the list has loaded.
    const beta = await waitFor(() => {
      const link = screen
        .getAllByRole('link', { name: 'Beta Project' })
        .find((candidate) => candidate.classList.contains('aw-row__link'))
      expect(link).toBeTruthy()
      return link as HTMLElement
    })
    // The tile must never come back, not even until the list is next fetched.
    const tile = `[data-rail-project="${projectBId}"]`
    let appeared = false
    const watch = new MutationObserver(() => {
      if (container.querySelector(tile)) appeared = true
    })
    watch.observe(container, { childList: true, subtree: true })
    fireEvent.click(beta)
    await waitFor(() => expect(window.location.pathname).toContain(projectBId))
    // Let the switch's answer land in the cached list.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    watch.disconnect()
    expect(appeared).toBe(false)
    expect(container.querySelector(tile)).toBeNull()
  })

  it('a link into an off module waits for the settings and never shows the page first', async () => {
    const layout = withLayoutLater()
    window.history.replaceState({}, '', `/projects/${projectAId}/pages`)
    render(<App />)
    // Give the app every chance to show the page while the settings load.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    expect(pagesPage()).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Pages is turned off' })).toBeNull()
    await layout.answer(['pages'])
    expect(await screen.findByRole('heading', { name: 'Pages is turned off' })).toBeVisible()
    expect(pagesPage()).toBeNull()
    expect(window.location.pathname).toBe(`/projects/${projectAId}/pages`)
  })

  it('a link into a module that is on waits, keeps the address, then shows the page', async () => {
    const layout = withLayoutLater()
    window.history.replaceState({}, '', `/projects/${projectAId}/pages`)
    render(<App />)
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    expect(pagesPage()).toBeNull()
    await layout.answer([])
    expect(await screen.findByRole('heading', { level: 1, name: 'Pages' })).toBeVisible()
    expect(window.location.pathname).toBe(`/projects/${projectAId}/pages`)
    expect(screen.queryByRole('heading', { name: 'Pages is turned off' })).toBeNull()
  })

  it('where the settings can’t load, the page shows, as before', async () => {
    const layout = withLayoutLater()
    window.history.replaceState({}, '', `/projects/${projectAId}/pages`)
    render(<App />)
    await layout.fail()
    expect(await screen.findByRole('heading', { level: 1, name: 'Pages' }, { timeout: 5000 })).toBeVisible()
  })

  it('a shortcut into a module waits for the settings, then follows them', async () => {
    const layout = withLayoutLater()
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    render(<App />)
    const goPages = () => {
      fireEvent.keyDown(document, { key: 'g' })
      fireEvent.keyDown(document, { key: 'p' })
    }
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    // While the settings load, G then P does nothing.
    goPages()
    expect(window.location.pathname).toBe(`/projects/${projectAId}`)
    // With Pages off it still does nothing.
    await layout.answer(['pages'])
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    goPages()
    expect(window.location.pathname).toBe(`/projects/${projectAId}`)
  })

  it('a shortcut into a module that is on works once the settings say so', async () => {
    const layout = withLayoutLater()
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    render(<App />)
    await layout.answer([])
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    await waitFor(() => {
      fireEvent.keyDown(document, { key: 'g' })
      fireEvent.keyDown(document, { key: 'p' })
      expect(window.location.pathname).toBe(`/projects/${projectAId}/pages`)
    })
  })

  // A whiteboard's address has no project in it: the board's own project, or
  // its own workspace, decides, not Home's.
  function withBoard(board: { projectId: string | null; organizationId: string }, offAt: string) {
    const standard = installStandardApi()
    vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input)
      if (path.endsWith('/layout')) {
        return jsonResponse({ effective: effectiveWith(path.endsWith(`${offAt}/layout`) ? ['whiteboards'] : []) })
      }
      if (path.endsWith(`/api/v1/whiteboards/${boardId}`)) {
        return jsonResponse({ whiteboard: { id: boardId, title: 'Plans', deleted: false, ...board }, links: [], canEdit: true, revisions: [] })
      }
      return standard(input, init)
    })
  }
  const boardId = '019fb600-0000-7000-8000-0000000000b0'

  it('a project’s whiteboard follows that project’s modules, not Home’s', async () => {
    withBoard({ projectId: projectAId, organizationId: organization.id }, `/projects/${projectAId}`)
    window.history.replaceState({}, '', `/whiteboards/${boardId}`)
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'Whiteboards is turned off' })).toBeVisible()
    expect(window.location.pathname).toBe(`/whiteboards/${boardId}`)
  })

  it('a whiteboard across another workspace follows that workspace’s modules', async () => {
    const other = '019fb600-0000-7000-8000-0000000000b1'
    withBoard({ projectId: null, organizationId: other }, `/organizations/${other}`)
    window.history.replaceState({}, '', `/whiteboards/${boardId}`)
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'Whiteboards is turned off' })).toBeVisible()
  })
})
