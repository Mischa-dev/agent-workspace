import { useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { usePageTitle } from '../shell/documentTitleModel'
import { useWorkItem } from './hooks'
import { TaskDetail } from './TaskDetail'
import './work.css'

/** A task on its own page: the same parts as the side panel, with its fields in a rail beside the text when there's room. */
export function WorkItemDetailPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const { projectId = '', workItemId = '' } = useParams()
  const project = useProject(projectId)
  const item = useWorkItem(projectId, workItemId)
  usePageTitle(item.data ? `${item.data.key} ${item.data.title}` : null)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'work_items' },
    project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading task" />
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
      <div className="aw-taskpage">
        <NavigationSyncAlert {...navigationSync} />
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
