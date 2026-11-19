import { describe, expect, it } from 'vitest'

import { SELF_HANDLED } from '../handlers'
import { HANDLERS } from '../handlers/table'
import { parseRealtimeEnvelope } from '../realtimeModel'
import { parseServerFrame } from './protocol'
import { parseFrames, replay, type Seed } from './replay'

// W1.7: every recorded stream under fixtures/ (from e2e/live-streams.spec.ts,
// re-recorded with RECORD_FRAMES=1) replays through the real live connection
// with its seed, and meets its expectations file. All are protocol 2's
// (W1.6); the first four were recorded on protocol 1 and rewrapped as
// `event` frames when its path went (W1.6b), with the same expectations.

type Count = number | { min: number }
type Expectations = {
  about: string
  settleMs?: number
  /** Open queries and how many times each is fetched. */
  fetched?: Record<string, Count>
  /** Cached queries that go stale. */
  stale?: string[]
  /** Queries neither fetched nor made stale. */
  untouched?: string[]
  /** Frames the client sends (protocol 2), each matched on the fields given. */
  sent?: Array<Record<string, unknown>>
}

const frames = import.meta.glob<string>('./fixtures/*.jsonl', { eager: true, import: 'default', query: '?raw' })
const seeds = import.meta.glob<Seed>('./fixtures/*.seed.json', { eager: true, import: 'default' })
const expectations = import.meta.glob<Expectations>('./fixtures/*.expect.json', { eager: true, import: 'default' })

const fixtures = Object.keys(frames).map((path) => {
  const base = path.replace(/\.jsonl$/, '')
  return { name: base.split('/').at(-1) as string, frames: parseFrames(frames[path]), seed: seeds[`${base}.seed.json`], expect: expectations[`${base}.expect.json`] }
})

describe('recorded event streams', () => {
  it('has the streams the plan names, each with a seed and expectations', () => {
    expect(fixtures.map((fixture) => fixture.name).sort()).toEqual(['chat-burst', 'page-edits', 'reconnect', 'resume', 'resync', 'task-board-moves'])
    for (const fixture of fixtures) {
      expect(fixture.seed, `${fixture.name}.seed.json`).toBeDefined()
      expect(fixture.expect, `${fixture.name}.expect.json`).toBeDefined()
    }
  })

  it.each(fixtures.map((fixture) => [fixture.name, fixture] as const))('%s: every frame is one the client reads and handles', (_, fixture) => {
    for (const frame of fixture.frames) {
      if ('$' in frame) continue
      const wire = parseServerFrame(frame)
      expect(wire, JSON.stringify(frame)).not.toBeNull()
      expect(wire?.type, JSON.stringify(frame)).not.toBe('other')
      if (wire?.type !== 'event') continue
      const event = parseRealtimeEnvelope(wire.event)
      expect(event && (event.type in HANDLERS || SELF_HANDLED.has(event.type)), JSON.stringify(frame)).toBe(true)
    }
  })

  it.each(fixtures.map((fixture) => [fixture.name, fixture] as const))(
    '%s replays as expected',
    async (_, fixture) => {
      const run = await replay(fixture.frames, fixture.seed, { settleMs: fixture.expect.settleMs })
      for (const [name, count] of Object.entries(fixture.expect.fetched ?? {})) {
        if (typeof count === 'number') expect(run.fetches[name], `${name} fetches`).toBe(count)
        else expect(run.fetches[name], `${name} fetches`).toBeGreaterThanOrEqual(count.min)
      }
      for (const name of fixture.expect.stale ?? []) expect(run.invalidated, `${name} stale`).toContain(name)
      for (const name of fixture.expect.untouched ?? []) expect(run.invalidated, `${name} untouched`).not.toContain(name)
      for (const frame of fixture.expect.sent ?? []) expect(run.sent).toContainEqual(expect.objectContaining(frame))
    },
    10_000,
  )
})
