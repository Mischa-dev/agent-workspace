import { useEffect, useMemo, useRef, useState, type FormEvent, type RefObject } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import {
  Avatar,
  Banner,
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  MessageSkeleton,
  PageHeader,
  SegmentedControl,
  SelectField,
  TextArea,
  TextField,
  actorKind,
} from '../ui'
import { useCreateWorkItem, useWorkItemBoard, useWorkItems, useWorkItemWorkflow } from './hooks'
import { workItemPriorities } from './constants'
import { workItemHref } from './links'
import type { WorkItemFilters, WorkItemPriority } from './types'
import { WorkItemBoard as WorkItemBoardProjection } from './WorkItemBoard'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

function filtersFromParameters(parameters: URLSearchParams): WorkItemFilters {
  const priority = parameters.get('priority')
  return {
    q: parameters.get('q') || undefined,
    typeId: parameters.get('typeId') || undefined,
    statusId: parameters.get('statusId') || undefined,
    priority: workItemPriorities.some((item) => item.value === priority) ? (priority as WorkItemPriority) : undefined,
    archived: parameters.get('archived') === 'true' || undefined,
    cursor: parameters.get('cursor') || undefined,
    limit: 50,
  }
}

function persistedFilters(filters: WorkItemFilters) {
  return Object.fromEntries(
    Object.entries({
      q: filters.q,
      typeId: filters.typeId,
      statusId: filters.statusId,
      semanticCategory: filters.semanticCategory,
      priority: filters.priority,
      assigneeActorId: filters.assigneeActorId,
      unassigned: filters.unassigned,
      archived: filters.archived,
    }).filter(([, value]) => value !== undefined),
  )
}

function parametersFromFilters(filters: Record<string, unknown>) {
  const parameters = new URLSearchParams()
  for (const key of ['q', 'typeId', 'statusId', 'priority', 'archived', 'view']) {
    const value = filters[key]
    if (typeof value === 'string' && value) parameters.set(key, value)
    if (value === true) parameters.set(key, 'true')
  }
  return parameters
}

export function WorkItemsPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const [parameters, setParameters] = useSearchParams()
  const [createOpen, setCreateOpen] = useState(false)
  const createButtonRef = useRef<HTMLButtonElement>(null)
  const view = parameters.get('view') === 'board' ? 'board' : 'list'
  const project = useProject(projectId)
  const workflow = useWorkItemWorkflow(projectId)
  const filters = useMemo(() => filtersFromParameters(parameters), [parameters])
  const list = useWorkItems(projectId, filters)
  const board = useWorkItemBoard(projectId, filters, view === 'board')
  const savedFilters = project.data?.navigation.filters
  const shouldRestore =
    parameters.size === 0 &&
    project.isSuccess &&
    project.data.navigation.location.kind === 'work_items' &&
    Boolean(savedFilters && Object.keys(savedFilters).length > 0)

  useEffect(() => {
    if (shouldRestore && savedFilters) {
      setParameters(parametersFromFilters(savedFilters), { replace: true })
    }
  }, [savedFilters, setParameters, shouldRestore])

  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'work_items' },
    !shouldRestore && project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
    { ...persistedFilters(filters), view },
  )

  const setFilter = (key: string, value: string | boolean) => {
    const next = new URLSearchParams(parameters)
    next.delete('cursor')
    if (value === '' || value === false) next.delete(key)
    else next.set(key, String(value))
    setParameters(next)
  }

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

  const hasFilters = Object.keys(persistedFilters(filters)).length > 0
  const archived = Boolean(project.data.archivedAt)
  const newButton = archived ? null : (
    <Button icon="plus" onClick={() => setCreateOpen(true)} ref={createButtonRef} variant="primary">
      New work item
    </Button>
  )
  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide">
        <NavigationSyncAlert {...navigationSync} />
        <PageHeader actions={newButton} icon="work" title="Work items">
          Everything that needs doing in {project.data.name}, newest first.
        </PageHeader>

        <div aria-label="Filter work items" className="aw-worktools" role="search">
          <TextField
            aria-label="Search work items"
            className="aw-worktools__search"
            maxLength={120}
            onChange={(event) => setFilter('q', event.target.value)}
            placeholder="Search by title or key"
            prefix={<Icon name="search" size={14} />}
            type="search"
            value={filters.q ?? ''}
          />
          <SelectField aria-label="Type" onChange={(event) => setFilter('typeId', event.target.value)} value={filters.typeId ?? ''}>
            <option value="">All types</option>
            {workflow.data?.types.map((type) => (
              <option key={type.id} value={type.id}>
                {type.name}
              </option>
            ))}
          </SelectField>
          <SelectField aria-label="Status" onChange={(event) => setFilter('statusId', event.target.value)} value={filters.statusId ?? ''}>
            <option value="">All statuses</option>
            {workflow.data?.statuses.map((status) => (
              <option key={status.id} value={status.id}>
                {status.name}
              </option>
            ))}
          </SelectField>
          <SelectField aria-label="Priority" onChange={(event) => setFilter('priority', event.target.value)} value={filters.priority ?? ''}>
            <option value="">All priorities</option>
            {workItemPriorities.map((item) => (
              <option key={item.value} value={item.value}>
                {item.label}
              </option>
            ))}
          </SelectField>
          <Checkbox checked={Boolean(filters.archived)} label="Archived" onChange={(checked) => setFilter('archived', checked)} />
          <span className="aw-worktools__spacer" />
          <SegmentedControl
            label="View"
            onChange={(next) => setFilter('view', next === 'board' ? 'board' : '')}
            options={[
              { value: 'list', label: 'List', icon: 'list' },
              { value: 'board', label: 'Board', icon: 'board' },
            ]}
            value={view}
          />
        </div>

        {archived && (
          <Banner className="aw-page__banner" icon="archive">
            This Project is archived, so its work items can’t change.
          </Banner>
        )}

        {view === 'list' && list.isPending ? <MessageSkeleton label="Loading work items" rows={3} /> : null}
        {view === 'list' && list.isError ? <WorkItemsError onRetry={() => void list.refetch()} /> : null}
        {view === 'list' && list.isSuccess && list.data.items.length === 0 ? (
          <EmptyState action={hasFilters ? null : newButton} icon="work" title={hasFilters ? 'No matching work items' : 'No work items yet'}>
            {hasFilters
              ? 'Try a different search, or clear some filters.'
              : 'Write down what needs doing, then hand it to a person or an agent.'}
          </EmptyState>
        ) : null}
        {view === 'list' && list.isSuccess && list.data.items.length > 0 ? (
          <ul aria-label="Work items" className="aw-worklist">
            {list.data.items.map((item) => (
              <li key={item.id}>
                <Link className="aw-worklist__row" to={workItemHref(projectId, item.id)}>
                  <PriorityIcon priority={item.priority} />
                  <span className="aw-worklist__key">{item.key}</span>
                  <StatusIcon category={item.status.semanticCategory} label={item.status.name} />
                  <span className="aw-worklist__title">{item.title}</span>
                  {item.archivedAt ? <span className="aw-worklist__tag">Archived</span> : null}
                  <span className="aw-worklist__type">{item.itemType.name}</span>
                  {item.assignee ? (
                    <Avatar
                      kind={actorKind(item.assignee.kind)}
                      label
                      name={item.assignee.displayName}
                      seed={item.assignee.actorId}
                      size={22}
                    />
                  ) : (
                    <span aria-label="Unassigned" className="aw-worklist__nobody" role="img">
                      <Icon name="unassigned" size={16} />
                    </span>
                  )}
                </Link>
              </li>
            ))}
          </ul>
        ) : null}

        {view === 'list' && list.isSuccess && (filters.cursor || list.data.nextCursor) ? (
          <nav aria-label="Work item pages" className="aw-pager">
            <Button disabled={!filters.cursor} icon="arrow-up" onClick={() => setFilter('cursor', '')} size="sm">
              Back to newest
            </Button>
            <Button disabled={!list.data.nextCursor} iconEnd="arrow-down" onClick={() => setFilter('cursor', list.data.nextCursor ?? '')} size="sm">
              Older
            </Button>
          </nav>
        ) : null}
        {view === 'board' && board.isPending ? <MessageSkeleton label="Loading the board" rows={3} /> : null}
        {view === 'board' && board.isError ? <WorkItemsError onRetry={() => void board.refetch()} /> : null}
        {view === 'board' && board.isSuccess ? (
          <WorkItemBoardProjection
            columns={board.data.columns}
            disabled={archived}
            projectId={projectId}
            statuses={workflow.data?.statuses ?? []}
          />
        ) : null}
      </div>
      {createOpen && workflow.data ? (
        <CreateWorkItemDialog
          onClose={() => setCreateOpen(false)}
          projectId={projectId}
          returnFocusRef={createButtonRef}
          workflow={workflow.data}
        />
      ) : null}
    </AppShell>
  )
}

function CreateWorkItemDialog({
  onClose,
  projectId,
  returnFocusRef,
  workflow,
}: {
  onClose: () => void
  projectId: string
  returnFocusRef: RefObject<HTMLButtonElement | null>
  workflow: NonNullable<ReturnType<typeof useWorkItemWorkflow>['data']>
}) {
  const navigate = useNavigate()
  const mutation = useCreateWorkItem()
  const idempotencyKey = useRef(crypto.randomUUID())
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [typeId, setTypeId] = useState(workflow.types.find((item) => item.isDefault)?.id ?? workflow.types[0]?.id ?? '')
  const [priority, setPriority] = useState<WorkItemPriority>('no_priority')

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || mutation.isPending) return
    mutation.mutate(
      {
        projectId,
        title: title.trim(),
        description,
        typeId: typeId || undefined,
        priority,
        idempotencyKey: idempotencyKey.current,
      },
      {
        onSuccess: (item) => {
          idempotencyKey.current = crypto.randomUUID()
          navigate(workItemHref(projectId, item.id))
        },
      },
    )
  }

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={mutation.isPending} disabled={!title.trim()} type="submit" variant="primary">
            Create work item
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      returnFocus={returnFocusRef}
      title="New work item"
      width={560}
    >
      <TextField
        data-autofocus
        id="new-work-item-title"
        label="Title"
        maxLength={240}
        onChange={(event) => setTitle(event.target.value)}
        placeholder="What needs doing?"
        required
        value={title}
      />
      <TextArea
        id="new-work-item-description"
        label="Description"
        maxLength={100000}
        onChange={(event) => setDescription(event.target.value)}
        placeholder="Add the outcome you want, any limits, and useful links."
        rows={5}
        value={description}
      />
      <div className="aw-form__grid">
        <SelectField id="new-work-item-type" label="Type" onChange={(event) => setTypeId(event.target.value)} value={typeId}>
          {workflow.types.map((type) => (
            <option key={type.id} value={type.id}>
              {type.name}
            </option>
          ))}
        </SelectField>
        <SelectField
          id="new-work-item-priority"
          label="Priority"
          onChange={(event) => setPriority(event.target.value as WorkItemPriority)}
          prefix={<PriorityIcon decorative priority={priority} />}
          value={priority}
        >
          {workItemPriorities.map((item) => (
            <option key={item.value} value={item.value}>
              {item.label}
            </option>
          ))}
        </SelectField>
      </div>
      {mutation.isError ? <Banner tone="danger">{mutation.error.message}</Banner> : null}
    </Dialog>
  )
}

function WorkItemsError({ onRetry }: { onRetry: () => void }) {
  return (
    <ErrorState action={<Button onClick={onRetry}>Try again</Button>} title="Work items didn’t load">
      Check your connection and try again.
    </ErrorState>
  )
}
