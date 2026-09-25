/** Identity helpers: how people and agents get their colors, initials and kinds. */

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
