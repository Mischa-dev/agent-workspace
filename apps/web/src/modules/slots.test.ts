import { describe, expect, it } from 'vitest'

import appSource from '../app/App.tsx?raw'
import paletteSource from '../shell/CommandPalette.tsx?raw'
import homeSource from '../projects/HomePage.tsx?raw'
import homeSidebarSource from '../shell/HomeSidebar.tsx?raw'
import railSource from '../shell/ProjectRail.tsx?raw'
import { SLASH_COMMANDS } from '../later/slashCommands'
import { SETTINGS_PAGES } from '../settings/registry'
import { SHORTCUTS } from '../shell/shortcutSheet'
import { MODULE_SLOTS, moduleOf, moduleOfPath, patternScore, type SlotKind } from './slots'

// AWD-75's guard: every route, palette entry, shortcut, slash command,
// settings page, Home section and sidebar row names its module, by being
// claimed in exactly one manifest (`modules/<id>.json`, under `slots`). The
// server's guard (apps/server/src/modules/mod.rs) covers its routes, tools,
// layout parts, settings, setup steps, events and notifications.

function claims(kind: SlotKind) {
  const found = new Map<string, string[]>()
  for (const [module, slots] of Object.entries(MODULE_SLOTS)) {
    for (const value of slots[kind] ?? []) found.set(value, [...(found.get(value) ?? []), module])
  }
  return found
}

/** Every value claimed by a module, once each, and no claim that matches nothing. */
function expectNamed(kind: SlotKind, values: string[]) {
  const unnamed = values.filter((value) => moduleOf(kind, value) === null)
  expect(unnamed, `${kind} without a module: add each to the manifest of the module it belongs to`).toEqual([])
  const twice = [...claims(kind)].filter(([, modules]) => modules.length > 1)
  expect(twice, `${kind} claimed twice`).toEqual([])
  const stale = [...claims(kind).keys()].filter((pattern) => !values.some((value) => patternScore(pattern, value) !== null))
  expect(stale, `${kind} in manifests that don't exist`).toEqual([])
}

describe('every entry point names its module', () => {
  it('loads every core manifest', () => {
    expect(Object.keys(MODULE_SLOTS).sort()).toEqual(
      ['agents', 'automations', 'channels', 'code', 'collections', 'inbox', 'messages', 'pages', 'projects', 'runs', 'sheets', 'suggestions', 'tasks', 'whiteboards', 'workspace'],
    )
  })

  it('routes', () => {
    const paths = [...appSource.matchAll(/\bpath="([^"]+)"/g)].map((match) => match[1])
    expect(paths.length).toBeGreaterThan(30)
    // Pages name routes exactly as App.tsx writes them.
    const pages = claims('pages')
    expect(paths.filter((path) => !pages.has(path)), 'routes without a module').toEqual([])
    expectNamed('pages', paths)
    expect(moduleOfPath('/projects/p1/work-items/t1')).toBe('tasks')
    expect(moduleOfPath('/projects/p1/pages')).toBe('pages')
    expect(moduleOfPath('/whiteboards/w1')).toBe('whiteboards')
    expect(moduleOfPath('/AWD-12')).toBe('tasks')
    expect(moduleOfPath('/settings/workspace/modules')).toBe('workspace')
  })

  it('palette entries', () => {
    const fixed = [...paletteSource.matchAll(/\bid: '([^']+)'/g)].map((match) => match[1])
    // Entries made per project, channel, task and so on: their prefix, with something after it.
    const made = [...paletteSource.matchAll(/\bid: `([a-z-]+:(?:[a-z-]+:)?)\$\{/g)].map((match) => `${match[1]}x`)
    expect(fixed.length).toBeGreaterThan(30)
    const ids = [...new Set([...fixed, ...made])].filter((id) => !id.startsWith('slash:'))
    expectNamed('palette', ids)
  })

  it('keyboard shortcuts', () => {
    expectNamed('shortcuts', SHORTCUTS.map((shortcut) => shortcut.action))
  })

  it('slash commands', () => {
    expectNamed('slash', SLASH_COMMANDS.map((command) => command.name))
  })

  it('settings pages', () => {
    expectNamed('settings', SETTINGS_PAGES.map((page) => `${page.group}/${page.id}`))
  })

  it('Home sections', () => {
    const sections = ['needsYou', 'working', 'capacity', 'tasks', 'setup', 'recent']
    expectNamed('home', sections)
    for (const section of ['needsYou', 'recent']) expect(homeSource).toContain(`homeOn('${section}')`)
  })

  it('rail and Home sidebar rows that are not layout parts', () => {
    expectNamed('sidebar', ['rail.home', 'rail.messages', 'rail.projects', 'rail.search', 'rail.settings', 'home.home', 'home.inbox', 'project.overview'])
    // Rows that a module can turn off check it where they're drawn.
    expect(homeSidebarSource).toMatch(/moduleOn\('inbox'\) \? <SidebarLink[^\n]*label="Inbox"/)
    expect(railSource).toMatch(/moduleOn\('messages'\) \? \(\s*<UtilityTile[^/]*label="Messages"/)
    expect(railSource).toContain("experience.moduleOn('projects')")
    // Every other Home sidebar row is a layout part, which follows modules on the server.
    const lines = homeSidebarSource.split('\n')
    lines.forEach((line, index) => {
      if (!line.includes('<SidebarLink')) return
      // The check sits on the row's line or the one before it.
      const near = `${lines[index - 1] ?? ''}\n${line}`
      expect(near.includes('shows(') || near.includes('moduleOn(') || line.includes('label="Home"'), line).toBe(true)
    })
  })
})
