import { createElement } from 'react'
import { act, render, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { settingsKeys, type EffectiveSettings } from '../settings/api'
import OfflineReading from './OfflineReading'
import { offlineOwner, offlineReadingOn } from './offlinePointer'
import {
  OFFLINE_CAPS,
  cachedForOffline,
  clearOffline,
  keepOffline,
  offlineClass,
  pickForOffline,
  resetOfflineForTests,
  restoreOffline,
  saveOffline,
} from './persist'
import { things } from './things'

const client = () => new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } })

function settings(values: Record<string, unknown>): EffectiveSettings {
  return {
    organizationId: 'ws-1',
    projectId: null,
    settings: Object.entries(values).map(([key, value]) => ({
      key,
      value,
      source: 'user',
      overrides: null,
      fallbackValue: null,
      layers: [],
      default: null,
    })),
    capabilities: { manageWorkspace: false, manageProject: false, manageOwn: true },
  }
}

beforeEach(() => {
  resetOfflineForTests()
  things.clear()
  // Nothing here reaches a server.
  vi.stubGlobal('fetch', vi.fn(() => new Promise(() => {})))
})

afterEach(() => vi.unstubAllGlobals())

describe('reading offline', () => {
  it('is off unless the person turns it on, and the workspace can switch it off', () => {
    expect(offlineReadingOn(settings({}))).toBe(false)
    expect(offlineReadingOn(settings({ 'offline.reading': true }))).toBe(true)
    expect(offlineReadingOn(settings({ 'offline.reading': true, 'offline.allowed': false }))).toBe(false)
    expect(offlineOwner()).toBeNull()
  })

  it('keeps what pages show and never settings, tokens or search', () => {
    expect(offlineClass(queryKeys.session)).toBe('shell')
    expect(offlineClass(queryKeys.timeline('p', 'c'))).toBe('conversation')
    expect(offlineClass(queryKeys.workItem('p', 't'))).toBe('page')
    expect(offlineClass(['project', 'p', 'memory', 'page', 'x'])).toBe('page')
    expect(offlineClass(['inbox', 'list', 'all', 'all', ''])).toBe('list')
    expect(offlineClass(queryKeys.search('q', undefined))).toBeNull()
    expect(offlineClass(['organization', 'w', 'settings', 'effective'])).toBe('shell')
    expect(offlineClass(['organization', 'w', 'members'])).toBeNull()
    expect(offlineClass(['account', 'tokens'])).toBeNull()
    expect(offlineClass(queryKeys.providerAccounts('p'))).toBeNull()
  })

  it('keeps at most its caps: the latest conversations, a list’s first page, a conversation’s latest messages', () => {
    const cache = client()
    for (let index = 0; index < OFFLINE_CAPS.conversations + 5; index += 1) {
      cache.setQueryData(queryKeys.timeline('p', `c${index}`), { messages: [] }, { updatedAt: 1000 + index })
    }
    cache.setQueryData(['inbox', 'list'], { pages: [{ items: [1] }, { items: [2] }], pageParams: [null, 'next'] }, { updatedAt: 5000 })
    const long = Array.from({ length: OFFLINE_CAPS.messages + 20 }, (_, index) => ({ id: index }))
    cache.setQueryData(queryKeys.timeline('p', 'long'), { messages: long }, { updatedAt: 6000 })
    cache.setQueryData(['account', 'tokens'], { tokens: ['secret'] })

    const picked = pickForOffline(cachedForOffline(cache))
    const timelines = picked.filter((query) => offlineClass(query.queryKey) === 'conversation')
    expect(timelines).toHaveLength(OFFLINE_CAPS.conversations)
    expect(timelines.some((query) => query.queryKey[3] === 'c0')).toBe(false)
    const inbox = picked.find((query) => query.queryKey[0] === 'inbox')?.data as { pages: unknown[] }
    expect(inbox.pages).toEqual([{ items: [1] }])
    const kept = picked.find((query) => query.queryKey[3] === 'long')?.data as { messages: { id: number }[] }
    expect(kept.messages).toHaveLength(OFFLINE_CAPS.messages)
    expect(kept.messages.at(-1)).toEqual({ id: OFFLINE_CAPS.messages + 19 })
    expect(picked.some((query) => query.queryKey[0] === 'account')).toBe(false)
  })

  it('keeps the session however many newer shell queries there are', () => {
    const cache = client()
    cache.setQueryData(queryKeys.session, { authenticated: true }, { updatedAt: 1 })
    for (let index = 0; index < 100; index += 1) cache.setQueryData(['statuses', `s${index}`], {}, { updatedAt: 1000 + index })
    const picked = pickForOffline(cachedForOffline(cache))
    expect(picked.some((query) => query.queryKey[0] === 'session')).toBe(true)
  })

  it('shows an account’s copy at start, to that account only, and forgets it at sign-out', async () => {
    const reading = client()
    reading.setQueryData(queryKeys.timeline('p', 'c'), { messages: [{ id: 'm1' }] })
    things.put({ kind: 'task', id: 't1', version: 2, title: 'Kept' })
    const stop = keepOffline(reading, 'alice', 10_000)
    await saveOffline(reading, 'alice')
    stop()

    things.clear()
    const restarted = client()
    expect(await restoreOffline(restarted)).toBe(true)
    expect(restarted.getQueryData(queryKeys.timeline('p', 'c'))).toEqual({ messages: [{ id: 'm1' }] })
    // Shown, and asked for again however recently it was kept.
    expect(restarted.getQueryState(queryKeys.timeline('p', 'c'))?.isInvalidated).toBe(true)
    expect(things.get('task', 't1')?.title).toBe('Kept')

    // Bob signs in on the same device: Alice's copy goes, and nothing of hers is shown to him.
    const stopBob = keepOffline(client(), 'bob', 10_000)
    stopBob()
    expect(offlineOwner()).toBe('bob')
    const forBob = client()
    await restoreOffline(forBob)
    expect(forBob.getQueryData(queryKeys.timeline('p', 'c'))).toBeUndefined()

    await clearOffline()
    expect(offlineOwner()).toBeNull()
    expect(await restoreOffline(client())).toBe(false)
  })

  it('shows nothing from a copy cleared while it was being read', async () => {
    const reading = client()
    reading.setQueryData(queryKeys.timeline('p', 'c'), { messages: [{ id: 'm1' }] })
    const stop = keepOffline(reading, 'alice', 10_000)
    await saveOffline(reading, 'alice')
    stop()

    const restarted = client()
    const restoring = restoreOffline(restarted)
    const clearing = clearOffline()
    expect(await restoring).toBe(false)
    await clearing
    expect(restarted.getQueryData(queryKeys.timeline('p', 'c'))).toBeUndefined()
  })

  it('stops keeping and clears when the workspace switches it off', async () => {
    const cache = client()
    const target = { scope: 'workspace' as const, organizationId: 'ws-1' }
    cache.setQueryData(settingsKeys.effective(target), settings({ 'offline.reading': true }))
    render(
      createElement(QueryClientProvider, { client: cache }, createElement(OfflineReading, { actorId: 'alice', workspaceId: 'ws-1', workspaces: [{ id: 'ws-1', name: 'One' }] })),
    )
    await waitFor(() => expect(offlineOwner()).toBe('alice'))

    await act(async () => {
      cache.setQueryData(settingsKeys.effective(target), settings({ 'offline.reading': true, 'offline.allowed': false }))
    })
    await waitFor(() => expect(offlineOwner()).toBeNull())
    expect(await restoreOffline(client())).toBe(false)
  })

  it('keeps nothing while another workspace the person is in doesn’t allow it', async () => {
    const cache = client()
    const mine = { scope: 'workspace' as const, organizationId: 'ws-1' }
    const theirs = { scope: 'workspace' as const, organizationId: 'ws-2' }
    cache.setQueryData(settingsKeys.effective(mine), settings({ 'offline.reading': true }))
    cache.setQueryData(settingsKeys.effective(theirs), settings({}))
    const workspaces = [
      { id: 'ws-1', name: 'One' },
      { id: 'ws-2', name: 'Two' },
    ]
    render(createElement(QueryClientProvider, { client: cache }, createElement(OfflineReading, { actorId: 'alice', workspaceId: 'ws-1', workspaces })))
    await waitFor(() => expect(offlineOwner()).toBe('alice'))

    await act(async () => {
      cache.setQueryData(settingsKeys.effective(theirs), settings({ 'offline.allowed': false }))
    })
    await waitFor(() => expect(offlineOwner()).toBeNull())
    expect(await restoreOffline(client())).toBe(false)
  })
})
