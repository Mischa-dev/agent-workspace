import { describe, expect, it } from 'vitest'

import { healthTone } from './projectStatusModel'

describe('project status', () => {
  it('reads a health by key or label', () => {
    expect(healthTone('at_risk')).toBe('warning')
    expect(healthTone('Off track')).toBe('danger')
    expect(healthTone('on_track')).toBe('success')
    expect(healthTone(null)).toBe('muted')
  })
})
