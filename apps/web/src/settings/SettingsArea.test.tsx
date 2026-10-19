import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { App } from '../app/App'
import { installAppShellLifecycle, installStandardApi, jsonResponse, projectAId, projectBId } from '../test/appHarness'
import { parseSettingsPath } from './route'

describe('settings addresses', () => {
  installAppShellLifecycle()

  it.each([
    ['/settings', '/settings/account/profile'],
    ['/settings/account', '/settings/account/profile'],
    ['/settings/appearance', '/settings/account/appearance'],
    [`/projects/${projectBId}/settings`, `/settings/projects/${projectBId}/general`],
    [`/projects/${projectBId}/settings?tab=members`, `/settings/projects/${projectBId}/members`],
    [`/projects/${projectBId}/settings?tab=agents`, `/settings/projects/${projectBId}/agents`],
    [`/projects/${projectBId}/settings?tab=codex`, `/settings/projects/${projectBId}/codex-runs`],
    [`/projects/${projectBId}/settings?tab=repositories`, `/settings/projects/${projectBId}/repositories`],
    [`/projects/${projectBId}/settings?tab=github`, `/settings/projects/${projectBId}/repositories`],
  ])('sends the old link %s to %s', async (from, to) => {
    installStandardApi()
    window.history.replaceState({}, '', from)
    render(<App />)
    await waitFor(() => expect(window.location.pathname).toBe(to))
    expect(await screen.findByRole('navigation', { name: 'Settings' })).toBeVisible()
  })

  it.each([
    ['/settings/workspace/run-defaults', '/settings/workspace/runs#defaults'],
    ['/settings/workspace/ai', '/settings/workspace/runs#suggestions'],
    [`/settings/projects/${projectBId}/environments`, `/settings/projects/${projectBId}/runs#environments`],
    [`/settings/projects/${projectBId}/ai`, `/settings/projects/${projectBId}/runs#suggestions`],
    // A link to one setting keeps pointing at it.
    [`/settings/projects/${projectBId}/environments#setting-run.environment`, `/settings/projects/${projectBId}/runs#setting-run.environment`],
  ])('sends the moved page %s to %s', async (from, to) => {
    installStandardApi()
    window.history.replaceState({}, '', from)
    render(<App />)
    await waitFor(() => expect(`${window.location.pathname}${window.location.hash}`).toBe(to))
  })

  it('completes a Project address without a page and sends unknown ones to the account', () => {
    expect(parseSettingsPath(`projects/${projectBId}`)).toEqual({
      kind: 'redirect',
      to: `/settings/projects/${projectBId}/general`,
    })
    expect(parseSettingsPath(`projects/${projectBId}`, '?tab=unknown')).toEqual({
      kind: 'redirect',
      to: `/settings/projects/${projectBId}/general`,
    })
    expect(parseSettingsPath('nowhere/at/all')).toEqual({ kind: 'redirect', to: '/settings/account/profile' })
  })

  it('says why GitHub didn’t connect when the return has no Project in it', async () => {
    installStandardApi()
    window.history.replaceState({}, '', '/settings?github=error&reason=expired')
    render(<App />)
    expect(await screen.findByText(/link expired/)).toBeVisible()
    await waitFor(() => expect(window.location.pathname).toBe('/settings/account/profile'))
    await waitFor(() => expect(window.location.search).toBe(''))
  })

  it('finds a single setting by name and links to the page that holds it', async () => {
    const fetchMock = installStandardApi()
    const standard = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input) === '/api/v1/settings/registry') {
        return jsonResponse({
          settings: [
            {
              key: 'run.reasoning',
              label: 'Reasoning',
              description: 'How hard the model thinks before answering.',
              valueType: { type: 'choice', options: ['default', 'high'] },
              scopes: ['workspace', 'user', 'project'],
              launchOverride: true,
              default: 'default',
            },
          ],
        })
      }
      return standard ? standard(input, init) : jsonResponse({})
    })
    window.sessionStorage.clear()
    window.history.replaceState({}, '', '/settings/account/profile')
    render(<App />)

    fireEvent.change(await screen.findByRole('searchbox', { name: 'Search settings' }), {
      target: { value: 'thinks' },
    })
    const results = await screen.findByRole('list', { name: 'Matching settings' })
    const links = within(results).getAllByRole('link')
    expect(links.map((link) => link.getAttribute('href'))).toEqual([
      '/settings/workspace/runs#setting-run.reasoning',
      `/settings/projects/${projectAId}/runs#setting-run.reasoning`,
    ])
    expect(within(results).getAllByText('Reasoning')).toHaveLength(2)
  })
})
