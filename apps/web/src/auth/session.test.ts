import { renderHook } from '@testing-library/react'
import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { openCommandMenu, useCommandState } from '../shell/commandStore'
import { showToast, useToasts } from '../ui/toastStore'
import {
  clearAuthenticatedQueryCache,
  getSession,
  installAuthenticatedSession,
  isAuthenticatedSession,
  isTwoFactorChallenge,
  login,
  normalizeLoginResponse,
  normalizeSessionResponse,
  type AuthenticatedSession,
} from './session'

const userId = '019fb600-0000-7000-8000-000000000010'
const actorId = '019fb600-0000-7000-8000-000000000011'
const organizationId = '019fb600-0000-7000-8000-000000000012'

const authenticatedSession: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: userId,
    actorId,
    actorKind: 'human',
    loginName: 'new-actor',
    displayName: 'New Actor',
    capabilities: { createProject: false },
  },
  organizations: [],
}

function sessionWire(capabilities: unknown = { createProject: true }) {
  return {
    authenticated: true,
    user: {
      id: userId,
      actorId,
      actorKind: 'human',
      loginName: 'new-actor',
      displayName: 'New Actor',
      capabilities,
    },
    organizations: [
      {
        id: organizationId,
        name: 'Personal Organization',
        slug: 'personal-organization',
        role: 'owner',
      },
    ],
  }
}

function seedPrivateCaches(queryClient: QueryClient) {
  queryClient.setQueryData(queryKeys.projects, {
    projects: [{ id: 'previous-actor-project' }],
  })
  queryClient.setQueryData(queryKeys.project('previous-actor-project'), {
    id: 'previous-actor-project',
  })
  queryClient.setQueryData(
    queryKeys.timeline('previous-actor-project', 'previous-actor-thread'),
    {
    entries: [{ id: 'private-message' }],
    },
  )
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('session wire normalization', () => {
  it.each([
    ['missing', {}],
    ['string', { createProject: 'true' }],
    ['number', { createProject: 1 }],
    ['null', { createProject: null }],
  ])('fails the %s Project-creation grant closed', (_, capabilities) => {
    const session = normalizeSessionResponse(sessionWire(capabilities))

    expect(session.authenticated).toBe(true)
    expect(session.user?.capabilities.createProject).toBe(false)
  })

  it('accepts only well-shaped Organizations with known roles', () => {
    const wire = sessionWire()
    wire.organizations = [
      wire.organizations[0],
      {
        id: 'not-a-uuid',
        name: 'Bad ID',
        slug: 'bad-id',
        role: 'owner',
      },
      {
        id: '019fb600-0000-7000-8000-000000000013',
        name: 'Bad\u0000Name',
        slug: 'bad-name',
        role: 'admin',
      },
      {
        id: '019fb600-0000-7000-8000-000000000014',
        name: 'Bad Slug',
        slug: '../bad',
        role: 'admin',
      },
      {
        id: '019fb600-0000-7000-8000-000000000015',
        name: 'Unknown Role',
        slug: 'unknown-role',
        role: 'viewer',
      },
      {
        id: '019FB600-0000-7000-8000-000000000016',
        name: '  Admin Organization  ',
        slug: 'admin-organization',
        role: 'admin',
      },
    ]

    expect(normalizeSessionResponse(wire).organizations).toEqual([
      wire.organizations[0],
      {
        id: '019fb600-0000-7000-8000-000000000016',
        name: 'Admin Organization',
        slug: 'admin-organization',
        role: 'admin',
      },
    ])
  })

  it('applies the same normalizer to session reads and login responses', async () => {
    const malformedGrant = sessionWire({ createProject: 'true' })
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify(malformedGrant), {
          headers: { 'content-type': 'application/json' },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify(malformedGrant), {
          headers: { 'content-type': 'application/json' },
        }),
      )
    vi.stubGlobal('fetch', fetchMock)

    await expect(getSession()).resolves.toMatchObject({
      authenticated: true,
      user: { capabilities: { createProject: false } },
    })
    await expect(login('new-actor', 'correct horse battery staple')).resolves
      .toMatchObject({
        authenticated: true,
        user: { capabilities: { createProject: false } },
      })
  })
})

describe('two-factor login responses', () => {
  it('never treats a pending second step as signed in, even if the response also claims a user', () => {
    const pending = normalizeLoginResponse({ authenticated: false, twoFactorRequired: true, challenge: 'opaque' })
    expect(isAuthenticatedSession(pending)).toBe(false)
    expect(isTwoFactorChallenge(pending)).toBe(true)
    expect(pending).toMatchObject({ user: null, organizations: [], challenge: 'opaque' })

    const confused = normalizeLoginResponse({ ...sessionWire(), twoFactorRequired: true, challenge: 'opaque' })
    expect(isAuthenticatedSession(confused)).toBe(false)
    expect(isTwoFactorChallenge(confused)).toBe(true)
  })

  it('treats a challenge without a usable value as signed out', () => {
    const broken = normalizeLoginResponse({ authenticated: false, twoFactorRequired: true, challenge: '' })
    expect(isAuthenticatedSession(broken)).toBe(false)
    expect(isTwoFactorChallenge(broken)).toBe(false)
  })
})

describe('authenticated query-cache boundary', () => {
  it('removes every Actor-scoped cache on automatic sign-out', () => {
    const queryClient = new QueryClient()
    seedPrivateCaches(queryClient)

    clearAuthenticatedQueryCache(queryClient)

    expect(queryClient.getQueryData(queryKeys.projects)).toBeUndefined()
    expect(
      queryClient.getQueryData(
        queryKeys.project('previous-actor-project'),
      ),
    ).toBeUndefined()
    expect(
      queryClient.getQueryData(
        queryKeys.timeline(
          'previous-actor-project',
          'previous-actor-thread',
        ),
      ),
    ).toBeUndefined()
    expect(queryClient.getQueryData(queryKeys.session)).toEqual({
      authenticated: false,
      user: null,
      organizations: [],
    })
  })

  it('drops toasts that could name a private conversation or undo into it, and closes the command menu', () => {
    const queryClient = new QueryClient()
    showToast({ title: 'Archived #private-plans', onUndo: async () => undefined })
    openCommandMenu('private-plans')

    clearAuthenticatedQueryCache(queryClient)

    const { result } = renderHook(() => useToasts())
    expect(result.current).toEqual([])
    expect(renderHook(() => useCommandState()).result.current.open).toBeNull()
  })

  it('clears stale Actor data before installing a successful login', () => {
    const queryClient = new QueryClient()
    seedPrivateCaches(queryClient)

    installAuthenticatedSession(queryClient, authenticatedSession)

    expect(queryClient.getQueryData(queryKeys.projects)).toBeUndefined()
    expect(
      queryClient.getQueryData(
        queryKeys.timeline(
          'previous-actor-project',
          'previous-actor-thread',
        ),
      ),
    ).toBeUndefined()
    expect(queryClient.getQueryData(queryKeys.session)).toEqual(
      authenticatedSession,
    )
  })
})
