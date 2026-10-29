import type { Readiness, ReadinessCheck } from './api'

/** The checks every one of these tasks misses, in the order the server lists them; none for fewer than two tasks. */
export function sharedMissingChecks(readinesses: Readiness[]): ReadinessCheck[] {
  if (readinesses.length < 2) return []
  const [first, ...rest] = readinesses
  return first.checks.filter((check) => !check.ok && rest.every((other) => other.checks.some((item) => item.id === check.id && !item.ok)))
}
