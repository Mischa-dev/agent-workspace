import { FormEvent, useMemo, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { normalizeWorkItem } from './api'
import { workItemPriorities } from './constants'
import {
  useAssignableActors,
  useSetWorkItemArchived,
  useUpdateWorkItem,
  useWorkItem,
  useWorkItemWorkflow,
} from './hooks'
import type { WorkItem, WorkItemPriority } from './types'

type Draft = {
  title: string
  description: string
  typeId: string
  statusId: string
  priority: WorkItemPriority
  assigneeActorId: string
}

function draftFromItem(item: WorkItem): Draft {
  return {
    title: item.title,
    description: item.description,
    typeId: item.itemType.id,
    statusId: item.status.id,
    priority: item.priority,
    assigneeActorId: item.assignee?.actorId ?? '',
  }
}

function conflictItem(error: unknown): WorkItem | null {
  if (!isApiError(error) || error.code !== 'version_conflict') return null
  const details = error.details
  if (!details || typeof details !== 'object' || !('current' in details)) return null
  return normalizeWorkItem((details as { current: unknown }).current)
}

export function WorkItemDetailPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', workItemId = '' } = useParams()
  const project = useProject(projectId)
  const itemQuery = useWorkItem(projectId, workItemId)
  const workflow = useWorkItemWorkflow(projectId)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'work_items' },
    project.isSuccess &&
      !project.data.archivedAt &&
      project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

  if (project.isPending || itemQuery.isPending || workflow.isPending) {
    return <AppShell user={session.user}><DetailLoading /></AppShell>
  }
  if (project.isError || itemQuery.isError || workflow.isError) {
    return (
      <AppShell user={session.user}>
        <div className="page-content"><div className="error-state" role="alert">
          <h1>Work Item unavailable</h1>
          <p>It does not exist, is no longer accessible, or could not be loaded.</p>
          <button onClick={() => void Promise.all([project.refetch(), itemQuery.refetch(), workflow.refetch()])} type="button">Try again</button>
        </div></div>
      </AppShell>
    )
  }

  return (
    <AppShell
      context={<WorkItemContext item={itemQuery.data} />}
      project={project.data}
      user={session.user}
    >
      <div className="page-content work-item-detail">
        <NavigationSyncAlert {...navigationSync} />
        <Link className="back-link" to={`/projects/${encodeURIComponent(projectId)}/work-items`}>
          ← Work Items
        </Link>
        <WorkItemEditor
          item={itemQuery.data}
          projectArchived={Boolean(project.data.archivedAt)}
          workflow={workflow.data}
        />
      </div>
    </AppShell>
  )
}

function WorkItemEditor({
  item,
  projectArchived,
  workflow,
}: {
  item: WorkItem
  projectArchived: boolean
  workflow: NonNullable<ReturnType<typeof useWorkItemWorkflow>['data']>
}) {
  const update = useUpdateWorkItem()
  const archive = useSetWorkItemArchived()
  const [draft, setDraft] = useState(() => draftFromItem(item))
  const [base, setBase] = useState(item)
  const [conflict, setConflict] = useState<WorkItem | null>(null)
  const [assigneeQuery, setAssigneeQuery] = useState('')
  const assignees = useAssignableActors(item.projectId, assigneeQuery)
  const idempotencyKey = useRef(crypto.randomUUID())
  const archiveKey = useRef(crypto.randomUUID())

  const allowedStatuses = useMemo(() => {
    const allowed = new Set(
      workflow.transitions
        .filter((transition) => transition.fromStatusId === base.status.id)
        .map((transition) => transition.toStatusId),
    )
    allowed.add(base.status.id)
    return workflow.statuses.filter((status) => allowed.has(status.id))
  }, [base.status.id, workflow])
  const dirty = JSON.stringify(draft) !== JSON.stringify(draftFromItem(base))
  const disabled = projectArchived || Boolean(base.archivedAt) || !base.capabilities.edit

  const save = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!dirty || update.isPending) return
    setConflict(null)
    update.mutate(
      {
        projectId: base.projectId,
        workItemId: base.id,
        idempotencyKey: idempotencyKey.current,
        patch: {
          expectedVersion: base.version,
          title: draft.title.trim(),
          description: draft.description,
          typeId: draft.typeId,
          statusId: draft.statusId,
          priority: draft.priority,
          assigneeActorId: draft.assigneeActorId || null,
        },
      },
      {
        onSuccess: (current) => {
          idempotencyKey.current = crypto.randomUUID()
          setBase(current)
          setDraft(draftFromItem(current))
        },
        onError: (error) => setConflict(conflictItem(error)),
      },
    )
  }

  const toggleArchived = () => {
    if (archive.isPending) return
    archive.mutate(
      {
        projectId: base.projectId,
        workItemId: base.id,
        expectedVersion: base.version,
        archived: !base.archivedAt,
        idempotencyKey: archiveKey.current,
      },
      {
        onSuccess: (current) => {
          archiveKey.current = crypto.randomUUID()
          setBase(current)
          setDraft(draftFromItem(current))
          setConflict(null)
        },
        onError: (error) => setConflict(conflictItem(error)),
      },
    )
  }

  return (
    <>
      <header className="work-item-detail-heading">
        <div>
          <span className="work-item-key">{base.key}</span>
          <h1>{base.title}</h1>
        </div>
        {base.archivedAt ? <span className="archive-badge">Archived</span> : null}
      </header>

      {conflict ? (
        <section className="conflict-banner" role="alert">
          <div>
            <strong>A newer version exists.</strong>
            <p>Your draft is still here. Reload the latest fields, then reapply the changes you want.</p>
          </div>
          <button
            className="secondary-button"
            onClick={() => {
              setBase(conflict)
              setDraft(draftFromItem(conflict))
              setConflict(null)
              update.reset()
            }}
            type="button"
          >
            Load latest version
          </button>
        </section>
      ) : null}

      <form className="work-item-editor" onSubmit={save}>
        <label className="field">
          <span>Title</span>
          <input disabled={disabled} maxLength={240} onChange={(event) => setDraft({ ...draft, title: event.target.value })} required value={draft.title} />
        </label>
        <label className="field">
          <span>Description</span>
          <textarea disabled={disabled} maxLength={100000} onChange={(event) => setDraft({ ...draft, description: event.target.value })} placeholder="Describe the outcome, constraints, and useful context." rows={10} value={draft.description} />
        </label>
        <div className="field-grid work-item-properties">
          <label className="field">
            <span>Type</span>
            <select disabled={disabled} onChange={(event) => setDraft({ ...draft, typeId: event.target.value })} value={draft.typeId}>
              {workflow.types.map((type) => <option key={type.id} value={type.id}>{type.name}</option>)}
            </select>
          </label>
          <label className="field">
            <span>Status</span>
            <select disabled={disabled} onChange={(event) => setDraft({ ...draft, statusId: event.target.value })} value={draft.statusId}>
              {allowedStatuses.map((status) => <option key={status.id} value={status.id}>{status.name}</option>)}
            </select>
          </label>
          <label className="field">
            <span>Priority</span>
            <select disabled={disabled} onChange={(event) => setDraft({ ...draft, priority: event.target.value as WorkItemPriority })} value={draft.priority}>
              {workItemPriorities.map((priority) => <option key={priority.value} value={priority.value}>{priority.label}</option>)}
            </select>
          </label>
          <label className="field">
            <span>Assignee</span>
            <input disabled={disabled} maxLength={120} onChange={(event) => setAssigneeQuery(event.target.value)} placeholder="Filter people and agents" value={assigneeQuery} />
            <select disabled={disabled} onChange={(event) => setDraft({ ...draft, assigneeActorId: event.target.value })} value={draft.assigneeActorId}>
              <option value="">Unassigned</option>
              {base.assignee && !assignees.data?.some((actor) => actor.id === base.assignee?.actorId) ? (
                <option value={base.assignee.actorId}>{base.assignee.displayName}{base.assignee.active ? '' : ' (inactive)'}</option>
              ) : null}
              {assignees.data?.map((actor) => <option key={actor.id} value={actor.id}>{actor.displayName} · @{actor.handle}</option>)}
            </select>
          </label>
        </div>
        {update.isError && !conflict ? <p className="form-error-summary" role="alert">{update.error.message}</p> : null}
        <div className="form-actions">
          <span aria-live="polite" className="save-state">
            {update.isSuccess && !dirty ? `Saved as version ${base.version}` : dirty ? 'Unsaved changes' : `Version ${base.version}`}
          </span>
          <button className="secondary-button" disabled={!dirty || update.isPending} onClick={() => setDraft(draftFromItem(base))} type="button">Discard draft</button>
          <button className="primary-button" disabled={disabled || !dirty || !draft.title.trim() || update.isPending} type="submit">{update.isPending ? 'Saving…' : 'Save changes'}</button>
        </div>
      </form>

      {base.capabilities.archive && !projectArchived ? (
        <section className="archive-project work-item-archive" aria-labelledby="archive-work-item-heading">
          <h2 id="archive-work-item-heading">{base.archivedAt ? 'Restore Work Item' : 'Archive Work Item'}</h2>
          <p>{base.archivedAt ? 'Return this item to active planning views.' : 'Hide this item from ordinary views while preserving its identity and history.'}</p>
          {archive.isError && !conflict ? <p className="form-error-summary" role="alert">{archive.error.message}</p> : null}
          <button className={base.archivedAt ? 'secondary-button' : 'danger-button'} disabled={archive.isPending} onClick={toggleArchived} type="button">
            {archive.isPending ? 'Saving…' : base.archivedAt ? 'Restore Work Item' : 'Archive Work Item'}
          </button>
        </section>
      ) : null}
    </>
  )
}

function WorkItemContext({ item }: { item: WorkItem }) {
  return (
    <aside aria-label="Work Item context" className="context-drawer">
      <span className="section-label">WORK ITEM</span>
      <h2>{item.key}</h2>
      <dl>
        <div><dt>Status</dt><dd>{item.status.name}</dd></div>
        <div><dt>Type</dt><dd>{item.itemType.name}</dd></div>
        <div><dt>Priority</dt><dd>{item.priority.replace('_', ' ')}</dd></div>
        <div><dt>Assignee</dt><dd>{item.assignee?.displayName ?? 'Unassigned'}</dd></div>
        <div><dt>Version</dt><dd>{item.version}</dd></div>
      </dl>
      <p className="context-note">Acceptance criteria, linked Threads, repositories, comments, and activity join this canonical detail in the next M4 slices.</p>
    </aside>
  )
}

function DetailLoading() {
  return <div aria-busy="true" className="page-content"><div aria-label="Loading Work Item" className="page-skeleton"><span /><span /><span /></div></div>
}
