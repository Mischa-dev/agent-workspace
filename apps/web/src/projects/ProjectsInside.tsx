import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { here, spaceHref } from '../app/href'
import { Banner, ButtonLink, Icon, Row, RowList, Section, countLabel } from '../ui'
import type { Project } from './types'

/**
 * The projects inside this one, each with its open tasks, and the count for
 * the whole tree. Shown only where there are any, so a plain project's
 * overview stays as it was.
 */
export default function ProjectsInside({ project, tasks }: { project: Project; tasks: boolean }) {
  const tree = useQuery({
    queryKey: queryKeys.projectTree(project.id),
    queryFn: () => callOp('space_tree_get', { spaceId: project.id }, { workspaceId: project.organizationId }),
  })
  // Most projects have none, so nothing holds a place while it loads.
  if (tree.isPending) return null
  if (tree.isError) {
    return (
      <Banner onRetry={() => void tree.refetch()} tone="danger">
        The projects inside this one didn’t load.
      </Banner>
    )
  }
  const inside = tree.data.inside
  if (!inside.length) return null
  const all = tree.data.tasksWithInside
  const canAdd = project.capabilities.manage && !project.archivedAt
  return (
    <Section
      actions={
        canAdd ? (
          <ButtonLink icon="plus" size="sm" variant="ghost" to={here(`/projects/new?inside=${encodeURIComponent(project.id)}`)}>
            New project inside
          </ButtonLink>
        ) : null
      }
      description={tasks ? `${countLabel(all.open, 'open task')} across ${project.name} and the projects inside it.` : undefined}
      id="projects-inside-heading"
      title="Projects inside"
    >
      <RowList label="Projects inside">
        {inside.map((child) => (
          <Row
            actions={<Icon className="aw-row__chev" name="chevron-right" />}
            className="aw-row--link"
            key={child.id}
            leading={<Icon className="aw-row__glyph" name="layers" />}
            meta={[
              child.depth > 1 ? `Inside ${inside.find((other) => other.id === child.parentId)?.name ?? 'another project'}` : null,
              tasks && child.tasks ? countLabel(child.tasks.open, 'open task') : null,
              child.peopleFromParent ? null : 'Its own people',
            ]
              .filter(Boolean)
              .join(' · ')}
            title={
              <Link className="aw-row__link" to={spaceHref(child.id)}>
                {child.icon ? `${child.icon} ${child.name}` : child.name}
              </Link>
            }
          />
        ))}
      </RowList>
    </Section>
  )
}
