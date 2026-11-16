import { describe, expect, it } from 'vitest'

import { moduleRoutes } from '../app/routes'
import type { Experience } from '../experience/layout'
import { shortcutOn } from '../shell/shortcutModules'
import { createShortcutMatcher, gSequences } from '../shell/shortcuts'
import { shortcutSheet } from '../shell/shortcutSheet'
import type { WebModule } from './define'
import { captureEntries, moduleShortcuts, moduleThings, paletteEntries, sidebarRows, WEB_MODULES } from './registry'

// W1.13: the shell is drawn from the registrations. A module that exists only
// in this test brings a route, a sidebar row, a palette entry, a shortcut, a
// kind of thing and a capture entry, and each appears with no other file
// changed; turning a module off takes its rows and keys away, where the key
// acts (the open project, or Home).

const fake: WebModule = {
  id: 'scaffold',
  shell: 'home',
  routes: [{ path: '/scaffold', space: 'home', page: () => Promise.resolve({ default: () => null }) }],
  sidebar: [{ id: 'scaffold', space: 'home', rank: 99, icon: 'list', label: 'Scaffold', path: '/scaffold' }],
  palette: [{ id: 'g:scaffold', space: 'workspace', rank: 99, label: 'Scaffold', icon: 'list', shortcut: 'G Z', path: '/scaffold' }],
  shortcuts: [{ action: 'go-scaffold', key: 'z', label: 'Scaffold', path: '/scaffold', rank: 99 }],
  things: [{ kind: 'scaffold_item', label: 'Scaffold item', icon: 'list' }],
  capture: [{ id: 'scaffold', label: 'New scaffold item', icon: 'list', path: '/scaffold?new=1' }],
}
const withFake = [...WEB_MODULES, fake]

const everything = { shows: () => true, moduleOn: () => true }
const without = (off: string) => ({ shows: () => true, moduleOn: (module: string) => module !== off })

/** A layout as the shortcut checks read it. */
function experience(off: string[] = [], loading = false): Experience {
  return {
    modules: loading ? 'loading' : 'ready',
    moduleOn: (module: string) => !off.includes(module),
  } as unknown as Experience
}

function keys(sequence: string[], sequences = gSequences(withFake)) {
  const press = createShortcutMatcher(() => 0, sequences)
  let last: string | null = null
  for (const key of sequence) {
    last = press({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, target: null, isComposing: false })
  }
  return last
}

describe('a registration draws itself into the shell', () => {
  it('adds its route, sidebar row, palette entry, shortcut, thing and capture entry', () => {
    expect(moduleRoutes('routes', withFake).map((route) => route.path)).toContain('/scaffold')
    expect(moduleRoutes('routes').map((route) => route.path)).not.toContain('/scaffold')
    expect(sidebarRows('home', everything, withFake).map((row) => row.id)).toContain('scaffold')
    expect(paletteEntries('workspace', withFake).map((entry) => entry.id)).toContain('g:scaffold')
    expect(keys(['g', 'z'])).toBe('go-scaffold')
    expect(keys(['g', 'z'], gSequences())).toBeNull()
    expect(shortcutSheet(withFake).find((shortcut) => shortcut.action === 'go-scaffold')?.keys).toEqual(['G', 'Z'])
    expect(moduleThings(withFake).find((thing) => thing.kind === 'scaffold_item')?.module).toBe('scaffold')
    expect(captureEntries(everything, withFake).map((entry) => entry.id)).toContain('scaffold')
  })

  it('takes them away with its module', () => {
    expect(sidebarRows('home', without('scaffold'), withFake).map((row) => row.id)).not.toContain('scaffold')
    expect(captureEntries(without('scaffold'), withFake).map((entry) => entry.id)).not.toContain('scaffold')
  })
})

describe('registered keys follow the modules where they act', () => {
  it('keeps the keys the shell has always had', () => {
    const sequences = gSequences()
    expect(sequences).toMatchObject({
      h: 'go-home',
      i: 'go-inbox',
      w: 'go-my-work',
      l: 'go-later',
      m: 'go-messages',
      t: 'go-tasks',
      b: 'go-board',
      p: 'go-pages',
      d: 'go-decisions',
      s: 'go-settings',
      a: 'go-appearance',
    })
    expect(shortcutSheet().map((shortcut) => shortcut.label)).toEqual([
      'Search and run commands',
      'Show keyboard shortcuts',
      'New task',
      'Search messages and pages',
      'Previous item in the sidebar',
      'Next item in the sidebar',
      'Previous unread conversation',
      'Next unread conversation',
      'Mark all messages read',
      'Home',
      'Inbox',
      'My work',
      'Later',
      'Messages',
      'Tasks',
      'Board',
      'Pages',
      'Decisions',
      'Project settings',
      'Appearance',
    ])
    // Each key is its module's: no two modules claim one letter.
    const letters = moduleShortcuts().map((shortcut) => shortcut.key)
    expect(letters.filter((letter, index) => letters.indexOf(letter) !== index)).toEqual([])
  })

  it('a project key follows the open project, the rest Home', () => {
    // Tasks on in the project, off at Home: G T works there; G W doesn't.
    const project = experience()
    const home = experience(['tasks'])
    expect(shortcutOn('go-tasks', project, home)).toBe(true)
    expect(shortcutOn('go-my-work', project, home)).toBe(false)
    // The other way round.
    expect(shortcutOn('go-tasks', experience(['tasks']), experience())).toBe(false)
    expect(shortcutOn('go-my-work', experience(['tasks']), experience())).toBe(true)
    // With no project open, a project key follows Home.
    expect(shortcutOn('go-tasks', null, home)).toBe(false)
    // Pages' keys follow Pages in the project.
    expect(shortcutOn('go-pages', experience(['pages']), experience())).toBe(false)
    // The shell's own keys are always on.
    expect(shortcutOn('go-home', experience(['tasks']), experience(['tasks']))).toBe(true)
  })

  it('global keys wait for Home’s settings', () => {
    expect(shortcutOn('go-my-work', experience(), experience([], true))).toBe(false)
    expect(shortcutOn('go-inbox', experience(), experience([], true))).toBe(false)
    expect(shortcutOn('go-inbox', experience(), experience())).toBe(true)
  })
})
