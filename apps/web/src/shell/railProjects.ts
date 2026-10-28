import type { Project } from '../projects/types'

function projectRecency(project: Project) {
  const value = project.navigation.lastOpenedAt
  return value ? Date.parse(value) || 0 : 0
}

const byName = (left: Project, right: Project) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id)

/** Placed pins by their place; the rest keep the server's order (when they were pinned). */
const byPlace = (left: Project, right: Project) =>
  (left.navigation.railPosition ?? Number.MAX_SAFE_INTEGER) - (right.navigation.railPosition ?? Number.MAX_SAFE_INTEGER)

/**
 * The rail's projects: pins first, in the order you dragged them into (or
 * pinned them), then the five opened most recently. Those five sit in name
 * order, so opening one doesn't shuffle the tiles under your pointer.
 */
export function railProjects(projects: Project[], currentProjectId: string | null) {
  const pinned = projects.filter((project) => project.navigation.pinned).sort(byPlace)
  const recent = projects
    .filter((project) => !project.navigation.pinned)
    .sort((left, right) => projectRecency(right) - projectRecency(left))
    .slice(0, 5)
    .sort(byName)
  const visible = [...pinned, ...recent]
  const active = projects.find((project) => project.id === currentProjectId)
  if (active && !visible.some((project) => project.id === active.id)) visible.push(active)
  return visible
}

/**
 * Your pinned order after dropping a tile before or after another one. A tile
 * dragged into the pins is pinned there; dropped among the recent ones, it
 * joins the end of the pins. Null when nothing moved.
 */
export function railOrderAfterDrop(visible: Project[], draggedId: string, target: { id: string; after: boolean }): string[] | null {
  if (target.id === draggedId) return null
  const before = visible.filter((project) => project.navigation.pinned).map((project) => project.id)
  const order = before.filter((id) => id !== draggedId)
  const at = order.indexOf(target.id)
  if (at < 0) order.push(draggedId)
  else order.splice(at + (target.after ? 1 : 0), 0, draggedId)
  return order.join() === before.join() ? null : order
}

/** Your pinned order with one project moved a place up or down, or null at the end of the pins. */
export function railOrderAfterStep(visible: Project[], projectId: string, step: -1 | 1): string[] | null {
  const order = visible.filter((project) => project.navigation.pinned).map((project) => project.id)
  const at = order.indexOf(projectId)
  const to = at + step
  if (at < 0 || to < 0 || to >= order.length) return null
  order.splice(at, 1)
  order.splice(to, 0, projectId)
  return order
}
