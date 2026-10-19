import { useQueries } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { environmentsQuery } from '../../launch/hooks'
import type { Project } from '../../projects/types'
import { EmptyState, Row, RowList } from '../../ui'
import { projectSettingsHref } from '../paths'

/** The most projects whose environments are counted here; the rest are listed without. */
const COUNTED = 20

/**
 * Environments are set up per project, with its repository, so the workspace
 * lists its projects and what each one has, linking to where they're managed.
 */
export function WorkspaceEnvironments({ projects }: { projects: Project[] }) {
  const lists = useQueries({ queries: projects.slice(0, COUNTED).map((project) => environmentsQuery(project.id)) })
  if (!projects.length) {
    return (
      <EmptyState
        action={
          <Link className="aw-btn aw-btn--secondary aw-btn--md" to="/projects/new">
            <span className="aw-btn__label">Create a project</span>
          </Link>
        }
        icon="external"
        title="No projects yet"
      >
        Environments are set up in each project, so they appear here once there is one.
      </EmptyState>
    )
  }
  return (
    <RowList label="Environments by project">
      {projects.map((project, index) => {
        const list = lists[index]?.data
        const fallback = list?.environments.find((environment) => environment.isDefault)
        return (
          <Row
            actions={
              <Link
                aria-label={`Open ${project.name}’s environments`}
                className="aw-btn aw-btn--ghost aw-btn--sm"
                to={`${projectSettingsHref(project.id, 'runs')}#environments`}
              >
                <span className="aw-btn__label">Open</span>
              </Link>
            }
            key={project.id}
            meta={
              !list
                ? null
                : list.environments.length === 0
                  ? 'None yet, so runs work on this server'
                  : `${list.environments.length === 1 ? '1 environment' : `${list.environments.length} environments`}${fallback ? ` · Default: ${fallback.name}` : ''}`
            }
            title={project.name}
          />
        )
      })}
    </RowList>
  )
}
