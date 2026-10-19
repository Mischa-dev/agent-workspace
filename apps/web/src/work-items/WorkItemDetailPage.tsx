import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { usePageTitle } from '../shell/documentTitleModel'
import { Icon } from '../ui'
import { CreateWorkItemDialog } from './CreateWorkItemDialog'
import { useWorkItem, useWorkItemWorkflow } from './hooks'
import { workItemHref, workItemsHref } from './links'
import { TaskDetail } from './TaskDetail'
import './work.css'

/** A task on its own page: the same parts as the side panel, given the whole width. */
export function WorkItemDetailPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const { projectId = '', workItemId = '' } = useParams()
  const project = useProject(projectId)
  const item = useWorkItem(projectId, workItemId)
  const workflow = useWorkItemWorkflow(projectId)
  // C and the command menu's "Create task" open the dialog over this page instead of leaving it.
  const [parameters, setParameters] = useSearchParams()
  const createRequested = parameters.get('create') === 'task'
  const [createOpen, setCreateOpen] = useState(false)
  const [handledCreate, setHandledCreate] = useState(false)
  if (createRequested !== handledCreate) {
    setHandledCreate(createRequested)
    if (createRequested) setCreateOpen(true)
  }
  useEffect(() => {
    if (!createRequested) return
    const next = new URLSearchParams(parameters)
    next.delete('create')
    setParameters(next, { replace: true })
  }, [createRequested, parameters, setParameters])

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
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  // Viewers and workers read tasks but don't make them.
  const canCreate = !project.data.archivedAt && ['owner', 'admin', 'member'].includes(project.data.role)
  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide">
        <NavigationSyncAlert {...navigationSync} />
        <nav aria-label="Breadcrumb" className="aw-crumbs">
          <Link className="aw-crumbs__link" to={workItemsHref(projectId)}>
            <Icon name="work" size={14} />
            Tasks
          </Link>
          <Icon className="aw-crumbs__sep" name="chevron-right" size={14} />
          <span aria-current="page" className="aw-crumbs__here">
            {item.data?.key ?? 'Task'}
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
      {createOpen && workflow.data && canCreate ? (
        <CreateWorkItemDialog
          onClose={() => setCreateOpen(false)}
          onCreated={(id) => {
            setCreateOpen(false)
            navigate(workItemHref(projectId, id))
          }}
          projectId={projectId}
          workflow={workflow.data}
        />
      ) : null}
    </AppShell>
  )
}
