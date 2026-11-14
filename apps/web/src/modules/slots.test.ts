import { describe, expect, it } from 'vitest'

import paletteSource from '../shell/CommandPalette.tsx?raw'
import homeSource from '../projects/HomePage.tsx?raw'
import homeSidebarSource from '../shell/HomeSidebar.tsx?raw'
import railSource from '../shell/ProjectRail.tsx?raw'
import { routePaths } from '../app/routes'
import { SLASH_COMMANDS } from '../later/slashCommands'
import { SETTINGS_PAGES } from '../settings/registry'
import { SHORTCUTS } from '../shell/shortcutSheet'
import { sidebarRows, WEB_MODULES } from './registry'
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
    const paths = routePaths()
    expect(paths.length).toBeGreaterThan(30)
    // Pages name routes exactly as the route table (app/routes.tsx) writes them.
    const pages = claims('pages')
    expect(paths.filter((path) => !pages.has(path)), 'routes without a module').toEqual([])
    expectNamed('pages', paths)
    expect(moduleOfPath('/projects/p1/work-items/t1')).toBe('tasks')
    expect(moduleOfPath('/projects/p1/pages')).toBe('pages')
    expect(moduleOfPath('/whiteboards/w1')).toBe('whiteboards')
    expect(moduleOfPath('/AWD-12')).toBe('tasks')
    expect(moduleOfPath('/settings/workspace/modules')).toBe('workspace')
    // A module registers (M1.4) only the pages its manifest claims.
    for (const module of WEB_MODULES) {
      for (const route of [...(module.routes ?? []), ...(module.publicRoutes ?? [])]) {
        expect({ path: route.path, module: pages.get(route.path) }).toEqual({ path: route.path, module: [module.id] })
      }
    }
  })

  it('palette entries', () => {
    // The menu's own entries, and the pages modules register (M1.4).
    const registered = WEB_MODULES.flatMap((module) => (module.palette ?? []).map((entry) => entry.id))
    const fixed = [...[...paletteSource.matchAll(/\bid: '([^']+)'/g)].map((match) => match[1]), ...registered]
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
    // Home's rows are registrations (M1.4), which show only where their module is on.
    const inboxOff = { shows: () => true, moduleOn: (module: string) => module !== 'inbox' }
    expect(sidebarRows('home', { shows: () => true, moduleOn: () => true }).map((row) => row.label)).toContain('Inbox')
    expect(sidebarRows('home', inboxOff).map((row) => row.label)).not.toContain('Inbox')
    expect(railSource).toMatch(/moduleOn\('messages'\) \? \(\s*<UtilityTile[^/]*label="Messages"/)
    expect(railSource).toContain("experience.moduleOn('projects')")
    // Every other Home sidebar row is a layout part, which follows modules on the server.
    expect(homeSidebarSource).toContain("sidebarRows('home', experience)")
    expect(homeSidebarSource.match(/<SidebarLink/g)).toHaveLength(2)
    for (const module of WEB_MODULES) {
      for (const row of (module.sidebar ?? []).filter((row) => row.space === 'home')) {
        expect(Boolean(row.part) || row.label === 'Inbox', `${module.id}:${row.id}`).toBe(true)
      }
    }
  })
})
