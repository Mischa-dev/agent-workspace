import { useEffect, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { usePageTitle } from '../shell/documentTitleModel'
import { CreateWorkItemDialog } from './CreateWorkItemDialog'
import { useWorkItem, useWorkItemWorkflow } from './hooks'
import { workItemHref } from './links'
import { TaskDetail } from './TaskDetail'
import './work.css'

/** A task on its own page: the same parts as the side panel, with its fields in a rail beside the text when there's room. */
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
