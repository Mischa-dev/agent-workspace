import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { installAppShellLifecycle, installStandardApi, jsonResponse, projectAId } from '../test/appHarness'
import { App } from './App'

// AWD-75: a link into a module that's off shows a turned-off page, not a 404,
// and the module's rows leave the navigation.

function withModulesOff(modulesOff: string[], hidden: string[] = []) {
  const standard = installStandardApi()
  const effective = {
    source: 'default',
    preset: null,
    hidden,
    layoutHidden: [],
    words: {},
    lead: [],
    showingHidden: false,
    modulesOff,
  }
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) =>
    String(input).endsWith('/layout') ? jsonResponse({ effective }) : standard(input, init),
  )
}

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
})
