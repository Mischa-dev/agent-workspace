// Live presence on a board (C1.30): each open board sends where its pointer
// is, what it has selected and what part of the board it shows, as
// awareness frames in the board's live room; the others draw that as a
// cursor with a name, outline the selection, and can follow the view.

/** What one open board tells the others. */
export type BoardPresence = {
  pointer: { x: number; y: number } | null
  button: 'up' | 'down'
  selected: string[]
  viewport: { scrollX: number; scrollY: number; zoom: number; width: number; height: number }
}

/** Someone else on the board, by their connection. */
export type Peer = { accountId: string; presence: BoardPresence; seen: number }

/** A cursor that hasn't said anything for this long is gone. */
export const PEER_TIMEOUT_MS = 30_000
/** An open board says it's still here this often, even standing still. */
export const HEARTBEAT_MS = 10_000
/** Pointer moves go out at most this often. */
export const SEND_EVERY_MS = 80
/** At most this many selected elements are shared. */
export const MAX_SELECTED = 20

const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value)

/** Reads what another board sent; anything malformed is nobody. */
export function readPresence(state: unknown): BoardPresence | null {
  if (!state || typeof state !== 'object') return null
  const value = state as Record<string, unknown>
  const view = value.viewport as Record<string, unknown> | undefined
  if (!view || !finite(view.scrollX) || !finite(view.scrollY) || !finite(view.zoom) || view.zoom <= 0 || !finite(view.width) || !finite(view.height)) {
    return null
  }
  const pointer = value.pointer as Record<string, unknown> | null | undefined
  const selected = Array.isArray(value.selected) ? value.selected.filter((id): id is string => typeof id === 'string').slice(0, MAX_SELECTED) : []
  return {
    pointer: pointer && finite(pointer.x) && finite(pointer.y) ? { x: pointer.x, y: pointer.y } : null,
    button: value.button === 'down' ? 'down' : 'up',
    selected,
    viewport: { scrollX: view.scrollX, scrollY: view.scrollY, zoom: view.zoom, width: view.width, height: view.height },
  }
}

/** Drops the peers that have gone quiet; true when any went. */
export function prunePeers(peers: Map<string, Peer>, now: number) {
  let changed = false
  for (const [connection, peer] of peers) {
    if (now - peer.seen > PEER_TIMEOUT_MS) {
      peers.delete(connection)
      changed = true
    }
  }
  return changed
}

/**
 * Where a follower scrolls to show what the leader shows: the same middle
 * of the board at the leader's zoom, whatever size the follower's window is.
 */
export function followView(leader: BoardPresence['viewport'], follower: { width: number; height: number }) {
  const centreX = leader.width / (2 * leader.zoom) - leader.scrollX
  const centreY = leader.height / (2 * leader.zoom) - leader.scrollY
  return {
    scrollX: follower.width / (2 * leader.zoom) - centreX,
    scrollY: follower.height / (2 * leader.zoom) - centreY,
    zoom: leader.zoom,
  }
}
