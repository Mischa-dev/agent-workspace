import { createElement } from 'react'
import { act, render, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider, dehydrate } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { settingsKeys, type EffectiveSettings } from '../settings/api'
import OfflineReading from './OfflineReading'
import { offlineOwner, offlineReadingOn } from './offlinePointer'
import {
  OFFLINE_CAPS,
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

    const picked = pickForOffline(dehydrate(cache))
    const timelines = picked.queries.filter((query) => offlineClass(query.queryKey) === 'conversation')
    expect(timelines).toHaveLength(OFFLINE_CAPS.conversations)
    expect(timelines.some((query) => query.queryKey[3] === 'c0')).toBe(false)
    const inbox = picked.queries.find((query) => query.queryKey[0] === 'inbox')?.state.data as { pages: unknown[] }
    expect(inbox.pages).toEqual([{ items: [1] }])
    const kept = picked.queries.find((query) => query.queryKey[3] === 'long')?.state.data as { messages: { id: number }[] }
    expect(kept.messages).toHaveLength(OFFLINE_CAPS.messages)
    expect(kept.messages.at(-1)).toEqual({ id: OFFLINE_CAPS.messages + 19 })
    expect(picked.queries.some((query) => query.queryKey[0] === 'account')).toBe(false)
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

  it('stops keeping and clears when the workspace switches it off', async () => {
    const cache = client()
    const target = { scope: 'workspace' as const, organizationId: 'ws-1' }
    cache.setQueryData(settingsKeys.effective(target), settings({ 'offline.reading': true }))
    render(
      createElement(QueryClientProvider, { client: cache }, createElement(OfflineReading, { actorId: 'alice', workspaceId: 'ws-1' })),
    )
    await waitFor(() => expect(offlineOwner()).toBe('alice'))

    await act(async () => {
      cache.setQueryData(settingsKeys.effective(target), settings({ 'offline.reading': true, 'offline.allowed': false }))
    })
    await waitFor(() => expect(offlineOwner()).toBeNull())
    expect(await restoreOffline(client())).toBe(false)
  })
})
