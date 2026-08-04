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

import { TeamSettings } from './TeamSettings'

const projectId = '019fb800-0000-7000-8000-000000000001'
const actorId = '019fb800-0000-7000-8000-000000000002'
const invitationId = '019fb800-0000-7000-8000-000000000003'
const invitationUrl = `http://127.0.0.1:3100/join#token=aw_inv_${'x'.repeat(43)}`

const member = {
  id: '019fb800-0000-7000-8000-000000000004',
  projectId,
  actorId,
  role: 'member',
  displayName: '<img src=x onerror=alert(1)>',
  handle: 'second_human',
  enabled: true,
  version: 1,
  invitedBy: null,
  acceptedInvitationId: null,
  acceptedAt: null,
  disabledByActorId: null,
  disabledAt: null,
  createdAt: '2026-08-01T20:00:00Z',
  executionProfileReadiness: 'not_configured',
  capabilities: { changeRole: true, disable: true, enable: false },
}
const invitation = {
  id: invitationId,
  projectId,
  role: 'member',
  tokenPrefix: 'aw_inv_xxxxxxxxx',
  status: 'active',
  version: 1,
  issuedByActorId: '019fb800-0000-7000-8000-000000000005',
  issuedByDisplayName: 'Project Owner',
  expiresAt: '2026-08-02T20:00:00Z',
  acceptedByActorId: null,
  acceptedAt: null,
  revokedAt: null,
  createdAt: '2026-08-01T20:00:00Z',
}

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }),
  )
}

function renderTeam(organizationRole = 'owner') {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
        })
      }
    >
      <TeamSettings organizationRole={organizationRole} projectId={projectId} />
    </QueryClientProvider>,
  )
}

describe('TeamSettings', () => {
  beforeEach(() => {
    document.cookie = 'agent_workspace_csrf=csrf-team; Path=/'
    vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(
      '019fb800-0000-7000-8000-000000000006',
    )
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    document.cookie = 'agent_workspace_csrf=; Max-Age=0; Path=/'
  })

  it('renders hostile identity as text and applies an optimistic role update', async () => {
    const requests: Array<{ path: string; init?: RequestInit }> = []
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        requests.push({ path, init })
        if (path.endsWith('/members') && (init?.method ?? 'GET') === 'GET') {
          return jsonResponse({ members: [member] })
        }
        if (path.endsWith('/invitations') && (init?.method ?? 'GET') === 'GET') {
          return jsonResponse([invitation])
        }
        if (path.endsWith(`/members/${actorId}`)) {
          return jsonResponse({ ...member, role: 'viewer', version: 2 })
        }
        throw new Error(`Unexpected request: ${path}`)
      }),
    )

    renderTeam()
    expect(await screen.findByText(member.displayName)).toBeVisible()
    expect(document.querySelector('img')).toBeNull()
    fireEvent.change(screen.getByLabelText(`Role for ${member.displayName}`), {
      target: { value: 'viewer' },
    })

    await waitFor(() => {
      const update = requests.find(({ path }) => path.endsWith(`/members/${actorId}`))
      expect(JSON.parse(String(update?.init?.body))).toEqual({
        expectedVersion: 1,
        role: 'viewer',
      })
      const headers = new Headers(update?.init?.headers)
      expect(headers.get('idempotency-key')).toBe(
        '019fb800-0000-7000-8000-000000000006',
      )
      expect(headers.get('x-csrf-token')).toBe('csrf-team')
    })
  })

  it('shows a newly issued link once and never places it in a request URL', async () => {
    const paths: string[] = []
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        paths.push(path)
        if (path.endsWith('/members')) return jsonResponse({ members: [member] })
        if (path.endsWith('/invitations') && init?.method === 'POST') {
          return jsonResponse({ ...invitation, invitationUrl })
        }
        if (path.endsWith('/invitations')) return jsonResponse([invitation])
        throw new Error(`Unexpected request: ${path}`)
      }),
    )

    renderTeam()
    await screen.findByText('Invitation history')
    fireEvent.click(screen.getByRole('button', { name: 'Create invitation link' }))
    expect(await screen.findByText(invitationUrl)).toBeVisible()
    expect(paths.every((path) => !path.includes('aw_inv_'))).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByText(invitationUrl)).not.toBeInTheDocument()
  })

  it('does not offer admin invitations to an Organization admin', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) =>
        String(input).endsWith('/members')
          ? jsonResponse({ members: [member] })
          : jsonResponse([invitation]),
      ),
    )
    renderTeam('admin')
    await screen.findByText('Invitation history')
    expect(
      within(screen.getByLabelText('Invitation role')).queryByRole('option', {
        name: 'Admin',
      }),
    ).not.toBeInTheDocument()
  })
})
