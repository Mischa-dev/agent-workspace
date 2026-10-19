import { useSyncExternalStore } from 'react'

/**
 * Whether notifications are paused, as PausedMark (loaded just after the
 * first paint) last read it, so the account menu can offer Pause or Resume
 * without loading the notification settings itself.
 */
let pausedUntil: Date | null = null
const listeners = new Set<() => void>()

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function publishPausedUntil(until: Date | null) {
  if ((until?.getTime() ?? null) === (pausedUntil?.getTime() ?? null)) return
  pausedUntil = until
  for (const listener of listeners) listener()
}

export function usePausedUntil() {
  return useSyncExternalStore(subscribe, () => pausedUntil, () => pausedUntil)
}
