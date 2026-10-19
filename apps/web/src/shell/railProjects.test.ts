import { describe, expect, it } from 'vitest'

import type { Project } from '../projects/types'
import { railProjects } from './railProjects'

function project(name: string, lastOpenedAt: string | null, pinned = false): Project {
  return {
    id: name.toLowerCase(),
    organizationId: 'organization-a',
    key: name.slice(0, 3).toUpperCase(),
    name,
    archivedAt: null,
    role: 'owner',
    version: 1,
    attentionCount: 0,
    capabilities: { view: true, manage: true, archive: true, navigationWrite: true },
    navigation: {
      location: { kind: 'project_overview' },
      href: `/projects/${name.toLowerCase()}`,
      pinned,
      lastOpenedAt,
      version: 1,
    },
  }
}

const names = (projects: Project[]) => projects.map((item) => item.name)

describe('railProjects', () => {
  it('keeps recent tiles in name order, so opening one does not move the others', () => {
    const before = [project('Cedar', '2026-10-19T09:00:00Z'), project('Alder', '2026-10-19T08:00:00Z'), project('Birch', '2026-10-19T07:00:00Z')]
    expect(names(railProjects(before, null))).toEqual(['Alder', 'Birch', 'Cedar'])

    // Opening Birch makes it the most recent; the tiles stay where they were.
    const after = before.map((item) =>
      item.name === 'Birch' ? { ...item, navigation: { ...item.navigation, lastOpenedAt: '2026-10-19T10:00:00Z' } } : item,
    )
    expect(names(railProjects(after, 'birch'))).toEqual(['Alder', 'Birch', 'Cedar'])
  })

  it('puts pins first in their own order, then the five most recent, then the open project', () => {
    const projects = [
      project('Zinc', '2026-10-19T01:00:00Z', true),
      project('Apple', '2026-10-18T01:00:00Z', true),
      project('Fig', '2026-10-19T06:00:00Z'),
      project('Elm', '2026-10-19T05:00:00Z'),
      project('Date', '2026-10-19T04:00:00Z'),
      project('Clove', '2026-10-19T03:00:00Z'),
      project('Basil', '2026-10-19T02:00:00Z'),
      project('Acorn', '2026-10-10T00:00:00Z'),
      project('Never', null),
    ]
    expect(names(railProjects(projects, null))).toEqual(['Zinc', 'Apple', 'Basil', 'Clove', 'Date', 'Elm', 'Fig'])
    expect(names(railProjects(projects, 'never'))).toEqual(['Zinc', 'Apple', 'Basil', 'Clove', 'Date', 'Elm', 'Fig', 'Never'])
  })
})
