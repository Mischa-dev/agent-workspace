/**
 * Whiteboard changes from the realtime socket. An open board listens for its
 * own id and fetches what changed since its last look; lists and linked-board
 * panels refetch through the query cache instead.
 */
export type WhiteboardChange = { whiteboardId: string; sceneSeq: number; version: number }

type Listener = (change: WhiteboardChange) => void

const listeners = new Map<string, Set<Listener>>()

export function onWhiteboardChange(whiteboardId: string, listener: Listener) {
  const set = listeners.get(whiteboardId) ?? new Set<Listener>()
  set.add(listener)
  listeners.set(whiteboardId, set)
  return () => {
    set.delete(listener)
    if (set.size === 0) listeners.delete(whiteboardId)
  }
}

export function publishWhiteboardChange(change: WhiteboardChange) {
  for (const listener of listeners.get(change.whiteboardId) ?? []) listener(change)
}
