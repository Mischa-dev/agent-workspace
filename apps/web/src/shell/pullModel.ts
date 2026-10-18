const MAX_PULL = 110

/** Dampens the pull so it feels like a spring: easy at first, then heavier. */
export function pullDistance(dy: number) {
  if (dy <= 0) return 0
  return Math.min(MAX_PULL, dy * 0.5)
}
