import { describe, expect, it } from 'vitest'

import { configJson, normalizeConfig, recordsParams } from './api'
import { activeConfig, sameConfig } from './collectionModel'

const bug = { field: 'kind', op: 'is' as const, values: ['bug'] }
const urgent = { field: 'priority', op: 'is' as const, values: ['urgent'] }
const mine = { field: 'owner', op: 'is' as const, values: ['me'] }

describe("a view's filter groups", () => {
  it('keeps an "or" filter through a sort change and a save', () => {
    const stored = { filters: { or: [bug, urgent] }, sort: [{ field: 'title', direction: 'asc' }] }
    const config = normalizeConfig(stored)
    expect(config.filters).toEqual([])
    expect(config.filterGroups).toEqual([{ or: [bug, urgent] }])

    const resorted = { ...config, sort: [{ field: 'title', direction: 'desc' as const }] }
    expect(configJson(resorted).filters).toEqual({ or: [bug, urgent] })
    expect(sameConfig(resorted, config)).toBe(false)
    expect(JSON.parse(new URLSearchParams(recordsParams({ view: 'v', filters: configJson(resorted).filters })).get('filters') ?? '')).toEqual({
      or: [bug, urgent],
    })
  })

  it('shows the clauses that all hold and keeps the groups beside them', () => {
    const config = normalizeConfig({ filters: { and: [mine, { or: [bug, urgent] }] } })
    expect(config.filters).toEqual([mine])
    expect(config.filterGroups).toEqual([{ or: [bug, urgent] }])
    expect(configJson(config).filters).toEqual({ and: [mine, { or: [bug, urgent] }] })

    // Removing the clause the toolbar shows leaves the group as it was.
    expect(configJson({ ...config, filters: [] }).filters).toEqual({ or: [bug, urgent] })
    // Groups survive the archived-field pass, which only drops plain clauses.
    expect(activeConfig(config, []).filterGroups).toEqual([{ or: [bug, urgent] }])
  })

  it('writes a plain list back as a list', () => {
    const config = normalizeConfig({ filters: [bug, urgent] })
    expect(config.filters).toEqual([bug, urgent])
    expect(config.filterGroups).toBeUndefined()
    expect(configJson(config).filters).toEqual([bug, urgent])
  })
})
