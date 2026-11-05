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

  it('keeps a check on each row when its reason differs by task', () => {
    const waiting = (on: string): Readiness => ({
      ready: false,
      readyForAgents: false,
      checks: [{ id: 'blockers', ok: false, label: `Waiting on ${on}`, forAgents: false }],
    })
    expect(sharedMissingChecks([waiting('AWD-1'), waiting('AWD-2')])).toEqual([])
  })

  it('says nothing when one task already has what the others miss', () => {
    expect(sharedMissingChecks([readiness(['repository']), readiness([])])).toEqual([])
  })

  it('says nothing for a single task', () => {
    expect(sharedMissingChecks([readiness(['repository'])])).toEqual([])
  })
})
