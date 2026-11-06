import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { installAppShellLifecycle, installStandardApi, jsonResponse, projectAId } from '../test/appHarness'
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

function withModulesOff(modulesOff: string[], hidden: string[] = []) {
  const standard = installStandardApi()
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) =>
    String(input).endsWith('/layout') ? jsonResponse({ effective: effectiveWith(modulesOff, hidden) }) : standard(input, init),
  )
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
})
