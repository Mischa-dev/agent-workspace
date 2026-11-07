import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { rememberPlace } from '../shell/recentPlaces'
import {
  installAppShellLifecycle,
  installStandardApi,
  jsonResponse,
  organization,
  projectA,
  projectAId,
  projectB,
  projectBId,
  user,
} from '../test/appHarness'
import { App } from './App'

// AWD-75: each entry point follows the modules of where it leads, there: a
// project's own page or action that project's, the rest Home's. Home and a
// project disagree in each test, both ways round.

function effectiveWith(modulesOff: string[]) {
  return { source: 'default', preset: null, hidden: [], layoutHidden: [], words: {}, lead: [], showingHidden: false, modulesOff }
}

type Scopes = {
  /** Off at Home (the workspace layout). */
  home: string[]
  /** Off in each project, in its layout and in the projects lists. */
  projects?: Record<string, string[]>
  /** Projects only search finds, beyond the first page of the list. */
  found?: Array<typeof projectA>
  /** Answers some requests first, such as a run or a task key. */
  extra?: (path: string) => Promise<Response> | undefined
  /** Holds Home's layout until the returned function is called. */
  homeLater?: boolean
}

function withScopes({ home, projects = {}, found = [], extra, homeLater }: Scopes) {
  const standard = installStandardApi()
  const off = (project: typeof projectA) => ({ ...project, modulesOff: projects[project.id] ?? [] })
  const waiting: Array<() => void> = []
  let released = !homeLater
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input)
    const answered = extra?.(path)
    if (answered) return answered
    const project = /\/projects\/([^/]+)\/layout$/.exec(path)?.[1]
    if (project) return jsonResponse({ effective: effectiveWith(projects[project] ?? []) })
    if (path.endsWith('/layout')) {
      if (released) return jsonResponse({ effective: effectiveWith(home) })
      return new Promise<Response>((resolve) => waiting.push(() => void jsonResponse({ effective: effectiveWith(home) }).then(resolve)))
    }
    if (path.startsWith('/api/v1/projects?')) {
      const searched = new URLSearchParams(path.split('?')[1]).get('q')
      return jsonResponse({ projects: [projectA, projectB, ...(searched ? found : [])].map(off) })
    }
    for (const listed of [projectA, projectB]) {
      if (path.endsWith(`/projects/${listed.id}`)) return jsonResponse(off(listed))
    }
    return standard(input, init)
  })
  return async () => {
    released = true
    await act(async () => {
      for (const release of waiting.splice(0)) release()
    })
  }
}

const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 60))
  })

const turnedOff = (name: string) => screen.queryByRole('heading', { name: `${name} is turned off` })

describe('entry points follow the modules where they lead', () => {
  installAppShellLifecycle()

  const runId = '019fb600-0000-7000-8000-0000000000d0'
  const run = {
    id: runId,
    state: 'completed',
    organizationId: organization.id,
    projectId: projectAId,
    projectKey: 'AW',
    projectName: 'Agent Workspace',
    conversationId: null,
    threadName: null,
    taskId: null,
    taskKey: null,
    taskTitle: null,
    inputTokens: 0,
    outputTokens: 0,
    cachedInputTokens: 0,
    cost: null,
    currency: null,
    createdAt: '2026-11-01T00:00:00Z',
    startedAt: null,
    finishedAt: null,
    updatedAt: '2026-11-01T00:00:00Z',
  }

  it('a run follows its own project’s Runs, waiting for the run, not Home’s', async () => {
    // Runs on at Home, off in the run's project; the run answers late.
    let answer: (() => void) | null = null
    withScopes({
      home: [],
      projects: { [projectAId]: ['runs'] },
      extra: (path) =>
        path.endsWith(`/agent-runs/${runId}`)
          ? new Promise<Response>((resolve) => (answer = () => void jsonResponse(run).then(resolve)))
          : undefined,
    })
    window.history.replaceState({}, '', `/runs/${runId}`)
    const first = render(<App />)
    await settle()
    expect(turnedOff('Runs')).toBeNull()
    await act(async () => answer?.())
    expect(await screen.findByRole('heading', { name: 'Runs is turned off' })).toBeVisible()
    first.unmount()

    // Off at Home, on in the run's project: the run's page, not the turned-off one.
    withScopes({ home: ['runs'], extra: (path) => (path.endsWith(`/agent-runs/${runId}`) ? jsonResponse(run) : undefined) })
    render(<App />)
    await settle()
    await settle()
    expect(turnedOff('Runs')).toBeNull()
  })

  it('a task key leaves the decision to the task’s project', async () => {
    const references = (path: string) =>
      path.includes(`/projects/${projectAId}/work-item-references`)
        ? jsonResponse({ items: [{ id: 'item-7', key: 'AW-7', title: 'Ship it', statusCategory: 'started', href: `/projects/${projectAId}/work-items/item-7` }] })
        : undefined
    // Tasks off at Home, on in AW: the key opens the task's project.
    withScopes({ home: ['tasks'], extra: references })
    window.history.replaceState({}, '', '/AW-7')
    const first = render(<App />)
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${projectAId}/work-items/item-7`))
    await settle()
    expect(turnedOff('Tasks')).toBeNull()
    first.unmount()

    // On at Home, off in AW: the project's page says so.
    withScopes({ home: [], projects: { [projectAId]: ['tasks'] }, extra: references })
    window.history.replaceState({}, '', '/AW-7')
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'Tasks is turned off' })).toBeVisible()
    expect(window.location.pathname).toBe(`/projects/${projectAId}/work-items/item-7`)
  })

  it('the command menu follows each entry’s own project, Home for the rest', async () => {
    const gamma = { ...projectB, id: '019fb600-0000-7000-8000-0000000000e1', key: 'GAMMA', name: 'Gamma Project' }
    const delta = { ...projectB, id: '019fb600-0000-7000-8000-0000000000e2', key: 'DELTA', name: 'Delta Project' }
    // Pages off in AW, the project you're in; on at Home and in Beta. Tasks
    // off in Beta. Projects off in Gamma, which only search finds.
    withScopes({
      home: [],
      projects: { [projectAId]: ['pages'], [projectBId]: ['tasks'], [gamma.id]: ['projects'] },
      found: [gamma, delta],
    })
    rememberPlace(user.actorId, { path: `/projects/${projectBId}/pages`, title: 'Beta pages', at: Date.now() })
    rememberPlace(user.actorId, { path: `/projects/${projectAId}/pages`, title: 'AW pages', at: Date.now() })
    rememberPlace(user.actorId, { path: `/projects/${projectBId}/work-items`, title: 'Beta tasks', at: Date.now() })
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    render(<App />)
    fireEvent.click(await screen.findByRole('button', { name: 'Search and commands' }))
    const menu = await screen.findByRole('dialog')
    // Recent places follow their own project: Beta's Pages show, AW's and Beta's Tasks don't.
    expect(await within(menu).findByRole('option', { name: /Beta pages/ })).toBeVisible()
    expect(within(menu).queryByRole('option', { name: /AW pages/ })).toBeNull()
    expect(within(menu).queryByRole('option', { name: /Beta tasks/ })).toBeNull()
    const search = screen.getByRole('combobox', { name: 'Command menu' })
    // Creating a task: in AW (on there), not in Beta (off there).
    fireEvent.change(search, { target: { value: 'create task' } })
    expect(await within(menu).findByRole('option', { name: /^Create task(?! in)/ })).toBeVisible()
    expect(within(menu).queryByRole('option', { name: /Create task in Beta Project/ })).toBeNull()
    // AW's own Pages entry is off; Home's Inbox is on.
    fireEvent.change(search, { target: { value: 'pages' } })
    await settle()
    expect(within(menu).queryByRole('option', { name: /^Pages/ })).toBeNull()
    fireEvent.change(search, { target: { value: 'inbox' } })
    expect(await within(menu).findByRole('option', { name: /^Inbox/ })).toBeVisible()
    // A project search finds: Delta shows, Gamma (Projects off there) doesn't.
    fireEvent.change(search, { target: { value: 'project' } })
    expect(await within(menu).findByRole('option', { name: /Delta Project/ })).toBeVisible()
    expect(within(menu).queryByRole('option', { name: /Gamma Project/ })).toBeNull()
  })

  it('the command menu holds Home’s entries back while Home’s settings load', async () => {
    const release = withScopes({ home: [], homeLater: true })
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    render(<App />)
    fireEvent.click(await screen.findByRole('button', { name: 'Search and commands' }))
    const menu = await screen.findByRole('dialog')
    fireEvent.change(screen.getByRole('combobox', { name: 'Command menu' }), { target: { value: 'inbox' } })
    await settle()
    expect(within(menu).queryByRole('option', { name: /^Inbox/ })).toBeNull()
    await release()
    expect(await within(menu).findByRole('option', { name: /^Inbox/ })).toBeVisible()
  })

  const press = (...keys: string[]) => {
    for (const key of keys) fireEvent.keyDown(document, { key })
  }

  it('shortcuts and their sheet agree: project keys follow the project, the rest Home', async () => {
    // Tasks on in AW, off at Home: G T opens AW's tasks; G W (My work) does nothing.
    withScopes({ home: ['tasks'] })
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    const first = render(<App />)
    await settle()
    await waitFor(() => {
      press('g', 't')
      expect(window.location.pathname).toBe(`/projects/${projectAId}/work-items`)
    })
    press('g', 'w')
    await settle()
    expect(window.location.pathname).not.toBe('/my-work')
    press('?')
    const sheet = await screen.findByRole('dialog', { name: 'Keyboard shortcuts' })
    expect(sheet.querySelector('dd[aria-label="G then T"]')).toBeTruthy()
    expect(sheet.querySelector('dd[aria-label="G then W"]')).toBeNull()
    first.unmount()

    // Off in AW, on at Home: the other way round.
    withScopes({ home: [], projects: { [projectAId]: ['tasks'] } })
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    render(<App />)
    await settle()
    await waitFor(() => {
      press('g', 'w')
      expect(window.location.pathname).toBe('/my-work')
    })
    window.history.pushState({}, '', `/projects/${projectAId}`)
    await act(async () => window.dispatchEvent(new PopStateEvent('popstate')))
    await settle()
    press('g', 't')
    await settle()
    expect(window.location.pathname).toBe(`/projects/${projectAId}`)
    press('?')
    const again = await screen.findByRole('dialog', { name: 'Keyboard shortcuts' })
    expect(again.querySelector('dd[aria-label="G then W"]')).toBeTruthy()
    expect(again.querySelector('dd[aria-label="G then T"]')).toBeNull()
  })

  it('global shortcuts and the sheet wait for Home’s settings', async () => {
    const release = withScopes({ home: [], homeLater: true })
    window.history.replaceState({}, '', `/projects/${projectAId}`)
    render(<App />)
    await settle()
    press('g', 'w')
    await settle()
    expect(window.location.pathname).toBe(`/projects/${projectAId}`)
    press('?')
    const sheet = await screen.findByRole('dialog', { name: 'Keyboard shortcuts' })
    expect(sheet.querySelector('dd[aria-label="G then W"]')).toBeNull()
    await release()
    await waitFor(() => expect(sheet.querySelector('dd[aria-label="G then W"]')).toBeTruthy())
  })
})
