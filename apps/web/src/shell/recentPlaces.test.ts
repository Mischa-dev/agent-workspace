import { describe, expect, it } from 'vitest'

import { isRememberedPlace, placeIcon, readRecentPlaces, rememberPlace, withPlace } from './recentPlaces'
import { nextSidebarIndex } from './sidebarMove'
import { unreadInSidebar } from './markAllRead'
import { pauseChoices, pausedUntilLabel } from './notificationPause'
import { pullDistance } from './pullModel'
import type { SidebarChannel } from '../conversations/types'

describe('recent places', () => {
  it('remembers named places only, newest first, without duplicates', () => {
    expect(isRememberedPlace('/projects/p/channels/c', '#general · AWD', true)).toBe(true)
    // Still loading: the address names it "Channel" until the page says.
    expect(isRememberedPlace('/projects/p/channels/c', 'Channel · AWD', false)).toBe(false)
    expect(isRememberedPlace('/settings/account', 'Account settings', false)).toBe(false)
    expect(isRememberedPlace('/home', 'Home', false)).toBe(false)
    expect(isRememberedPlace('/projects/p/pages', 'Pages · AWD', false)).toBe(true)

    let places = withPlace([], { path: '/a', title: 'A', at: 1 })
    places = withPlace(places, { path: '/b', title: 'B', at: 2 })
    places = withPlace(places, { path: '/a', title: 'A again', at: 3 })
    expect(places.map((place) => place.title)).toEqual(['A again', 'B'])
    for (let index = 0; index < 20; index += 1) places = withPlace(places, { path: `/p${index}`, title: 'x', at: index })
    expect(places).toHaveLength(12)
  })

  it('keeps each person’s places apart and ignores junk in storage', () => {
    window.localStorage.clear()
    rememberPlace('me', { path: '/messages/1', title: 'Sam', at: 1 })
    expect(readRecentPlaces('me')).toEqual([{ path: '/messages/1', title: 'Sam', at: 1 }])
    expect(readRecentPlaces('someone-else')).toEqual([])
    window.localStorage.setItem('aw.recent-places.me', JSON.stringify([{ path: 'https://evil.example', title: 'x', at: 1 }, 'nope']))
    expect(readRecentPlaces('me')).toEqual([])
    expect(placeIcon('/projects/p/threads/t')).toBe('messages')
    expect(placeIcon('/projects/p/work-items/w')).toBe('checklist')
  })
})

describe('moving through the sidebar', () => {
  const rows = [
    { current: false, unread: true },
    { current: true, unread: false },
    { current: false, unread: false },
    { current: false, unread: true },
  ]

  it('steps one row at a time and stops at the ends', () => {
    expect(nextSidebarIndex(rows, 1, false)).toBe(2)
    expect(nextSidebarIndex(rows, -1, false)).toBe(0)
    expect(nextSidebarIndex([{ current: true, unread: false }], 1, false)).toBeNull()
    expect(nextSidebarIndex(rows.map((row) => ({ ...row, current: false })), -1, false)).toBe(3)
  })

  it('jumps to the next unread row either way, wrapping around', () => {
    expect(nextSidebarIndex(rows, 1, true)).toBe(3)
    expect(nextSidebarIndex(rows, -1, true)).toBe(0)
    expect(nextSidebarIndex([rows[0], rows[1]], 1, true)).toBe(0)
    expect(nextSidebarIndex([{ current: true, unread: true }], 1, true)).toBeNull()
  })
})

describe('mark all read', () => {
  it('finds unread channels and threads by sequence', () => {
    const thread = (id: string, highest: number, seen: number) => ({
      id,
      parentChannelId: 'c1',
      originMessageId: null,
      name: id,
      archivedAt: null,
      parentArchivedAt: null,
      highestSequence: highest,
      highestSeenSequence: seen,
      attention: 'settled' as const,
      attentionLabel: '',
    })
    const channels: SidebarChannel[] = [
      { id: 'c1', name: 'general', archivedAt: null, highestSequence: 5, highestSeenSequence: 5, unread: false, threads: [thread('t1', 9, 3), thread('t2', 2, 2)], hasMoreThreads: false },
      { id: 'c2', name: 'random', archivedAt: null, highestSequence: 7, highestSeenSequence: 1, unread: true, threads: [], hasMoreThreads: false },
    ]
    expect(unreadInSidebar(channels)).toEqual([
      { conversationId: 't1', highestSequence: 9 },
      { conversationId: 'c2', highestSequence: 7 },
    ])
  })
})

describe('pausing notifications', () => {
  it('offers an hour, four hours and the next morning', () => {
    const afternoon = new Date(2026, 9, 17, 14, 0)
    const [hour, four, morning] = pauseChoices(afternoon)
    expect(hour.until.getTime() - afternoon.getTime()).toBe(60 * 60_000)
    expect(four.until.getHours()).toBe(18)
    expect(morning.label).toBe('Until tomorrow morning')
    expect(morning.until).toEqual(new Date(2026, 9, 18, 9, 0))
    // At 2 a.m. the next morning is today.
    expect(pauseChoices(new Date(2026, 9, 17, 2, 0))[2].until).toEqual(new Date(2026, 9, 17, 9, 0))
    expect(pausedUntilLabel(morning.until, afternoon)).toMatch(/^tomorrow /)
  })
})

describe('pull to refresh', () => {
  it('follows the finger at half speed, up to a limit', () => {
    expect(pullDistance(-20)).toBe(0)
    expect(pullDistance(100)).toBe(50)
    expect(pullDistance(1_000)).toBe(110)
  })
})
