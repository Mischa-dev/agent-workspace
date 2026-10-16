import { describe, expect, it } from 'vitest'

import { actorKind, initials, swatchFor, SWATCHES } from './identityModel'

describe('identity helpers', () => {
  it('produces stable palette colors, including for Unicode seeds', () => {
    expect(SWATCHES).toContain(swatchFor('actor-1'))
    expect(swatchFor('Ada')).toBe(swatchFor('Ada'))
    expect(SWATCHES).toContain(swatchFor('🙂'))
  })

  it('creates initials from one or many Unicode-aware words', () => {
    expect(initials('')).toBe('?')
    expect(initials('  ada  ')).toBe('A')
    expect(initials('ada lovelace')).toBe('AL')
    expect(initials('Émile du Châtelet')).toBe('ÉC')
    expect(initials('🙂 helper')).toBe('🙂H')
  })

  it.each([
    ['external_agent', 'agent'], ['execution_worker', 'worker'],
    ['integration', 'app'], ['system', 'app'], ['human', 'person'], ['unknown', 'person'],
  ] as const)('maps %s to %s', (serverKind, expected) => {
    expect(actorKind(serverKind)).toBe(expected)
  })
})
