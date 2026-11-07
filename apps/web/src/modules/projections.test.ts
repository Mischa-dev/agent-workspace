import names from 'virtual:modules/names'
import switchable from 'virtual:modules/switchable'
import { describe, expect, it } from 'vitest'

import { MODULE_IDS } from './names'
import { ownerOf } from './ownership'
import { moduleOfPath } from './pageOwners'
import { CLAIMS, type SlotKind } from './slots'

// The build-time projections (moduleProjections in vite.config.ts) say exactly
// what the canonical manifests say: every claim of each kind, in module order
// and then the manifest's own order, and nothing else.

type Manifest = { name?: string; switchable?: boolean; slots?: Partial<Record<SlotKind, string[]>> }

const manifests = Object.entries(
  import.meta.glob<Manifest>(['../../../../modules/*.json', '!../../../../modules/presets.json'], { eager: true, import: 'default' }),
)
  .map(([path, manifest]) => ({ id: path.slice(path.lastIndexOf('/') + 1).replace(/\.json$/, ''), manifest }))
  .sort((left, right) => (left.id < right.id ? -1 : left.id > right.id ? 1 : 0))

describe('module projections', () => {
  it('match the manifests, kind by kind, in a fixed order', () => {
    for (const kind of Object.keys(CLAIMS) as SlotKind[]) {
      const expected = manifests.flatMap(({ id, manifest }) => (manifest.slots?.[kind] ?? []).map((pattern) => [pattern, id]))
      expect(CLAIMS[kind], kind).toEqual(expected)
    }
    // Every kind a manifest uses is projected.
    const used = new Set(manifests.flatMap(({ manifest }) => Object.keys(manifest.slots ?? {})))
    expect([...used].sort()).toEqual(Object.keys(CLAIMS).sort())
  })

  it('carry each module’s name and which can be off', () => {
    expect(MODULE_IDS).toEqual(manifests.map(({ id }) => id))
    expect(names).toEqual(Object.fromEntries(manifests.map(({ id, manifest }) => [id, manifest.name ?? id])))
    expect(switchable).toEqual(manifests.filter(({ manifest }) => manifest.switchable === true).map(({ id }) => id))
    expect(switchable).not.toContain('workspace')
  })

  it('match routes as the router does', () => {
    // A static route wins over the task-key route, and the catch-all comes last.
    expect(moduleOfPath('/home')).toBe('workspace')
    expect(moduleOfPath('/AWD-12')).toBe('tasks')
    expect(moduleOfPath('/settings/workspace/modules')).toBe('workspace')
    expect(moduleOfPath('/no/such/page')).toBe('workspace')
    expect(moduleOfPath('/projects/p1/pages/x1')).toBe('pages')
  })

  it('give a value to its most specific claim, the first on a tie', () => {
    const claims = [['a*', 'one'], ['ab*', 'two'], ['abc', 'three'], ['ab*', 'four']] as const
    expect(ownerOf(claims, 'abd')).toBe('two')
    expect(ownerOf(claims, 'abc')).toBe('three')
    expect(ownerOf(claims, 'ax')).toBe('one')
    expect(ownerOf(claims, 'b')).toBeNull()
  })
})
