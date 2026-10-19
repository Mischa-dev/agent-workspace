import type { Project } from '../projects/types'

function projectRecency(project: Project) {
  const value = project.navigation.lastOpenedAt
  return value ? Date.parse(value) || 0 : 0
}

const byName = (left: Project, right: Project) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id)

/**
 * The rail's projects: pins first, in the order they were pinned, then the
 * five opened most recently. Those five sit in name order, so opening one
 * doesn't shuffle the tiles under your pointer.
 */
export function railProjects(projects: Project[], currentProjectId: string | null) {
  const pinned = projects.filter((project) => project.navigation.pinned)
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
