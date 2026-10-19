import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { jsonResponse } from '../test/appHarness'
import { settingsKeys, type EffectiveSettings, type ResolvedSetting, type SettingDefinition, type SettingsTarget } from './api'
import { SettingField } from './SettingField'

const organizationId = '019fb600-0000-7000-8000-000000000000'
const projectId = '019fb600-0000-7000-8000-000000000002'
const target: SettingsTarget = { scope: 'project', organizationId, projectId }

const speed: SettingDefinition = {
  key: 'run.speed',
  label: 'Speed',
  description: 'Whether runs trade cost for speed.',
  valueType: { type: 'choice', options: ['default', 'standard', 'fast'] },
  scopes: ['workspace', 'user', 'project'],
  launchOverride: true,
  default: 'default',
}

const layer = (scope: 'workspace' | 'project', value: unknown, version: number) => ({
  scope,
  value,
  version,
  updatedAt: '2026-10-01T00:00:00Z',
  updatedByActorId: null,
})

const inherited: ResolvedSetting = {
  key: 'run.speed',
  value: 'fast',
  source: 'workspace',
  overrides: 'default',
  fallbackValue: 'default',
  layers: [layer('workspace', 'fast', 3)],
  default: 'default',
}

const overridden: ResolvedSetting = {
  key: 'run.speed',
  value: 'standard',
  source: 'project',
  overrides: 'workspace',
  fallbackValue: 'fast',
  layers: [layer('project', 'standard', 2), layer('workspace', 'fast', 3)],
  default: 'default',
}

function renderField(resolved: ResolvedSetting, canEdit = true) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const effective: EffectiveSettings = {
    organizationId,
    projectId,
    settings: [resolved],
    capabilities: { manageWorkspace: false, manageProject: true, manageOwn: true },
  }
  queryClient.setQueryData(settingsKeys.effective(target), effective)
  render(
    <QueryClientProvider client={queryClient}>
      <SettingField canEdit={canEdit} definition={speed} resolved={resolved} target={target} />
    </QueryClientProvider>,
  )
  return queryClient
}

function bodyOf(call: unknown[] | undefined) {
  return JSON.parse(String((call?.[1] as RequestInit | undefined)?.body))
}

describe('SettingField', () => {
  beforeEach(() => {
    document.cookie = 'agent_workspace_csrf=test-csrf; Path=/'
  })
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('shows what a project inherits and where from, read-only without the right to change it', () => {
    vi.stubGlobal('fetch', vi.fn())
    renderField(inherited, false)
    expect(screen.getByText('Fast')).toBeVisible()
    expect(screen.getByText(/Set in: Workspace/)).toBeVisible()
    expect(screen.queryByRole('switch', { name: 'Override' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Reset' })).not.toBeInTheDocument()
  })

  it('overrides at the project starting from the inherited value, with the version it was based on', async () => {
    const fetchMock = vi.fn(() => jsonResponse({ setting: { ...overridden, value: 'fast', layers: [layer('project', 'fast', 1), layer('workspace', 'fast', 3)] } }))
    vi.stubGlobal('fetch', fetchMock)
    renderField(inherited)

    fireEvent.click(screen.getByRole('switch', { name: 'Override' }))

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1))
    const [path, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe('/api/v1/settings/values/run.speed')
    expect(init.method).toBe('PUT')
    expect(bodyOf(fetchMock.mock.calls[0])).toEqual({
      scope: 'project',
      organizationId,
      projectId,
      value: 'fast',
      expectedVersion: 0,
    })
  })

  it('resets an override with its own version and says where the value will come from', async () => {
    const fetchMock = vi.fn(() => jsonResponse({ setting: inherited }))
    vi.stubGlobal('fetch', fetchMock)
    renderField(overridden)

    expect(screen.getByText('Set in: This project. Without it: Fast, from Workspace.')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Reset' }))

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1))
    const [path, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe('/api/v1/settings/values/run.speed/reset')
    expect(init.method).toBe('POST')
    expect(bodyOf(fetchMock.mock.calls[0])).toEqual({ scope: 'project', organizationId, projectId, expectedVersion: 2 })
  })

  it('refetches and says so when someone else changed the setting first', async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      if (String(input).startsWith('/api/v1/settings/effective')) {
        return jsonResponse({
          organizationId,
          projectId,
          settings: [overridden],
          capabilities: { manageWorkspace: false, manageProject: true, manageOwn: true },
        })
      }
      return jsonResponse({ error: { code: 'version_conflict', message: 'Changed.', details: { currentVersion: 3 } } }, 409)
    })
    vi.stubGlobal('fetch', fetchMock)
    const queryClient = renderField(overridden)

    fireEvent.click(screen.getByRole('radio', { name: 'Default' }))

    expect(await screen.findByText(/Someone else changed this setting just now/)).toBeVisible()
    expect(bodyOf(fetchMock.mock.calls[0])).toMatchObject({ value: 'default', expectedVersion: 2 })
    // The cached settings are marked stale so the page reloads them.
    expect(queryClient.getQueryState(settingsKeys.effective(target))?.isInvalidated).toBe(true)
  })
})
