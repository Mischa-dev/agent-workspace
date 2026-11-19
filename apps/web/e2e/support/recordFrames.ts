import { writeFileSync } from 'node:fs'
import path from 'node:path'

import type { Page } from '@playwright/test'

import { anonymousFixtureId, FIXTURE_IDS, type FixtureIdName } from '../../src/realtime/live/fixtureIds'
import { webRoot } from '../environment'

// W1.7: records the frames the real server sends the page's live connection,
// as fixtures for src/realtime/live/replay.ts. Every id is swapped for a fixed
// one (fixtureIds.ts), so a recording is the same each time; times are kept.
// RECORD_FRAMES=1 writes them under src/realtime/live/fixtures/, like
// UPDATE_DOC_PARITY for the page parity fixtures.

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi
const fixtures = path.join(webRoot, 'src', 'realtime', 'live', 'fixtures')

export type Recorder = {
  /** Frames since recording started (or since `clear`), with `{"$":"close"}` and `{"$":"open"}` where the socket closed and reopened. */
  frames: () => unknown[]
  clear: () => void
  /** Writes the frames as `<name>.jsonl` when RECORD_FRAMES=1, real ids swapped for the named ones and the rest numbered. */
  save: (name: string, named: Partial<Record<FixtureIdName, string>>) => string
}

export function recordFrames(page: Page): Recorder {
  let frames: unknown[] = []
  let sockets = 0
  // The socket whose frames count: a reload opens the next before the last one closes.
  let current: unknown = null
  page.on('websocket', (socket) => {
    if (!socket.url().includes('/api/v1/realtime')) return
    if (sockets++ > 0) {
      if (current) frames.push({ $: 'close' })
      frames.push({ $: 'open' })
    }
    current = socket
    socket.on('framereceived', ({ payload }) => {
      if (current !== socket || typeof payload !== 'string') return
      try {
        frames.push(JSON.parse(payload))
      } catch {
        // Not a frame the client reads.
      }
    })
    socket.on('close', () => {
      if (current !== socket) return
      current = null
      frames.push({ $: 'close' })
    })
  })
  return {
    frames: () => frames,
    clear: () => {
      frames = []
    },
    save: (name, named) => {
      const ids = new Map<string, string>()
      for (const [role, id] of Object.entries(named)) if (id) ids.set(id.toLowerCase(), FIXTURE_IDS[role as FixtureIdName])
      let next = 0
      // A socket that closed at the end of the test isn't part of the stream.
      const kept = frames.at(-1) && (frames.at(-1) as { $?: string }).$ === 'close' ? frames.slice(0, -1) : frames
      const text = kept
        .map((frame) =>
          JSON.stringify(frame).replace(UUID, (id) => {
            const key = id.toLowerCase()
            if (!ids.has(key)) ids.set(key, anonymousFixtureId(next++))
            return ids.get(key) as string
          }),
        )
        .join('\n')
      if (process.env.RECORD_FRAMES === '1') writeFileSync(path.join(fixtures, `${name}.jsonl`), `${text}\n`)
      return text
    },
  }
}
