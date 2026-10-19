import { describe, expect, it } from 'vitest'

import { applyUpdate, groupBySubject, normalizePage, safeHref, snoozeChoices, type InboxNotification } from './api'

const base: InboxNotification = {
  id: 'n1',
  kind: 'mention',
  projectId: 'p1',
  projectName: 'Project',
  subject: { kind: 'conversation', id: 'c1', title: '#general' },
  title: 'Owner mentioned you in #general',
  preview: 'hi',
  actor: null,
  count: 1,
  read: false,
  archived: false,
  snoozedUntil: null,
  at: '2026-10-16T08:00:00.000000Z',
  href: '/projects/p1/channels/c1',
  launchHref: null,
  approval: null,
  batch: null,
}

describe('inbox model', () => {
  it('keeps hrefs in the app or on https', () => {
    expect(safeHref('/projects/p1', 'p1')).toBe('/projects/p1')
    expect(safeHref('https://github.com/o/r/pull/1', 'p1')).toBe('https://github.com/o/r/pull/1')
    expect(safeHref('//evil.example/x', 'p1')).toBe('/projects/p1')
    expect(safeHref('javascript:alert(1)', 'p1')).toBe('/projects/p1')
  })

  it('drops malformed rows and reads the page', () => {
    const page = normalizePage({
      notifications: [{ ...base, href: 'javascript:x' }, { id: 'broken' }],
      nextCursor: '1.x',
      unreadCount: 3,
    })
    expect(page.notifications).toHaveLength(1)
    expect(page.notifications[0].href).toBe('/projects/p1')
    expect(page.nextCursor).toBe('1.x')
    expect(page.unreadCount).toBe(3)
  })

  it('applies changes optimistically per view', () => {
    expect(applyUpdate(base, 'inbox', { action: 'read', ids: ['n1'] })?.read).toBe(true)
    // In Unread, a row you read stays (as read) so you can still act on it; "all" empties the view.
    expect(applyUpdate(base, 'unread', { action: 'read', ids: ['n1'] })?.read).toBe(true)
    expect(applyUpdate(base, 'unread', { action: 'read', all: true })).toBeNull()
    expect(applyUpdate(base, 'inbox', { action: 'archive', ids: ['n1'] })).toBeNull()
    expect(applyUpdate(base, 'inbox', { action: 'archive', ids: ['other'] })).toBe(base)
    expect(applyUpdate(base, 'inbox', { action: 'read', all: true })?.read).toBe(true)
    expect(applyUpdate(base, 'archived', { action: 'read', all: true })).toBe(base)
    expect(applyUpdate(base, 'inbox', { action: 'snooze', ids: ['n1'], snoozeUntil: 'x' })).toBeNull()
  })

  it('groups by subject in order', () => {
    const second = { ...base, id: 'n2', subject: { kind: 'work_item', id: 'w1', title: 'AW-1' } }
    const third = { ...base, id: 'n3' }
    const groups = groupBySubject([base, second, third])
    expect(groups.map((group) => group.items.map((item) => item.id))).toEqual([['n1', 'n3'], ['n2']])
  })

  it('offers snoozes in the future', () => {
    const now = new Date('2026-10-16T08:00:00')
    for (const choice of snoozeChoices(now)) expect(choice.until.getTime()).toBeGreaterThan(now.getTime())
    expect(snoozeChoices(now).find((choice) => choice.id === 'next-week')?.until.getDay()).toBe(1)
  })

  it('only offers "later today" while the day has some left, and the next morning after a late night', () => {
    expect(snoozeChoices(new Date('2026-10-16T23:00:00')).map((choice) => choice.id)).toEqual(['tomorrow', 'next-week'])
    const night = new Date('2026-10-16T01:00:00')
    const morning = snoozeChoices(night).find((choice) => choice.id === 'tomorrow')!
    expect(morning.until.getDate()).toBe(16)
    expect(morning.until.getHours()).toBe(9)
  })
})
