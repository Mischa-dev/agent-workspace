import { describe, expect, it } from 'vitest'

import { HANDLERS, SELF_HANDLED } from '../handlers'
import { parseLegacyRealtimeEvent, parseRealtimeEnvelope } from '../realtimeModel'
import { parseFrames, replay, type Seed } from './replay'

// W1.7: every recorded stream under fixtures/ (from e2e/live-streams.spec.ts,
// re-recorded with RECORD_FRAMES=1) replays through the real live connection
// with its seed, and meets its expectations file.

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
    expect(fixtures.map((fixture) => fixture.name).sort()).toEqual(['chat-burst', 'page-edits', 'reconnect', 'task-board-moves'])
    for (const fixture of fixtures) {
      expect(fixture.seed, `${fixture.name}.seed.json`).toBeDefined()
      expect(fixture.expect, `${fixture.name}.expect.json`).toBeDefined()
    }
  })

  it.each(fixtures.map((fixture) => [fixture.name, fixture] as const))('%s: every frame is one the client reads and handles', (_, fixture) => {
    for (const frame of fixture.frames) {
      if ('$' in frame) continue
      const envelope = parseRealtimeEnvelope(frame)
      if (envelope) expect(envelope.type in HANDLERS || SELF_HANDLED.has(envelope.type), envelope.type).toBe(true)
      else expect(parseLegacyRealtimeEvent(frame), JSON.stringify(frame)).not.toBeNull()
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
    },
    10_000,
  )
})
