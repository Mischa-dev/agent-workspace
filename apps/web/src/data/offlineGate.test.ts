import { describe, expect, it } from 'vitest'

import type { EffectiveSettings } from '../settings/api'
import { offlineGate } from './offlineGate'

const settings = (values: Record<string, unknown>) =>
  ({ settings: Object.entries(values).map(([key, value]) => ({ key, value })) }) as unknown as EffectiveSettings

const home = { id: 'ws-home', name: 'Home' }
const work = { id: 'ws-work', name: 'Work' }

describe('offlineGate', () => {
  it('keeps when the person turned it on and every workspace they are in allows it', () => {
    const known = new Map([
      [home.id, settings({ 'offline.reading': true })],
      [work.id, settings({ 'offline.allowed': true })],
    ])
    expect(offlineGate(home.id, [home, work], (id) => known.get(id))).toEqual({ on: true, blockedBy: [] })
  })

  it('keeps nothing when another workspace the person is in does not allow it', () => {
    const known = new Map([
      [home.id, settings({ 'offline.reading': true })],
      [work.id, settings({ 'offline.allowed': false })],
    ])
    expect(offlineGate(home.id, [home, work], (id) => known.get(id))).toEqual({ on: false, blockedBy: [work] })
  })

  it('changes nothing while a workspace’s settings are unknown', () => {
    const known = new Map([[home.id, settings({ 'offline.reading': true })]])
    expect(offlineGate(home.id, [home, work], (id) => known.get(id)).on).toBeNull()
  })

  it('keeps nothing once the person turned it off, even while another workspace is unknown', () => {
    const known = new Map([[home.id, settings({ 'offline.reading': false })]])
    expect(offlineGate(home.id, [home, work], (id) => known.get(id)).on).toBe(false)
  })
})
