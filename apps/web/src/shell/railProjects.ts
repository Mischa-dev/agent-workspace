import type { Project } from '../projects/types'

function projectRecency(project: Project) {
  const value = project.navigation.lastOpenedAt
  return value ? Date.parse(value) || 0 : 0
}

const byName = (left: Project, right: Project) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id)

/** Placed pins by their place; the rest keep the server's order (when they were pinned). */
const byPlace = (left: Project, right: Project) =>
  (left.navigation.railPosition ?? Number.MAX_SAFE_INTEGER) - (right.navigation.railPosition ?? Number.MAX_SAFE_INTEGER)

/** The project `id` is inside, among the ones listed for you. */
function listedParent(projects: Project[], id: string) {
  const parentId = projects.find((project) => project.id === id)?.parentId
  return parentId ? projects.find((project) => project.id === parentId) : undefined
}

/**
 * The top project a sub-project sits under, as far up as you can see; the
 * project itself when it is inside nothing.
 */
export function topProjectId(projects: Project[], id: string | null): string | null {
  let top = id
  // A tree is at most a few levels deep; the bound only guards odd data.
  for (let step = 0; top && step < 8; step += 1) {
    const parent = listedParent(projects, top)
    if (!parent) break
    top = parent.id
  }
  return top
}

/** The projects straight inside `parentId`, by name. */
export function projectsInside(projects: Project[], parentId: string) {
  return projects.filter((project) => project.parentId === parentId && !project.archivedAt).sort(byName)
}

/**
 * The rail's projects: pins first, in the order you dragged them into (or
 * pinned them), then the five opened most recently. Those five sit in name
 * order, so opening one doesn't shuffle the tiles under your pointer. A
 * sub-project has no tile of its own unless you pin it: it is reached from
 * the project it is inside, whose tile stands for it.
 */
export function railProjects(projects: Project[], currentProjectId: string | null) {
  const pinned = projects.filter((project) => project.navigation.pinned).sort(byPlace)
  const recent = projects
    .filter((project) => !project.navigation.pinned && !listedParent(projects, project.id))
    .sort((left, right) => projectRecency(right) - projectRecency(left))
    .slice(0, 5)
    .sort(byName)
  const visible = [...pinned, ...recent]
  const active = projects.find((project) => project.id === topProjectId(projects, currentProjectId))
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

/**
 * The whole pinned order to save after reordering one workspace's rail: its
 * new order, then every other pin in the order it had. The server clears
 * the place of a pin the order leaves out, so leaving another workspace's
 * pins out would lose their order.
 */
export function railOrderKeepingOthers(order: string[], projects: Project[]): string[] {
  const placed = new Set(order)
  const others = projects.filter((project) => project.navigation.pinned && !placed.has(project.id)).sort(byPlace)
  return [...order, ...others.map((project) => project.id)]
}
