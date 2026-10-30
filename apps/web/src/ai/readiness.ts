import type { Readiness, ReadinessCheck } from './api'

/**
 * The checks every one of these tasks misses for the same reason, in the
 * order the server lists them; none for fewer than two tasks. A check whose
 * wording differs by task ("Waiting on AWD-3") stays on each row.
 */
export function sharedMissingChecks(readinesses: Readiness[]): ReadinessCheck[] {
  if (readinesses.length < 2) return []
  const [first, ...rest] = readinesses
  return first.checks.filter(
    (check) => !check.ok && rest.every((other) => other.checks.some((item) => item.id === check.id && item.label === check.label && !item.ok)),
  )
}
