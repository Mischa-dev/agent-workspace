/** Identity helpers: how people and agents get their colors, initials and kinds. */
import { createContext } from 'react'

export type ActorKind = 'person' | 'agent' | 'worker' | 'app'

export const SWATCHES = ['indigo', 'blue', 'teal', 'green', 'rust', 'rose', 'orchid', 'slate'] as const
export type Swatch = (typeof SWATCHES)[number]

/** Stable swatch for a name or id, so a person keeps their color everywhere. */
export function swatchFor(seed: string): Swatch {
  let hash = 0
  for (const character of seed) hash = (hash * 31 + (character.codePointAt(0) ?? 0)) >>> 0
  return SWATCHES[hash % SWATCHES.length]
}

export function initials(name: string) {
  const words = name.trim().split(/\s+/).filter(Boolean)
  if (words.length === 0) return '?'
  if (words.length === 1) return [...words[0]][0].toLocaleUpperCase()
  return ([...words[0]][0] + [...words[words.length - 1]][0]).toLocaleUpperCase()
}

/** Map the server's actor kinds onto the design's identity kinds. */
export function actorKind(kind: string): ActorKind {
  if (kind === 'external_agent') return 'agent'
  if (kind === 'execution_worker') return 'worker'
  if (kind === 'integration' || kind === 'system') return 'app'
  return 'person'
}

/**
 * Where an account's avatar image is, by account id (C2.17). The workspace
 * provides it from its avatar list, so every avatar shown with an id finds
 * its picture without each list carrying the address.
 */
export const AvatarSources = createContext<(accountId: string) => string | undefined>(() => undefined)

/**
 * Whether an account is online now, as a hook (C2.16). The session provides
 * it from the live connection's presence; an avatar asked to show presence
 * calls it with its account id.
 */
export const AvatarPresence = createContext<(accountId: string | undefined) => 'online' | undefined>(() => undefined)
