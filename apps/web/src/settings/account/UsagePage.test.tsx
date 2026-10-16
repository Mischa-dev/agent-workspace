import { render, screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { App } from '../../app/App'
import { installAppShellLifecycle, installStandardApi, jsonResponse } from '../../test/appHarness'

describe('usage and alerts', () => {
  installAppShellLifecycle()

  it('shows a plan with only a weekly window, counting down to its reset', async () => {
    const fetchMock = installStandardApi()
    const standard = fetchMock.getMockImplementation()
    const resetsAt = new Date(Date.now() + (2 * 60 + 5) * 60_000).toISOString()
    fetchMock.mockImplementation((input, init) => {
      if (String(input) === '/api/v1/usage') {
        return jsonResponse({
          accounts: [
            {
              accountId: 'acct-1',
              organizationId: 'org-1',
              name: 'Codex',
              harness: 'codex',
              harnessName: 'Codex',
              connected: true,
              plan: 'team',
              polledAt: new Date().toISOString(),
              pollFailedAt: null,
              pollFailure: null,
              windows: [
                {
                  key: 'codex.secondary',
                  label: 'Weekly',
                  source: 'reported',
                  scheduleId: null,
                  durationMinutes: 10080,
                  usedPercent: 62,
                  unusedPercent: 38,
                  resetsAt,
                  limitReached: false,
                  readAt: new Date().toISOString(),
                  rolledOver: false,
                },
              ],
              resetCredits: { available: 1, credits: [{ id: 'c1', title: null, expiresAt: null, grantedAt: null }] },
              schedules: [],
              rules: [
                { rule: 'reset_soon', description: '', enabled: true, hours: 3, percent: 25, defaultHours: 3, defaultPercent: 25 },
              ],
            },
          ],
        })
      }
      return standard ? standard(input, init) : jsonResponse({})
    })
    window.history.replaceState({}, '', '/settings/account/usage?account=acct-1')
    render(<App />)

    const windows = await screen.findByRole('list', { name: 'Usage windows' })
    expect(within(windows).getByText('Weekly')).toBeVisible()
    expect(within(windows).getByText('38% left')).toBeVisible()
    expect(within(windows).getByRole('meter', { name: 'Weekly: 62% used' })).toBeVisible()
    expect(within(windows).getByText(/^Resets in 2 h [45] min$/)).toBeVisible()
    expect(screen.getByText('1 banked reset')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Spend a reset' })).toBeEnabled()
  })
})
