import { Link, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { Icon } from '../ui'
import { useWorkItem } from './hooks'
import { workItemsHref } from './links'
import { TaskDetail } from './TaskDetail'
import './work.css'

/** A work item on its own page: the same parts as the side panel, given the whole width. */
export function WorkItemDetailPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const { projectId = '', workItemId = '' } = useParams()
  const project = useProject(projectId)
  const item = useWorkItem(projectId, workItemId)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'work_items' },
    project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading work item" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide">
        <NavigationSyncAlert {...navigationSync} />
        <nav aria-label="Breadcrumb" className="aw-crumbs">
          <Link className="aw-crumbs__link" to={workItemsHref(projectId)}>
            <Icon name="work" size={14} />
            Work items
          </Link>
          <Icon className="aw-crumbs__sep" name="chevron-right" size={14} />
          <span aria-current="page" className="aw-crumbs__here">
            {item.data?.key ?? 'Work item'}
          </span>
        </nav>
        <TaskDetail
          key={workItemId}
          layout="page"
          onOpenTask={(task) => navigate(task.href)}
          projectArchived={Boolean(project.data.archivedAt)}
          projectId={projectId}
          taskId={workItemId}
        />
      </div>
    </AppShell>
  )
}
