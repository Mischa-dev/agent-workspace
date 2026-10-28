import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience } from '../experience/layout'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { WorkTabs } from '../shell/ProjectTabs'
import { Banner, Button, ErrorState, ListSkeleton, PageHeader, SegmentedControl } from '../ui'
import { BoardSkeleton } from './BoardSkeleton'
import { CreateWorkItemDialog } from './CreateWorkItemDialog'
import { FilterBar } from './FilterBar'
import { useWorkItemWorkflow } from './hooks'
import { activeFilterKeys, parametersFromRemembered, rememberedState } from './listModel'
import { notify } from './notice'
import { TaskDetail } from './TaskDetail'
import { useListState } from './useListState'
import { WorkItemList } from './WorkItemList'
import './work.css'

const ViewsMenu = lazy(() => import('./ViewsMenu'))
// Managing labels loads when someone opens it.
const LabelManager = lazy(() => import('./LabelManager').then((module) => ({ default: module.LabelManager })))
// The board is its own chunk: most visits open the list.
const WorkItemBoard = lazy(() => import('./WorkItemBoard').then((module) => ({ default: module.WorkItemBoard })))

/**
 * Every task in a project as a list or a board, with filters, sorting,
 * grouping and a side panel for the one you're looking at. The whole view,
 * panel included, is in the URL.
 */
export function WorkItemsPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const { state, update, parameters, setParameters } = useListState()
  const [createOpen, setCreateOpen] = useState(false)
  const [managingLabels, setManagingLabels] = useState(false)
  // C and the command menu's "Create task" add ?create=task to whatever view is open.
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
  const experience = useProjectExperience(projectId)
  // A layout without the board shows the list, whatever the link says, and
  // the filters, Views menu and list all go by what's shown.
  const board = experience.shows('area.board')
  const shown = useMemo(() => (board || state.view === 'list' ? state : { ...state, view: 'list' as const }), [board, state])
  const view = shown.view
  const workflow = useWorkItemWorkflow(projectId)
  const savedFilters = project.data?.navigation.filters
  // The remembered view comes back only when the page is entered with an
  // empty URL, once per project. Later, a URL emptied by the person (back to
  // the default view, last filter cleared) stays empty. Arriving with only
  // ?create=task (C from another page) counts as empty.
  const viewParameters = [...parameters.keys()].filter((key) => key !== 'create').length
  const [entry, setEntry] = useState<{ projectId: string; restore: boolean } | null>(null)
  if (project.isSuccess && entry?.projectId !== projectId) {
    setEntry({
      projectId,
      restore:
        viewParameters === 0 &&
        project.data.navigation.location.kind === 'work_items' &&
        Boolean(savedFilters && Object.keys(savedFilters).length > 0),
    })
  } else if (entry?.restore && viewParameters > 0) {
    setEntry({ ...entry, restore: false })
  }
  const shouldRestore = entry?.projectId === projectId && entry.restore && viewParameters === 0

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
        <ProjectPageLoading label="Loading tasks" />
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

  const archived = Boolean(project.data.archivedAt)
  // Viewers and workers read tasks but don't change them.
  const readOnly = archived || !['owner', 'admin', 'member'].includes(project.data.role)
  const hasFilters = activeFilterKeys(state).length > 0 || Boolean(state.filters.q)
  const newButton = readOnly ? null : (
    <Button disabled={!workflow.data} icon="plus" onClick={() => setCreateOpen(true)} ref={createButtonRef} variant="primary">
      New {experience.word('task')}
    </Button>
  )
  const toggleDone = () => update((current) => ({ ...current, showDone: !current.showDone }), { replace: true })
  const headerActions = (
    <>
      {board ? (
        <SegmentedControl
          label="View"
          onChange={(next) => update((current) => ({ ...current, view: next }))}
          options={[
            { value: 'list', label: 'List', icon: 'list' },
            { value: 'board', label: 'Board', icon: 'board' },
          ]}
          value={state.view}
        />
      ) : null}
      <Suspense fallback={null}>
        <ViewsMenu actorId={session.user.actorId} projectId={projectId} readOnly={archived} state={shown} update={update} />
      </Suspense>
      {newButton}
    </>
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
      <div className={view === 'board' ? 'aw-page aw-page--wide aw-workpage aw-workpage--board' : 'aw-page aw-page--wide aw-workpage'}>
        <NavigationSyncAlert {...navigationSync} />
        <PageHeader actions={headerActions} icon="work" title={experience.label('task', 'many')}>
          Everything that needs doing in {project.data.name}.
        </PageHeader>
        <WorkTabs current="items" projectId={projectId} />
        <FilterBar projectId={projectId} state={shown} update={update} workflow={workflow.data} />
        {archived ? (
          <Banner className="aw-page__banner" icon="archive">
            This project is archived, so its tasks can’t change.
          </Banner>
        ) : null}
        {workflow.isPending ? (view === 'board' ? <BoardSkeleton /> : <ListSkeleton label={`Loading ${experience.word('task', 'many')}`} rows={6} />) : null}
        {workflow.isError ? <ErrorState error={workflow.error} onRetry={() => void workflow.refetch()} title={`${experience.label('task', 'many')} didn’t load`} /> : null}
        {workflow.data && view === 'list' ? (
          <WorkItemList
            disabled={readOnly}
            emptyAction={newButton}
            hasFilters={hasFilters}
            notify={notify}
            onClearFilters={() => update((current) => ({ ...current, filters: {} }))}
            onClosePanel={closeTask}
            onManageLabels={() => setManagingLabels(true)}
            onOpenTask={openTask}
            onToggleDone={toggleDone}
            openTaskId={state.task}
            projectId={projectId}
            state={shown}
            workflow={workflow.data}
          />
        ) : null}
        {workflow.data && view === 'board' ? (
          <Suspense fallback={<BoardSkeleton />}>
            <WorkItemBoard
              disabled={readOnly}
              notify={notify}
              onClosePanel={closeTask}
              onManageLabels={() => setManagingLabels(true)}
              onOpenTask={openTask}
              onToggleDone={toggleDone}
              openTaskId={state.task}
              projectId={projectId}
              state={state}
              workflow={workflow.data}
            />
          </Suspense>
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
      {managingLabels ? (
        <Suspense fallback={null}>
          <LabelManager onClose={() => setManagingLabels(false)} projectId={projectId} />
        </Suspense>
      ) : null}
    </AppShell>
  )
}
