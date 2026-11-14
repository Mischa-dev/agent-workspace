import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

import { PART_IDS } from '../experience/parts'
import { routePaths } from '../app/routes'
import { MODULE_IDS } from './names'
import { REGISTERED_FOLDERS, WEB_MODULES, paletteEntries, sidebarRows } from './registry'

// M1.4: the web registrations against the manifests (`modules/*.json` at the
// root of the repository), which name each module's web folder.

type Manifest = { id: string; web?: { folder: string } }

const root = join(process.cwd(), '..', '..', 'modules')
const manifests: Manifest[] = readdirSync(root)
  .filter((name) => name.endsWith('.json') && name !== 'presets.json')
  .map((name) => JSON.parse(readFileSync(join(root, name), 'utf8')) as Manifest)

const src = join(process.cwd(), 'src')

describe('the web module registry', () => {
  it('registers only modules that have a manifest, each once', () => {
    const ids = WEB_MODULES.map((module) => module.id)
    expect(ids.filter((id) => !MODULE_IDS.includes(id))).toEqual([])
    expect(ids.filter((id, index) => ids.indexOf(id) !== index)).toEqual([])
  })

  it('has a module.ts in the folder each manifest names, and none elsewhere', () => {
    const named = Object.fromEntries(manifests.filter((manifest) => manifest.web).map((manifest) => [manifest.id, manifest.web?.folder]))
    expect(REGISTERED_FOLDERS).toEqual(named)
  })

  it('keeps every module.ts free of static imports other than types', () => {
    for (const folder of Object.values(REGISTERED_FOLDERS)) {
      const source = readFileSync(join(src, folder, 'module.ts'), 'utf8')
      const imports = source.split('\n').filter((line) => /^import\s/.test(line) && !/^import type\s/.test(line))
      expect({ folder, imports }).toEqual({ folder, imports: [] })
    }
  })

  it('names a part for every sidebar row, or shows it wherever its module is on', () => {
    const alwaysShown = ['inbox:inbox']
    for (const module of WEB_MODULES) {
      for (const row of module.sidebar ?? []) {
        if (!row.part) {
          expect(alwaysShown).toContain(`${module.id}:${row.id}`)
          continue
        }
        expect(PART_IDS).toContain(row.part)
        if (row.unless) expect(PART_IDS).toContain(row.unless)
      }
    }
  })

  it('puts every registered page in the route table, once', () => {
    const paths = routePaths()
    for (const module of WEB_MODULES) {
      for (const route of [...(module.routes ?? []), ...(module.publicRoutes ?? [])]) {
        expect(paths.filter((path) => path === route.path)).toEqual([route.path])
      }
    }
  })

  it('leaves out the rows and entries of a module that is off', () => {
    const everything = { shows: () => true, moduleOn: () => true }
    const noTasks = { shows: () => true, moduleOn: (module: string) => module !== 'tasks' }
    expect(sidebarRows('project', everything).map((row) => row.module)).toContain('tasks')
    expect(sidebarRows('project', noTasks).map((row) => row.module)).not.toContain('tasks')
    // Decisions stands in for Pages only where Pages is hidden.
    expect(sidebarRows('project', everything).map((row) => row.id)).not.toContain('decisions')
    const ids = paletteEntries('project').map((entry) => entry.id)
    expect(ids.indexOf('p:tasks')).toBeLessThan(ids.indexOf('p:pages'))
  })
})
