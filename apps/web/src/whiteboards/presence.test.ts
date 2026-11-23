import { describe, expect, it } from 'vitest'

import { PEER_TIMEOUT_MS, followView, prunePeers, readPresence, type Peer } from './presence'

const viewport = { scrollX: -100, scrollY: 50, zoom: 2, width: 800, height: 600 }

describe('board presence', () => {
  it('reads what another board sent and refuses anything malformed', () => {
    expect(readPresence({ pointer: { x: 1, y: 2 }, button: 'down', selected: ['a', 3, 'b'], viewport })).toEqual({
      pointer: { x: 1, y: 2 },
      button: 'down',
      selected: ['a', 'b'],
      viewport,
    })
    expect(readPresence(null)).toBeNull()
    expect(readPresence({ viewport: { ...viewport, zoom: 0 } })).toBeNull()
    expect(readPresence({ pointer: { x: 'no' }, viewport })?.pointer).toBeNull()
    expect(readPresence({ selected: Array.from({ length: 50 }, (_, i) => `e${i}`), viewport })?.selected).toHaveLength(20)
  })

  it('follows the middle of the leader’s view at the leader’s zoom', () => {
    const view = followView(viewport, { width: 400, height: 300 })
    // Both windows centre the same point of the board.
    const leaderCentre = { x: viewport.width / (2 * viewport.zoom) - viewport.scrollX, y: viewport.height / (2 * viewport.zoom) - viewport.scrollY }
    expect(400 / (2 * view.zoom) - view.scrollX).toBeCloseTo(leaderCentre.x)
    expect(300 / (2 * view.zoom) - view.scrollY).toBeCloseTo(leaderCentre.y)
    expect(view.zoom).toBe(2)
  })

  it('drops a board that went quiet', () => {
    const peers = new Map<string, Peer>([
      ['old', { accountId: 'a', presence: readPresence({ viewport })!, seen: 0 }],
      ['new', { accountId: 'b', presence: readPresence({ viewport })!, seen: PEER_TIMEOUT_MS }],
    ])
    expect(prunePeers(peers, PEER_TIMEOUT_MS + 1)).toBe(true)
    expect([...peers.keys()]).toEqual(['new'])
    expect(prunePeers(peers, PEER_TIMEOUT_MS + 1)).toBe(false)
  })
})
