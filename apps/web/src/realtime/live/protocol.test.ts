import { describe, expect, it } from 'vitest'

import { parseRealtimeEnvelope } from '../realtimeModel'
import { after, hello, parseServerFrame } from './protocol'

// The server's example of every protocol 2 frame (K5.9), which its own tests
// parse too: a frame the server sends that the web can't read fails here.
const fixtures = import.meta.glob<{ client: Array<Record<string, unknown>>; server: unknown[] }>(
  '../../../../server/tests/fixtures/realtime_wire_v2.json',
  { eager: true, import: 'default' },
)
const wire = Object.values(fixtures)[0]

describe('protocol 2 frames', () => {
  it('reads every frame the server sends', () => {
    expect(wire, 'apps/server/tests/fixtures/realtime_wire_v2.json').toBeDefined()
    for (const frame of wire.server) {
      const parsed = parseServerFrame(frame)
      expect(parsed, JSON.stringify(frame)).not.toBeNull()
      expect(parsed?.type, JSON.stringify(frame)).not.toBe('other')
      if (parsed?.type === 'event') expect(parseRealtimeEnvelope(parsed.event), 'the envelope inside').not.toBeNull()
    }
  })

  it('sends hello as the server reads it', () => {
    expect(JSON.parse(hello('18342'))).toEqual(wire.client.find((frame) => frame.type === 'hello'))
    expect(JSON.parse(hello(null))).toEqual({ type: 'hello', protocol: 2, client: 'web' })
  })

  it('leaves protocol 1 envelopes to the old reader', () => {
    expect(parseServerFrame({ type: 'conversation.message.created', event_id: 'x' })).toBeNull()
    expect(parseServerFrame({ type: 'event', position: 'not a number', event: {} })).toBeNull()
  })

  it('orders positions past the safe integers', () => {
    expect(after('10', '9')).toBe(true)
    expect(after('9007199254740993', '9007199254740992')).toBe(true)
    expect(after('18342', '18342')).toBe(false)
    expect(after('1', null)).toBe(true)
  })
})
