import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'
import { hrefFor } from '../app/thingHref'
import { useProjectExperience } from '../experience/layout'
import type { PartId } from '../experience/parts'
import { moduleIcon } from '../modules/builtIcons'
import type { Project } from '../projects/types'
import { SidebarLink } from './SidebarParts'

// M1.7: built modules' sidebar entries, in the space whose collection or page
// each points at. A module is a part of the layout by its key (`module.<key>`,
// see `modulePart`), so a layout or My view can hide it like any other row,
// and a module that's switched off shows nothing. Loaded after the shell.

/** Where an entry goes: a collection or a page in this space. Forms open from their collection. */
function entryHref(target: unknown, spaceId: string): string | null {
  const value = target as { kind?: string; collection?: string; page?: string } | null
  if (value?.kind === 'collection' && value.collection) return hrefFor({ kind: 'collection', id: value.collection, spaceId })
  if (value?.kind === 'page' && value.page) return hrefFor({ kind: 'page', id: value.page, spaceId })
  return null
}

/** A built module's layout part: `module.<name>`, its key without the `local.` every built key starts with. */
function modulePart(key: string): PartId {
  return `module.${key.replace(/^[a-z]+\./, '')}` as PartId
}

export default function BuiltModuleRows({ onNavigate, project }: { onNavigate: () => void; project: Project }) {
  const experience = useProjectExperience(project.id)
  const list = useQuery({ ...opQuery('module_list', { projectId: project.id }, { workspaceId: project.organizationId }), staleTime: 60_000 })
  const rows = (list.data?.built ?? [])
    .filter((module) => module.on && experience.shows(modulePart(module.key)))
    .flatMap((module) =>
      (module.nav ?? []).flatMap((entry) => {
        // An entry shows in the space it points into, and in its module's own
        // space even after what it points at moved elsewhere.
        const here = entry.spaceId === project.id || (module.spaceId === project.id && Boolean(entry.spaceId))
        const to = here && entry.spaceId ? entryHref(entry.target, entry.spaceId) : null
        // A page's entry carries no icon of its own: it wears a page's.
        const icon = entry.icon ? moduleIcon(entry.icon) : (entry.target as { kind?: string } | null)?.kind === 'page' ? 'list' : moduleIcon(module.icon)
        return to ? [{ key: `${module.id}:${entry.id}`, label: entry.label, icon, to }] : []
      }),
    )
  return (
    <>
      {rows.map((row) => (
        <SidebarLink icon={row.icon} key={row.key} label={row.label} onNavigate={onNavigate} to={row.to} />
      ))}
    </>
  )
}
