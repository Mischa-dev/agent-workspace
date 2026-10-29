import { describe, expect, it } from 'vitest'

import type { Readiness } from './api'
import { sharedMissingChecks } from './readiness'

const readiness = (missing: string[]): Readiness => ({
  ready: true,
  readyForAgents: missing.length === 0,
  checks: ['criteria', 'description', 'repository'].map((id) => ({ id, ok: !missing.includes(id), label: id, forAgents: true })),
})

describe('sharedMissingChecks', () => {
  it('finds what every task misses, so it is said once above the list', () => {
    expect(sharedMissingChecks([readiness(['criteria', 'repository']), readiness(['repository', 'description'])]).map((check) => check.id)).toEqual([
      'repository',
    ])
  })

  it('says nothing for a single task', () => {
    expect(sharedMissingChecks([readiness(['repository'])])).toEqual([])
  })
})
