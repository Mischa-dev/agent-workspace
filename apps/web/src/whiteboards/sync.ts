import type { StoredElement } from './api'

/**
 * What the board knows the server holds, per element: the version and nonce
 * it last sent or received. A local element is unsaved when its copy differs.
 */
export type Known = Map<string, { version: number; versionNonce: number }>

/** Excalidraw's rule, the same one the server applies: the higher version
 * wins, and on a tie the lower nonce does. */
export function incomingWins(
  incoming: { version: number; versionNonce: number },
  stored: { version: number; versionNonce: number } | undefined,
) {
  if (!stored) return true
  return incoming.version > stored.version || (incoming.version === stored.version && incoming.versionNonce < stored.versionNonce)
}

/** The local elements the server doesn't have yet. */
export function unsaved(elements: readonly StoredElement[], known: Known): StoredElement[] {
  return elements.filter((element) => {
    const stored = known.get(element.id)
    if (!stored) return !element.isDeleted
    return stored.version !== element.version || stored.versionNonce !== element.versionNonce
  })
}

export function remember(known: Known, elements: readonly StoredElement[]) {
  for (const element of elements) {
    known.set(element.id, { version: element.version, versionNonce: element.versionNonce })
  }
}

/** The images elements show that this board hasn't loaded. */
export function missingFiles(elements: readonly StoredElement[], loaded: ReadonlySet<string>): string[] {
  const wanted = new Set<string>()
  for (const element of elements) {
    if (element.isDeleted || element.type !== 'image') continue
    const fileId = element.fileId
    if (typeof fileId === 'string' && fileId && !loaded.has(fileId)) wanted.add(fileId)
  }
  return [...wanted]
}

export type SaveState = 'saved' | 'saving' | 'unsaved' | 'offline' | 'error' | 'failed'

export function saveLabel(state: SaveState) {
  switch (state) {
    case 'saved':
      return 'All changes saved'
    case 'saving':
      return 'Saving…'
    case 'unsaved':
      return 'Unsaved changes'
    case 'offline':
      return 'Offline: changes will save when you reconnect'
    case 'error':
      return 'Changes aren’t saving. Retrying…'
    case 'failed':
      return 'Changes aren’t saving'
  }
}
