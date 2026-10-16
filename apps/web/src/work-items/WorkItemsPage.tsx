import { useCallback, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, ErrorState, MessageSkeleton, PageHeader } from '../ui'
import { CreateWorkItemDialog } from './CreateWorkItemDialog'
import { FilterBar } from './FilterBar'
import { useWorkItemWorkflow } from './hooks'
import { LabelManager } from './LabelManager'
import { activeFilterKeys, parametersFromRemembered, rememberedState } from './listModel'
import { NoticeBar } from './notice'
import { TaskDetail } from './TaskDetail'
import { useListState } from './useListState'
import { useNotice } from './useNotice'
import { WorkItemBoard } from './WorkItemBoard'
import { WorkItemList } from './WorkItemList'
import './work.css'

/**
 * Every work item in a Project as a list or a board, with filters, sorting,
 * grouping and a side panel for the one you're looking at. The whole view,
 * panel included, is in the URL.
 */
export function WorkItemsPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const { state, update, parameters, setParameters } = useListState()
  const [createOpen, setCreateOpen] = useState(false)
  const [managingLabels, setManagingLabels] = useState(false)
  // C and the command menu's "Create task" land here with ?create=task.
  const createRequested = parameters.get('create') === 'task'
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
  const createButtonRef = useRef<HTMLButtonElement>(null)
  const project = useProject(projectId)
  const workflow = useWorkItemWorkflow(projectId)
  const { notice, notify } = useNotice()
  const savedFilters = project.data?.navigation.filters
  // The remembered view comes back only when the page is entered with an
  // empty URL, once per project. Later, a URL emptied by the person (back to
  // the default view, last filter cleared) stays empty.
  const [entry, setEntry] = useState<{ projectId: string; restore: boolean } | null>(null)
  if (project.isSuccess && entry?.projectId !== projectId) {
    setEntry({
      projectId,
      restore:
        parameters.size === 0 &&
        project.data.navigation.location.kind === 'work_items' &&
        Boolean(savedFilters && Object.keys(savedFilters).length > 0),
    })
  } else if (entry?.restore && parameters.size > 0) {
    setEntry({ ...entry, restore: false })
  }
  const shouldRestore = entry?.projectId === projectId && entry.restore && parameters.size === 0

  useEffect(() => {
    if (shouldRestore && savedFilters) setParameters(parametersFromRemembered(savedFilters), { replace: true })
  }, [savedFilters, setParameters, shouldRestore])

  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'work_items' },
    !shouldRestore && project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
    rememberedState(state),
  )

  const openTask = useCallback(
    (id: string) => update((current) => ({ ...current, task: id }), { replace: state.task !== null }),
    [state.task, update],
  )
  const closeTask = useCallback(() => update((current) => ({ ...current, task: null })), [update])

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading work items" />
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

  const archived = Boolean(project.data.archivedAt)
  // Viewers and workers read tasks but don't change them.
  const readOnly = archived || !['owner', 'admin', 'member'].includes(project.data.role)
  const hasFilters = activeFilterKeys(state).length > 0 || Boolean(state.filters.q)
  const newButton = readOnly ? null : (
    <Button disabled={!workflow.data} icon="plus" onClick={() => setCreateOpen(true)} ref={createButtonRef} variant="primary">
      New work item
    </Button>
  )
  const panel = state.task ? (
    <div
      className="aw-taskpanel"
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || event.defaultPrevented) return
        if ((event.target as HTMLElement).closest('input, textarea, select, .aw-popover, [role="dialog"][aria-modal="true"]:not(.aw-panel)')) return
        event.preventDefault()
        closeTask()
      }}
    >
      <TaskDetail
        key={state.task}
        layout="panel"
        onClose={closeTask}
        onOpenTask={(task) => openTask(task.id)}
        projectArchived={archived}
        projectId={projectId}
        taskId={state.task}
      />
    </div>
  ) : undefined

  return (
    <AppShell context={panel} contextKey={state.task ?? undefined} onContextDismiss={closeTask} project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide aw-workpage">
        <NavigationSyncAlert {...navigationSync} />
        <PageHeader actions={newButton} icon="work" title="Work items">
          Everything that needs doing in {project.data.name}.
        </PageHeader>
        <FilterBar projectId={projectId} state={state} update={update} workflow={workflow.data} />
        {archived ? (
          <Banner className="aw-page__banner" icon="archive">
            This Project is archived, so its work items can’t change.
          </Banner>
        ) : null}
        <NoticeBar notice={notice} onDismiss={() => notify(null)} />
        {workflow.isPending ? <MessageSkeleton label="Loading work items" rows={3} /> : null}
        {workflow.isError ? (
          <ErrorState action={<Button onClick={() => void workflow.refetch()}>Try again</Button>} title="Work items didn’t load">
            Check your connection and try again.
          </ErrorState>
        ) : null}
        {workflow.data && state.view === 'list' ? (
          <WorkItemList
            disabled={readOnly}
            emptyAction={newButton}
            hasFilters={hasFilters}
            notify={notify}
            onClearFilters={() => update((current) => ({ ...current, filters: {} }))}
            onClosePanel={closeTask}
            onManageLabels={() => setManagingLabels(true)}
            onOpenTask={openTask}
            openTaskId={state.task}
            projectId={projectId}
            state={state}
            workflow={workflow.data}
          />
        ) : null}
        {workflow.data && state.view === 'board' ? (
          <WorkItemBoard
            disabled={readOnly}
            notify={notify}
            onClosePanel={closeTask}
            onManageLabels={() => setManagingLabels(true)}
            onOpenTask={openTask}
            openTaskId={state.task}
            projectId={projectId}
            state={state}
            workflow={workflow.data}
          />
        ) : null}
      </div>
      {createOpen && workflow.data ? (
        <CreateWorkItemDialog
          onClose={() => setCreateOpen(false)}
          onCreated={(id) => {
            setCreateOpen(false)
            openTask(id)
          }}
          projectId={projectId}
          returnFocusRef={createButtonRef}
          workflow={workflow.data}
        />
      ) : null}
      {managingLabels ? <LabelManager onClose={() => setManagingLabels(false)} projectId={projectId} /> : null}
    </AppShell>
  )
}
