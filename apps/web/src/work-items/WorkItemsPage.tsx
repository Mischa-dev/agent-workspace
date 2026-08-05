import {
  FormEvent,
  KeyboardEvent as ReactKeyboardEvent,
  RefObject,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectContextDrawer } from '../projects/ProjectContextDrawer'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import {
  useCreateWorkItem,
  useMoveWorkItem,
  useWorkItemBoard,
  useWorkItems,
  useWorkItemWorkflow,
} from './hooks'
import { workItemPriorities } from './constants'
import type { WorkItemCard, WorkItemFilters, WorkItemPriority, WorkItemStatus, WorkflowStatus } from './types'

function filtersFromParameters(parameters: URLSearchParams): WorkItemFilters {
  const priority = parameters.get('priority')
  return {
    q: parameters.get('q') || undefined,
    typeId: parameters.get('typeId') || undefined,
    statusId: parameters.get('statusId') || undefined,
    priority: workItemPriorities.some((item) => item.value === priority)
      ? (priority as WorkItemPriority)
      : undefined,
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
  const filters = useMemo(
    () => filtersFromParameters(parameters),
    [parameters],
  )
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
      setParameters(parametersFromFilters(savedFilters), {
        replace: true,
      })
    }
  }, [savedFilters, setParameters, shouldRestore])

  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'work_items' },
    !shouldRestore &&
      project.isSuccess &&
      !project.data.archivedAt &&
      project.data.capabilities.navigationWrite,
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
    return <AppShell user={session.user}><WorkItemsLoading /></AppShell>
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <WorkItemsError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const hasFilters = Object.keys(persistedFilters(filters)).length > 0
  return (
    <AppShell
      context={<ProjectContextDrawer project={project.data} />}
      project={project.data}
      user={session.user}
    >
      <div className="page-content work-items-page">
        <NavigationSyncAlert {...navigationSync} />
        <header className="page-heading">
          <div>
            <span className="section-label">{project.data.key} / PLANNING</span>
            <h1>Work Items</h1>
            <p>Durable units of work, ordered newest first.</p>
          </div>
          {!project.data.archivedAt && (
            <button
              className="primary-button"
              onClick={() => setCreateOpen(true)}
              ref={createButtonRef}
              type="button"
            >
              New Work Item
            </button>
          )}
        </header>

        <section aria-label="Work Item filters" className="work-item-filters">
          <label className="search-field">
            <span className="visually-hidden">Search Work Items</span>
            <input
              maxLength={120}
              onChange={(event) => setFilter('q', event.target.value)}
              placeholder="Search title or key"
              type="search"
              value={filters.q ?? ''}
            />
          </label>
          <label>
            <span className="visually-hidden">Type</span>
            <select
              aria-label="Type"
              onChange={(event) => setFilter('typeId', event.target.value)}
              value={filters.typeId ?? ''}
            >
              <option value="">All types</option>
              {workflow.data?.types.map((type) => (
                <option key={type.id} value={type.id}>{type.name}</option>
              ))}
            </select>
          </label>
          <label>
            <span className="visually-hidden">Status</span>
            <select
              aria-label="Status"
              onChange={(event) => setFilter('statusId', event.target.value)}
              value={filters.statusId ?? ''}
            >
              <option value="">All statuses</option>
              {workflow.data?.statuses.map((status) => (
                <option key={status.id} value={status.id}>{status.name}</option>
              ))}
            </select>
          </label>
          <label>
            <span className="visually-hidden">Priority</span>
            <select
              aria-label="Priority"
              onChange={(event) => setFilter('priority', event.target.value)}
              value={filters.priority ?? ''}
            >
              <option value="">All priorities</option>
              {workItemPriorities.map((item) => (
                <option key={item.value} value={item.value}>{item.label}</option>
              ))}
            </select>
          </label>
          <label className="archive-filter">
            <input
              checked={Boolean(filters.archived)}
              onChange={(event) => setFilter('archived', event.target.checked)}
              type="checkbox"
            />
            Archived
          </label>
        </section>

        <nav aria-label="Work Item view" className="work-item-view-switcher">
          <button aria-pressed={view === 'list'} className="text-button" onClick={() => setFilter('view', '')} type="button">List</button>
          <button aria-pressed={view === 'board'} className="text-button" onClick={() => setFilter('view', 'board')} type="button">Board</button>
        </nav>

        {view === 'list' && list.isPending ? <WorkItemsLoading compact /> : null}
        {view === 'list' && list.isError ? <WorkItemsError onRetry={() => void list.refetch()} /> : null}
        {view === 'list' && list.isSuccess && list.data.items.length === 0 ? (
          <div className="empty-state compact-empty-state">
            <span aria-hidden="true" className="empty-state-mark">◇</span>
            <h2>{hasFilters ? 'No matching Work Items' : 'No Work Items yet'}</h2>
            <p>
              {hasFilters
                ? 'Adjust the filters to widen this view.'
                : 'Create the first item to give this Project a durable planning surface.'}
            </p>
          </div>
        ) : null}
        {view === 'list' && list.isSuccess && list.data.items.length > 0 ? (
          <ul aria-label="Work Items" className="work-item-list">
            {list.data.items.map((item) => (
              <li key={item.id}>
                <Link
                  to={`/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(item.id)}`}
                >
                  <span className="work-item-key">{item.key}</span>
                  <span className="work-item-title">{item.title}</span>
                  <span className="work-item-type">{item.itemType.name}</span>
                  <span className="work-item-state" data-category={item.status.semanticCategory}>
                    {item.status.name}
                  </span>
                  <span className="work-item-owner">
                    {item.assignee?.displayName ?? 'Unassigned'}
                  </span>
                  <span className="work-item-priority" data-priority={item.priority}>
                    {workItemPriorities.find((candidate) => candidate.value === item.priority)?.label}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        ) : null}

        {view === 'list' && list.isSuccess && (filters.cursor || list.data.nextCursor) ? (
          <nav aria-label="Work Item pages" className="pagination-actions">
            <button
              className="secondary-button"
              disabled={!filters.cursor}
              onClick={() => setFilter('cursor', '')}
              type="button"
            >
              Newest
            </button>
            <button
              className="secondary-button"
              disabled={!list.data.nextCursor}
              onClick={() => setFilter('cursor', list.data.nextCursor ?? '')}
              type="button"
            >
              Older
            </button>
          </nav>
        ) : null}
        {view === 'board' && board.isPending ? <WorkItemsLoading compact /> : null}
        {view === 'board' && board.isError ? <WorkItemsError onRetry={() => void board.refetch()} /> : null}
        {view === 'board' && board.isSuccess ? (
          <WorkItemBoard
            columns={board.data.columns}
            disabled={Boolean(project.data.archivedAt)}
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

function WorkItemBoard({
  columns,
  disabled,
  projectId,
  statuses,
}: {
  columns: Array<{ status: WorkItemStatus; items: WorkItemCard[]; hasMore: boolean }>
  disabled: boolean
  projectId: string
  statuses: WorkflowStatus[]
}) {
  const move = useMoveWorkItem()
  const [dragged, setDragged] = useState<WorkItemCard | null>(null)
  const submitMove = (item: WorkItemCard, targetStatusId: string, targetPosition: number) => {
    move.mutate({
      projectId,
      workItemId: item.id,
      expectedVersion: item.version,
      targetStatusId,
      targetPosition,
      idempotencyKey: crypto.randomUUID(),
    })
  }
  return (
    <section aria-label="Work Item board" className="work-item-board">
      {columns.map((column) => (
        <section
          aria-label={`${column.status.name} column`}
          className="work-item-board-column"
          data-category={column.status.semanticCategory}
          key={column.status.id}
          onDragOver={(event) => { if (!disabled) event.preventDefault() }}
          onDrop={(event) => {
            event.preventDefault()
            if (dragged && !disabled) submitMove(dragged, column.status.id, column.items.length + 1)
            setDragged(null)
          }}
        >
          <header><h2>{column.status.name}</h2><span>{column.items.length}{column.hasMore ? '+' : ''}</span></header>
          <ol>
            {column.items.map((item, index) => (
              <li draggable={!disabled && !move.isPending} key={item.id} onDragEnd={() => setDragged(null)} onDragStart={() => setDragged(item)}>
                <Link to={`/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(item.id)}`}><span className="work-item-key">{item.key}</span><strong>{item.title}</strong></Link>
                <span className="board-card-meta">{item.itemType.name} · {item.assignee?.displayName ?? 'Unassigned'}</span>
                {!disabled ? (
                  <div className="board-card-actions">
                    <button aria-label={`Move ${item.key} up`} className="text-button" disabled={move.isPending || index === 0} onClick={() => submitMove(item, column.status.id, index)} type="button">↑</button>
                    <button aria-label={`Move ${item.key} down`} className="text-button" disabled={move.isPending || index === column.items.length - 1} onClick={() => submitMove(item, column.status.id, index + 2)} type="button">↓</button>
                    <label><span className="visually-hidden">Move {item.key} to status</span><select aria-label={`Move ${item.key} to status`} disabled={move.isPending} onChange={(event) => { if (event.target.value !== column.status.id) submitMove(item, event.target.value, 1) }} value={column.status.id}>{statuses.map((status) => <option key={status.id} value={status.id}>{status.name}</option>)}</select></label>
                  </div>
                ) : null}
              </li>
            ))}
          </ol>
          {column.items.length === 0 ? <p>Drop or move an item here.</p> : null}
          {column.hasMore ? <p>More items are available in the filtered list.</p> : null}
        </section>
      ))}
      {move.isError ? <p className="form-error-summary board-error" role="alert">The board changed before the move completed. It has been refreshed; try the move again.</p> : null}
    </section>
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
  const dialogRef = useRef<HTMLElement>(null)
  const idempotencyKey = useRef(crypto.randomUUID())
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [typeId, setTypeId] = useState(
    workflow.types.find((item) => item.isDefault)?.id ?? workflow.types[0]?.id ?? '',
  )
  const [priority, setPriority] = useState<WorkItemPriority>('no_priority')

  useEffect(() => {
    const applicationRoot = document.querySelector<HTMLElement>('#root')
    const applicationWasInert = applicationRoot?.hasAttribute('inert') ?? false
    const returnFocus = returnFocusRef.current
    applicationRoot?.setAttribute('inert', '')
    return () => {
      if (!applicationWasInert) applicationRoot?.removeAttribute('inert')
      window.requestAnimationFrame(() => returnFocus?.focus())
    }
  }, [returnFocusRef])

  const handleDialogKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
      return
    }
    if (event.key !== 'Tab') return
    const focusable = Array.from(
      dialogRef.current?.querySelectorAll<HTMLElement>(
        'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ) ?? [],
    ).filter((element) => element.tabIndex >= 0)
    if (!focusable.length) return
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

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
          navigate(
            `/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(item.id)}`,
          )
        },
      },
    )
  }

  return createPortal(
    <div className="modal-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section
        aria-labelledby="new-work-item-title"
        aria-modal="true"
        className="work-item-dialog"
        onKeyDown={handleDialogKeyDown}
        ref={dialogRef}
        role="dialog"
      >
        <header>
          <div>
            <span className="section-label">NEW WORK ITEM</span>
            <h2 id="new-work-item-title">Capture the work</h2>
          </div>
          <button aria-label="Close" className="icon-button" onClick={onClose} type="button">×</button>
        </header>
        <form onSubmit={submit}>
          <label className="field">
            <span>Title</span>
            <input autoFocus maxLength={240} onChange={(event) => setTitle(event.target.value)} required value={title} />
          </label>
          <label className="field">
            <span>Description</span>
            <textarea maxLength={100000} onChange={(event) => setDescription(event.target.value)} rows={6} value={description} />
          </label>
          <div className="field-grid">
            <label className="field">
              <span>Type</span>
              <select onChange={(event) => setTypeId(event.target.value)} value={typeId}>
                {workflow.types.map((type) => <option key={type.id} value={type.id}>{type.name}</option>)}
              </select>
            </label>
            <label className="field">
              <span>Priority</span>
              <select onChange={(event) => setPriority(event.target.value as WorkItemPriority)} value={priority}>
                {workItemPriorities.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
          </div>
          {mutation.isError ? <p className="form-error-summary" role="alert">{mutation.error.message}</p> : null}
          <div className="form-actions">
            <button className="secondary-button" onClick={onClose} type="button">Cancel</button>
            <button className="primary-button" disabled={!title.trim() || mutation.isPending} type="submit">
              {mutation.isPending ? 'Creating…' : 'Create Work Item'}
            </button>
          </div>
        </form>
      </section>
    </div>,
    document.body,
  )
}

function WorkItemsLoading({ compact = false }: { compact?: boolean }) {
  return (
    <div aria-busy="true" className={compact ? 'work-item-list-skeleton' : 'page-content'}>
      <div aria-label="Loading Work Items" className="page-skeleton">
        <span /><span /><span />
      </div>
    </div>
  )
}

function WorkItemsError({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="inline-error" role="alert">
      <p>Work Items could not be loaded.</p>
      <button onClick={onRetry} type="button">Try again</button>
    </div>
  )
}
