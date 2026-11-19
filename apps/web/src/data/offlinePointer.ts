// Whose offline copy this device keeps (W1.31), read before anything else
// so the app can show it before the network answers. Only an account id; the
// copy itself is in IndexedDB (data/persist.ts), loaded only when this says
// there is one.

import type { EffectiveSettings } from '../settings/api'

const POINTER = 'agent-workspace:offline-reading'
/** Where the copy is kept (data/persist.ts opens it). */
export const OFFLINE_DATABASE = 'agent-workspace-offline'

/** The account whose pages this device keeps for reading offline, if any. */
export function offlineOwner(): string | null {
  try {
    return window.localStorage.getItem(POINTER)
  } catch {
    return null
  }
}

export function setOfflineOwner(owner: string | null) {
  try {
    if (owner) window.localStorage.setItem(POINTER, owner)
    else window.localStorage.removeItem(POINTER)
  } catch {
    // Without storage nothing is kept, so there's nothing to point at.
  }
}

/** Forget every kept copy on this device, without loading the code that keeps them: signing out calls this. */
export function forgetOffline() {
  setOfflineOwner(null)
  try {
    indexedDB?.deleteDatabase(OFFLINE_DATABASE)
  } catch {
    // No IndexedDB, so nothing was kept.
  }
}

/** On when the person turned "Read offline" on and their workspace allows it. */
export function offlineReadingOn(settings: EffectiveSettings): boolean {
  const value = (key: string) => settings.settings.find((setting) => setting.key === key)?.value
  return value('offline.allowed') !== false && value('offline.reading') === true
}
