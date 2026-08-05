import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import axe from 'axe-core'

import { ExternalAgentSettings } from './ExternalAgentSettings'

const projectId = '019fd100-0000-7000-8000-000000000001'
const connectionId = '019fd100-0000-7000-8000-000000000002'
const actorId = '019fd100-0000-7000-8000-000000000003'
const credentialId = '019fd100-0000-7000-8000-000000000004'
const token = `aw_ext_${'x'.repeat(43)}`
const hostileName = '<img src=x onerror=alert(1)>'

const connection = {
  id: connectionId,
  projectId,
  actorId,
  connectionName: 'Hermes M9',
  displayName: hostileName,
  handle: 'hermes',
  role: 'member',
  runtimeKind: 'hermes',
  runtimeVersion: '0.20.0',
  scopes: ['read', 'write', 'events'],
  status: 'active',
  version: 1,
  lastUsedAt: null,
  lastErrorCategory: null,
  disabledAt: null,
  removedAt: null,
  createdAt: '2026-08-05T07:00:00Z',
  updatedAt: '2026-08-05T07:00:00Z',
}

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }),
  )
}

function renderSettings() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
          },
        })
      }
    >
      <ExternalAgentSettings projectId={projectId} />
    </QueryClientProvider>,
  )
}

describe('ExternalAgentSettings', () => {
  beforeEach(() => {
    document.cookie = 'agent_workspace_csrf=csrf-external; Path=/'
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    document.cookie = 'agent_workspace_csrf=; Max-Age=0; Path=/'
  })

  it('renders hostile identity inert and exposes explicit reversible controls', async () => {
    const requests: Array<{ path: string; init?: RequestInit }> = []
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        requests.push({ path, init })
        if (!init?.method || init.method === 'GET') {
          return jsonResponse({ connections: [connection] })
        }
        if (init.method === 'PATCH') {
          return jsonResponse({
            connection: { ...connection, status: 'disabled', version: 2 },
            oneTimeCredential: null,
          })
        }
        throw new Error(`Unexpected request: ${path}`)
      }),
    )

    const rendered = renderSettings()
    expect(await screen.findByText(hostileName)).toBeVisible()
    expect(document.querySelector('img')).toBeNull()
    expect(
      (
        await axe.run(rendered.container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])

    fireEvent.click(screen.getByRole('button', { name: 'Disable' }))
    await waitFor(() => {
      const mutation = requests.find(({ init }) => init?.method === 'PATCH')
      expect(JSON.parse(String(mutation?.init?.body))).toEqual({
        expectedVersion: 1,
        enabled: false,
      })
      expect(new Headers(mutation?.init?.headers).get('x-csrf-token')).toBe(
        'csrf-external',
      )
    })

    fireEvent.click(screen.getByRole('button', { name: 'Remove…' }))
    expect(
      screen.getByRole('button', { name: 'Confirm removal' }),
    ).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(
      screen.queryByRole('button', { name: 'Confirm removal' }),
    ).not.toBeInTheDocument()
  })

  it('shows a created bearer once, copies it, and never puts it in a URL', async () => {
    const paths: string[] = []
    let created = false
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        paths.push(path)
        if (init?.method === 'POST') {
          created = true
          return jsonResponse({
            connection,
            oneTimeCredential: {
              id: credentialId,
              token,
              tokenPrefix: token.slice(0, 16),
              expiresAt: '2026-09-04T07:00:00Z',
            },
          })
        }
        return jsonResponse({ connections: created ? [connection] : [] })
      }),
    )

    renderSettings()
    await screen.findByText('No external agent connections have been created.')
    fireEvent.click(screen.getByRole('button', { name: 'Connect external agent' }))
    expect(await screen.findByText(token)).toBeVisible()
    expect(paths.every((path) => !path.includes('aw_ext_'))).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: 'Copy bearer' }))
    await waitFor(() => {
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(token)
      expect(screen.getByRole('button', { name: 'Copied' })).toBeVisible()
    })
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByText(token)).not.toBeInTheDocument()
  })
})
