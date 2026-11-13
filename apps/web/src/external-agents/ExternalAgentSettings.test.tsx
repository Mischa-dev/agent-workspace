import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
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

    fireEvent.click(screen.getByRole('button', { name: `${hostileName} actions` }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Disable' }))
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

    fireEvent.click(screen.getByRole('button', { name: `${hostileName} actions` }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove…' }))
    const confirm = await screen.findByRole('dialog', { name: `Remove ${hostileName}?` })
    expect(confirm).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() =>
      expect(
        screen.queryByRole('dialog', { name: `Remove ${hostileName}?` }),
      ).not.toBeInTheDocument(),
    )
    expect(requests.some(({ path }) => path.endsWith('/remove'))).toBe(false)
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
    await screen.findByText('No agents connected yet')
    fireEvent.change(screen.getByLabelText('Connection name'), {
      target: { value: 'Hermes M9' },
    })
    fireEvent.change(screen.getByLabelText('Display name'), {
      target: { value: 'Hermes teammate' },
    })
    fireEvent.change(screen.getByLabelText('Handle'), {
      target: { value: 'hermes' },
    })
    fireEvent.change(screen.getByLabelText('Set it up in'), {
      target: { value: '' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Connect agent' }))
    expect(await screen.findByText(token)).toBeVisible()
    expect(paths.every((path) => !path.includes('aw_ext_'))).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: 'Copy token' }))
    await waitFor(() => {
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(token)
      expect(screen.getByRole('button', { name: 'Copied' })).toBeVisible()
    })
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByText(token)).not.toBeInTheDocument()
  })
  it('adds a Codex runner in one form and shows its settings once', async () => {
    const posts: Array<{ path: string; body: unknown }> = []
    const secret = `whsec_${'s'.repeat(43)}`
    let created = false
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        if (init?.method === 'POST') {
          created = true
          posts.push({ path, body: JSON.parse(String(init.body)) })
          return jsonResponse({
            connection: { ...connection, displayName: 'Codex', handle: 'codex' },
            oneTimeCredential: {
              id: credentialId,
              token,
              tokenPrefix: token.slice(0, 16),
              expiresAt: '2026-09-04T07:00:00Z',
            },
            webhookSecret: secret,
          })
        }
        return jsonResponse({ connections: created ? [connection] : [] })
      }),
    )

    renderSettings()
    await screen.findByText('No agents connected yet')
    fireEvent.click(screen.getByRole('radio', { name: 'Codex' }))
    expect(screen.getByLabelText('Handle')).toHaveValue('codex')
    fireEvent.change(screen.getByLabelText('Runner URL (optional)'), {
      target: { value: 'https://runner.example/wake' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Add runner' }))
    const settings = await screen.findByLabelText('Runner settings')
    expect(posts).toHaveLength(1)
    expect(posts[0].body).toMatchObject({
      displayName: 'Codex',
      handle: 'codex',
      role: 'member',
      webhookUrl: 'https://runner.example/wake',
    })
    expect(settings.textContent).toContain(`AGENT_WORKSPACE_TOKEN=${token}`)
    expect(settings.textContent).toContain(`AGENT_WORKSPACE_WEBHOOK_SECRET=${secret}`)
    expect(settings.textContent).toContain('AGENT_WORKSPACE_RUNNER=codex')
    expect(posts.every(({ path }) => !path.includes('aw_ext_') && !path.includes('whsec_'))).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: 'Copy settings' }))
    await waitFor(() => {
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(settings.textContent)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByText(secret, { exact: false })).not.toBeInTheDocument()
  })

  it('makes a one-time setup message for an agent’s app without showing its token', async () => {
    const posts: Array<{ path: string; body: unknown }> = []
    const url = 'https://workspace.test/connect/agent/abc123'
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        if (init?.method === 'POST') {
          posts.push({ path, body: JSON.parse(String(init.body)) })
          return jsonResponse({
            connection: { ...connection, version: 2 },
            oneTimeCredential: null,
            setupLink: { url, app: 'codex', expiresAt: '2026-09-04T07:30:00Z' },
          })
        }
        return jsonResponse({ connections: [connection] })
      }),
    )

    renderSettings()
    fireEvent.click(await screen.findByRole('button', { name: `${hostileName} actions` }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Set up in an app…' }))
    const dialog = await screen.findByRole('dialog', { name: `Set up ${hostileName} in an app` })
    expect(dialog).toBeVisible()
    fireEvent.change(within(dialog).getByLabelText('Set it up in'), { target: { value: 'codex' } })
    fireEvent.click(screen.getByRole('button', { name: 'Make setup message' }))
    const message = `Set yourself up as @hermes in Agent Workspace: fetch ${url} once and follow the steps it gives you. The link works once, within 30 minutes.`
    expect(await screen.findByText(message)).toBeVisible()
    expect(posts).toEqual([
      {
        path: `/api/v1/projects/${projectId}/external-agents/${connectionId}/setup-link`,
        body: { expectedVersion: 1, app: 'codex' },
      },
    ])
    fireEvent.click(screen.getByRole('button', { name: 'Copy message' }))
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith(message))
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByText(message)).not.toBeInTheDocument()
  })
})
