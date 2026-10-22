import { describe, expect, it } from 'vitest'

import { filterCommands, type CommandItem } from '../ui'
import { pageTitleFor, unreadCount } from './documentTitleModel'
import { SHORTCUTS } from './shortcutSheet'
import { createShortcutMatcher, parseTaskKey } from './shortcuts'

function key(value: string, extra: Partial<KeyboardEvent> = {}) {
  return { key: value, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false, target: document.body, ...extra }
}

describe('keyboard shortcuts', () => {
  it('runs single keys and G sequences outside text fields only', () => {
    let now = 0
    const press = createShortcutMatcher(() => now)
    expect(press(key('k', { metaKey: true }))).toBe('command-menu')
    expect(press(key('c'))).toBe('create-task')
    expect(press(key('?', { shiftKey: true }))).toBe('shortcut-sheet')
    expect(press(key('g'))).toBeNull()
    expect(press(key('i'))).toBe('go-inbox')
    expect(press(key('g'))).toBeNull()
    now = 5_000
    expect(press(key('t'))).toBeNull()

    const field = document.createElement('input')
    document.body.appendChild(field)
    expect(press(key('c', { target: field }))).toBeNull()
    // ⌘K still opens the menu while typing.
    expect(press(key('k', { ctrlKey: true, target: field }))).toBe('command-menu')
    field.remove()
  })

  it('moves between conversations and marks all read, also from the composer', () => {
    const press = createShortcutMatcher()
    const wrap = document.createElement('div')
    wrap.className = 'aw-composer-wrap'
    const composer = document.createElement('textarea')
    wrap.appendChild(composer)
    document.body.appendChild(wrap)
    expect(press(key('ArrowDown', { altKey: true, target: composer }))).toBe('next-conversation')
    expect(press(key('ArrowUp', { altKey: true }))).toBe('previous-conversation')
    expect(press(key('ArrowDown', { altKey: true, shiftKey: true }))).toBe('next-unread')
    expect(press(key('ArrowUp', { altKey: true, shiftKey: true }))).toBe('previous-unread')
    expect(press(key('Escape', { shiftKey: true, target: composer }))).toBe('mark-all-read')
    expect(press(key('Escape'))).toBeNull()
    expect(press(key('ArrowDown'))).toBeNull()
    wrap.remove()
    // Any other field keeps ⌥↑ and ⌥↓ for moving the cursor, so a page being edited stays put.
    const editor = document.createElement('textarea')
    document.body.appendChild(editor)
    expect(press(key('ArrowDown', { altKey: true, target: editor }))).toBeNull()
    expect(press(key('Escape', { shiftKey: true, target: editor }))).toBeNull()
    editor.remove()
    // A menu or dialog keeps its own arrows.
    const menu = document.createElement('div')
    menu.setAttribute('role', 'menu')
    const item = document.createElement('button')
    menu.appendChild(item)
    document.body.appendChild(menu)
    expect(press(key('ArrowDown', { altKey: true, target: item }))).toBeNull()
    menu.remove()
  })

  it('does what the shortcut sheet says for every key it lists', () => {
    type Modifiers = { metaKey?: boolean; altKey?: boolean; shiftKey?: boolean }
    const named: Record<string, Modifiers> = { '⌘': { metaKey: true }, '⌥': { altKey: true }, '⇧': { shiftKey: true } }
    const keyNames: Record<string, string> = { '↑': 'ArrowUp', '↓': 'ArrowDown', Esc: 'Escape' }
    for (const shortcut of SHORTCUTS) {
      const press = createShortcutMatcher()
      const modifiers = shortcut.keys.filter((part) => part in named)
      const rest = shortcut.keys.filter((part) => !(part in named)).map((part) => keyNames[part] ?? part.toLowerCase())
      const extra: Modifiers = Object.assign({}, ...modifiers.map((part) => named[part]))
      if (rest[0] === '?') extra.shiftKey = true
      let action = null
      for (const part of rest) action = press(key(part, extra))
      expect(action, shortcut.keys.join(' ')).toBe(shortcut.action)
    }
  })

  it('reads task keys in any case and rejects look-alikes', () => {
    expect(parseTaskKey('awd-12')).toEqual({ projectKey: 'AWD', number: 12, key: 'AWD-12' })
    expect(parseTaskKey('AWD-0')).toBeNull()
    expect(parseTaskKey('12-AWD')).toBeNull()
    expect(parseTaskKey('settings')).toBeNull()
  })
})

describe('command menu filtering', () => {
  const item = (id: string, label: string, extra: Partial<CommandItem> = {}): CommandItem => ({
    id,
    label,
    group: 'Go to',
    onSelect: () => undefined,
    ...extra,
  })

  it('puts prefix matches first and keeps server results', () => {
    const items = [item('a', 'Project settings'), item('b', 'Settings', { keywords: ['preferences'] }), item('c', 'Home'), item('d', 'Fix login', { matched: true })]
    expect(filterCommands(items, 'set').map((candidate) => candidate.id)).toEqual(['b', 'a', 'd'])
    expect(filterCommands(items, 'prefer').map((candidate) => candidate.id)).toEqual(['b', 'd'])
    expect(filterCommands(items, '')).toHaveLength(4)
  })
})

describe('document title', () => {
  const projects = [{ id: 'p1', name: 'Agent Workspace', attentionCount: 2, archivedAt: null }, { id: 'p2', name: 'Old', attentionCount: 5, archivedAt: '2026-01-01' }]

  it('counts the inbox’s unread and the conversations with unread messages', () => {
    expect(unreadCount(2, [{ unread: true }, { unread: false }])).toBe(3)
  })

  it('names the page and its project', () => {
    expect(pageTitleFor('/projects/p1/work-items', '?view=board', projects, null)).toBe('Board · Agent Workspace')
    expect(pageTitleFor('/projects/p1/channels/c1', '', projects, '#general')).toBe('#general · Agent Workspace')
    expect(pageTitleFor('/projects/p1/next', '', projects, null)).toBe('Planning · Agent Workspace')
    expect(pageTitleFor('/messages', '', projects, null)).toBe('Messages')
    expect(pageTitleFor('/settings/account/appearance', '', projects, null)).toBe('Appearance')
    expect(pageTitleFor('/settings/workspace/members', '', projects, null)).toBe('Settings')
    expect(pageTitleFor('/design-system', '', projects, null)).toBe('Design system')
  })
})
