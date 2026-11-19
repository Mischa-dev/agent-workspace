import { describe, expect, it } from 'vitest'

import { myColors } from './myColors'

describe('myColors', () => {
  const overrides = { accent: '#224466' }

  it('keeps colours made on Match system in both modes', () => {
    const document = myColors('system', overrides)
    expect(document.light).toEqual({ base: 'quiet', tokens: overrides })
    expect(document.dark).toEqual({ base: 'quiet-dark', tokens: overrides })
  })

  it('keeps colours made on one theme in that theme’s mode', () => {
    expect(myColors('quiet-dark', overrides).dark).toEqual({ base: 'quiet-dark', tokens: overrides })
    expect(myColors('quiet-dark', overrides).light.tokens).toEqual({})
    expect(myColors('paper', overrides).light).toEqual({ base: 'paper', tokens: overrides })
  })
})
