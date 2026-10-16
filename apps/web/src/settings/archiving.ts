/**
 * Projects being archived from their General page right now. Settings would
 * otherwise see the archived Project and send the person to its overview
 * before the page can take them Home.
 */
const archiving = new Set<string>()

export function markArchiving(projectId: string, active: boolean) {
  if (active) archiving.add(projectId)
  else archiving.delete(projectId)
}

export function isArchiving(projectId: string) {
  return archiving.has(projectId)
}
