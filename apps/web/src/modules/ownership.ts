import switchable from 'virtual:modules/switchable'

// AWD-75: which module owns an entry point, from one kind of the manifests'
// claims (`virtual:modules/slots/<kind>`, built from `modules/*.json`). Each
// part of the app imports only the kinds it asks about, so the first page load
// doesn't carry the palette's or Home's claims.

export type Claims = ReadonlyArray<readonly [pattern: string, module: string]>

/** How well a pattern (exact, or ending in `*`) matches a value; null when it doesn't. */
export function patternScore(pattern: string, value: string): number | null {
  if (pattern.endsWith('*')) {
    const prefix = pattern.slice(0, -1)
    return value.startsWith(prefix) ? prefix.length : null
  }
  return pattern === value ? pattern.length + 1 : null
}

/** The module whose claims hold `value` most specifically (the first such, on a tie), or null. */
export function ownerOf(claims: Claims, value: string): string | null {
  let best: { module: string; score: number } | null = null
  for (const [pattern, module] of claims) {
    const score = patternScore(pattern, value)
    if (score !== null && (!best || score > best.score)) best = { module, score }
  }
  return best?.module ?? null
}

const SWITCHABLE = new Set(switchable)

/** Whether a module can be off for someone (the workspace itself can't). */
export function moduleSwitchable(module: string) {
  return SWITCHABLE.has(module)
}
