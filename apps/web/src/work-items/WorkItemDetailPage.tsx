import { FormEvent, useMemo, useRef, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { searchMentionableActors } from '../conversations/api'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { MessageContent } from '../conversations/MessageContent'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { listRepositories } from '../repositories/api'
import { AppShell } from '../shell/AppShell'
import { normalizeWorkItem, searchWorkItemReferences } from './api'
import { workItemPriorities } from './constants'
import {
  useAssignableActors,
  useCreateAcceptanceCriterion,
  useCreateWorkItemComment,
  useRemoveAcceptanceCriterion,
  useSetWorkItemArchived,
  useSetWorkItemPrimaryRepository,
  useSetWorkItemRepositoryLink,
  useSetWorkItemThreadLink,
  useUpdateAcceptanceCriterion,
  useUpdateWorkItem,
  useWorkItem,
  useWorkItemWorkflow,
  useWorkItemTimeline,
  useWorkWithWorker,
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

      <AcceptanceCriteria
        disabled={disabled}
        item={base}
        onItemChange={(current) => {
          setBase(current)
          setDraft(draftFromItem(current))
          setConflict(null)
        }}
      />

      <WorkWithWorker disabled={disabled} item={base} />

      <WorkItemLinks
        disabled={disabled}
        item={base}
        onItemChange={(current) => {
          setBase(current)
          setDraft(draftFromItem(current))
          setConflict(null)
        }}
      />

      <WorkItemCollaboration disabled={disabled} item={base} />

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

function WorkWithWorker({ disabled, item }: { disabled: boolean; item: WorkItem }) {
  const navigate = useNavigate()
  const conversations = useProjectConversationSidebar(item.projectId)
  const repositories = useQuery({
    queryKey: ['projects', item.projectId, 'repositories', 'worker-picker'],
    queryFn: () => listRepositories(item.projectId),
    enabled: Boolean(item.projectId),
  })
  const mutation = useWorkWithWorker()
  const [channelId, setChannelId] = useState('')
  const [repositoryId, setRepositoryId] = useState('')
  const [prompt, setPrompt] = useState('')
  const channels = (conversations.data?.channels ?? []).filter((channel) => !channel.archivedAt)
  const availableRepositories = (repositories.data?.repositories ?? []).filter(
    (repository) =>
      repository.enabled &&
      repository.trusted &&
      repository.preparationState === 'ready' &&
      !repository.archivedAt,
  )

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!channelId || !prompt.trim() || mutation.isPending) return
    mutation.mutate(
      {
        projectId: item.projectId,
        workItemId: item.id,
        expectedVersion: item.version,
        channelId,
        prompt: prompt.trim(),
        repositoryId: repositoryId || undefined,
        idempotencyKey: crypto.randomUUID(),
      },
      {
        onSuccess: (response) => {
          navigate(
            `/projects/${encodeURIComponent(item.projectId)}/threads/${encodeURIComponent(response.threadId)}`,
          )
        },
      },
    )
  }

  if (disabled) return null
  return (
    <section className="work-item-section worker-launch" aria-labelledby="work-with-worker-heading">
      <header>
        <div>
          <h2 id="work-with-worker-heading">Work with Worker</h2>
          <p>Create a linked Thread and start an isolated run with this Work Item as durable context.</p>
        </div>
      </header>
      <form className="worker-launch-form" onSubmit={submit}>
        <div className="field-grid">
          <label className="field">
            <span>Channel</span>
            <select onChange={(event) => setChannelId(event.target.value)} required value={channelId}>
              <option value="">Select Channel</option>
              {channels.map((channel) => <option key={channel.id} value={channel.id}>{channel.name}</option>)}
            </select>
          </label>
          <label className="field">
            <span>Repository</span>
            <select onChange={(event) => setRepositoryId(event.target.value)} value={repositoryId}>
              <option value="">Automatic selection</option>
              {availableRepositories.map((repository) => <option key={repository.id} value={repository.id}>{repository.owner}/{repository.name}</option>)}
            </select>
          </label>
        </div>
        <label className="field">
          <span>Instructions</span>
          <textarea maxLength={18000} onChange={(event) => setPrompt(event.target.value)} placeholder="Describe the focused change or investigation for the Worker." required rows={4} value={prompt} />
        </label>
        {mutation.isError ? <p className="form-error-summary" role="alert">{mutation.error.message}</p> : null}
        <div className="form-actions">
          <span className="save-state">Only the immutable Work Item ID, key, and these instructions are sent.</span>
          <button className="primary-button" disabled={!channelId || !prompt.trim() || mutation.isPending} type="submit">{mutation.isPending ? 'Starting…' : 'Create Thread and start'}</button>
        </div>
      </form>
    </section>
  )
}

function AcceptanceCriteria({
  disabled,
  item,
  onItemChange,
}: {
  disabled: boolean
  item: WorkItem
  onItemChange: (item: WorkItem) => void
}) {
  const create = useCreateAcceptanceCriterion()
  const update = useUpdateAcceptanceCriterion()
  const remove = useRemoveAcceptanceCriterion()
  const [text, setText] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editingText, setEditingText] = useState('')
  const operationKey = useRef(crypto.randomUUID())
  const busy = create.isPending || update.isPending || remove.isPending

  const nextKey = () => {
    const key = operationKey.current
    operationKey.current = crypto.randomUUID()
    return key
  }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!text.trim() || busy) return
    create.mutate(
      {
        projectId: item.projectId,
        workItemId: item.id,
        expectedVersion: item.version,
        text: text.trim(),
        idempotencyKey: nextKey(),
      },
      {
        onSuccess: (current) => {
          setText('')
          onItemChange(current)
        },
      },
    )
  }

  return (
    <section className="work-item-section" aria-labelledby="acceptance-heading">
      <header>
        <div>
          <h2 id="acceptance-heading">Acceptance criteria</h2>
          <p>{item.acceptanceCriteria.filter((criterion) => criterion.completed).length} of {item.acceptanceCriteria.length} complete</p>
        </div>
      </header>
      {item.acceptanceCriteria.length ? (
        <ol className="criteria-list">
          {item.acceptanceCriteria.map((criterion, index) => (
            <li key={criterion.id}>
              {editingId === criterion.id ? (
                <form
                  className="criterion-edit-form"
                  onSubmit={(event) => {
                    event.preventDefault()
                    if (!editingText.trim() || busy) return
                    update.mutate(
                      {
                        projectId: item.projectId,
                        workItemId: item.id,
                        criterionId: criterion.id,
                        expectedVersion: item.version,
                        text: editingText.trim(),
                        idempotencyKey: nextKey(),
                      },
                      {
                        onSuccess: (current) => {
                          setEditingId(null)
                          setEditingText('')
                          onItemChange(current)
                        },
                      },
                    )
                  }}
                >
                  <label className="visually-hidden" htmlFor={`edit-criterion-${criterion.id}`}>Edit {criterion.text}</label>
                  <input
                    autoFocus
                    id={`edit-criterion-${criterion.id}`}
                    maxLength={1000}
                    onChange={(event) => setEditingText(event.target.value)}
                    value={editingText}
                  />
                  <button className="secondary-button" disabled={!editingText.trim() || busy} type="submit">Save</button>
                  <button className="text-button" disabled={busy} onClick={() => setEditingId(null)} type="button">Cancel</button>
                </form>
              ) : (
                <label>
                  <input
                    checked={criterion.completed}
                    disabled={disabled || busy}
                    onChange={(event) =>
                      update.mutate(
                        {
                          projectId: item.projectId,
                          workItemId: item.id,
                          criterionId: criterion.id,
                          expectedVersion: item.version,
                          completed: event.target.checked,
                          idempotencyKey: nextKey(),
                        },
                        { onSuccess: onItemChange },
                      )
                    }
                    type="checkbox"
                  />
                  <span>{criterion.text}</span>
                </label>
              )}
              <div className="criterion-actions">
                <button
                  aria-label={`Edit ${criterion.text}`}
                  disabled={disabled || busy || editingId === criterion.id}
                  onClick={() => { setEditingId(criterion.id); setEditingText(criterion.text) }}
                  type="button"
                >Edit</button>
                <button
                  aria-label={`Move ${criterion.text} up`}
                  disabled={disabled || busy || index === 0}
                  onClick={() => update.mutate({ projectId: item.projectId, workItemId: item.id, criterionId: criterion.id, expectedVersion: item.version, position: index, idempotencyKey: nextKey() }, { onSuccess: onItemChange })}
                  type="button"
                >↑</button>
                <button
                  aria-label={`Move ${criterion.text} down`}
                  disabled={disabled || busy || index === item.acceptanceCriteria.length - 1}
                  onClick={() => update.mutate({ projectId: item.projectId, workItemId: item.id, criterionId: criterion.id, expectedVersion: item.version, position: index + 2, idempotencyKey: nextKey() }, { onSuccess: onItemChange })}
                  type="button"
                >↓</button>
                <button
                  aria-label={`Remove ${criterion.text}`}
                  disabled={disabled || busy}
                  onClick={() => remove.mutate({ projectId: item.projectId, workItemId: item.id, criterionId: criterion.id, expectedVersion: item.version, idempotencyKey: nextKey() }, { onSuccess: onItemChange })}
                  type="button"
                >×</button>
              </div>
              {criterion.completedBy ? <small>Completed by {criterion.completedBy.displayName}</small> : null}
            </li>
          ))}
        </ol>
      ) : <p className="compact-note">No acceptance criteria yet.</p>}
      {!disabled ? (
        <form className="inline-create-form" onSubmit={submit}>
          <label className="visually-hidden" htmlFor={`criterion-${item.id}`}>New acceptance criterion</label>
          <input id={`criterion-${item.id}`} maxLength={1000} onChange={(event) => setText(event.target.value)} placeholder="Add an observable outcome" value={text} />
          <button className="secondary-button" disabled={!text.trim() || busy} type="submit">Add criterion</button>
        </form>
      ) : null}
      {create.isError || update.isError || remove.isError ? <p className="form-error-summary" role="alert">The checklist changed before this action completed. Reload the Work Item and reapply the change.</p> : null}
    </section>
  )
}

function WorkItemLinks({
  disabled,
  item,
  onItemChange,
}: {
  disabled: boolean
  item: WorkItem
  onItemChange: (item: WorkItem) => void
}) {
  const conversations = useProjectConversationSidebar(item.projectId)
  const repositories = useQuery({
    queryKey: ['projects', item.projectId, 'repositories', 'work-item-picker'],
    queryFn: () => listRepositories(item.projectId),
    enabled: Boolean(item.projectId),
  })
  const threadMutation = useSetWorkItemThreadLink()
  const repositoryMutation = useSetWorkItemRepositoryLink()
  const primaryMutation = useSetWorkItemPrimaryRepository()
  const [threadId, setThreadId] = useState('')
  const [repositoryId, setRepositoryId] = useState('')
  const [repositoryRole, setRepositoryRole] = useState<'primary' | 'related'>('related')
  const availableThreads = (conversations.data?.channels ?? []).flatMap((channel) =>
    channel.threads
      .filter((thread) => !thread.archivedAt && !item.linkedThreads.some((link) => link.id === thread.id))
      .map((thread) => ({ ...thread, channelName: channel.name })),
  )
  const availableRepositories = (repositories.data?.repositories ?? []).filter(
    (repository) => !repository.archivedAt && !item.linkedRepositories.some((link) => link.id === repository.id),
  )
  const busy = threadMutation.isPending || repositoryMutation.isPending || primaryMutation.isPending

  return (
    <section className="work-item-section" aria-labelledby="links-heading">
      <header><div><h2 id="links-heading">Links</h2><p>Discussion and code context remain independent, durable records.</p></div></header>
      <div className="work-item-link-columns">
        <div>
          <h3>Threads</h3>
          {item.linkedThreads.length ? (
            <ul className="linked-record-list">
              {item.linkedThreads.map((thread) => (
                <li key={thread.id}>
                  <div><Link to={thread.href}>{thread.name}</Link><small>{thread.parentChannelName}{thread.newestRunState ? ` · ${thread.newestRunState}` : ''}{thread.archivedAt ? ' · archived' : ''}</small></div>
                  {!disabled ? <button className="text-button" disabled={busy} onClick={() => threadMutation.mutate({ projectId: item.projectId, workItemId: item.id, threadId: thread.id, linked: false, idempotencyKey: crypto.randomUUID() }, { onSuccess: onItemChange })} type="button">Unlink</button> : null}
                </li>
              ))}
            </ul>
          ) : <p className="compact-note">No linked Threads.</p>}
          {!disabled ? (
            <form className="link-picker" onSubmit={(event) => { event.preventDefault(); if (!threadId) return; threadMutation.mutate({ projectId: item.projectId, workItemId: item.id, threadId, linked: true, idempotencyKey: crypto.randomUUID() }, { onSuccess: (current) => { setThreadId(''); onItemChange(current) } }) }}>
              <label htmlFor={`thread-link-${item.id}`}>Link an active Thread</label>
              <div><select id={`thread-link-${item.id}`} onChange={(event) => setThreadId(event.target.value)} value={threadId}><option value="">Select Thread</option>{availableThreads.map((thread) => <option key={thread.id} value={thread.id}>{thread.channelName} / {thread.name}</option>)}</select><button className="secondary-button" disabled={!threadId || busy} type="submit">Link</button></div>
            </form>
          ) : null}
        </div>
        <div>
          <h3>Repositories</h3>
          {item.linkedRepositories.length ? (
            <ul className="linked-record-list">
              {item.linkedRepositories.map((repository) => (
                <li key={repository.id}>
                  <div><Link to={repository.href}>{repository.owner}/{repository.name}</Link><small>{repository.role}{repository.available ? '' : ` · ${repository.preparationState}`}{repository.archivedAt ? ' · archived' : ''}</small></div>
                  {!disabled ? <div className="linked-record-actions">{repository.role !== 'primary' && repository.available ? <button className="text-button" disabled={busy} onClick={() => primaryMutation.mutate({ projectId: item.projectId, workItemId: item.id, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() }, { onSuccess: onItemChange })} type="button">Make primary</button> : null}<button className="text-button" disabled={busy} onClick={() => repositoryMutation.mutate({ projectId: item.projectId, workItemId: item.id, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() }, { onSuccess: onItemChange })} type="button">Unlink</button></div> : null}
                </li>
              ))}
            </ul>
          ) : <p className="compact-note">No linked repositories.</p>}
          {!disabled ? (
            <form className="link-picker" onSubmit={(event) => { event.preventDefault(); if (!repositoryId) return; repositoryMutation.mutate({ projectId: item.projectId, workItemId: item.id, repositoryId, expectedVersion: item.version, role: repositoryRole, idempotencyKey: crypto.randomUUID() }, { onSuccess: (current) => { setRepositoryId(''); onItemChange(current) } }) }}>
              <label htmlFor={`repository-link-${item.id}`}>Link a repository</label>
              <div><select id={`repository-link-${item.id}`} onChange={(event) => setRepositoryId(event.target.value)} value={repositoryId}><option value="">Select repository</option>{availableRepositories.map((repository) => <option key={repository.id} value={repository.id}>{repository.owner}/{repository.name}</option>)}</select><select aria-label="Repository role" onChange={(event) => setRepositoryRole(event.target.value as 'primary' | 'related')} value={repositoryRole}><option value="related">Related</option><option value="primary">Primary</option></select><button className="secondary-button" disabled={!repositoryId || busy} type="submit">Link</button></div>
            </form>
          ) : null}
        </div>
      </div>
      <div>
        <h3>Pull requests</h3>
        {item.linkedPullRequests.length ? (
          <ul className="linked-record-list">
            {item.linkedPullRequests.map((pullRequest) => (
              <li key={pullRequest.id}>
                <div>
                  <a href={pullRequest.url} rel="noreferrer" target="_blank">
                    #{pullRequest.number} {pullRequest.title}
                  </a>
                  <small>
                    {pullRequest.draft ? 'draft · ' : ''}
                    {pullRequest.state} · {pullRequest.source}
                    {pullRequest.confirmed ? '' : ' · needs confirmation'}
                    {' · checks '}{pullRequest.checksState}
                    {' · review '}{pullRequest.reviewState}
                    {' · mergeability '}{pullRequest.mergeabilityState}
                    {pullRequest.syncState === 'failed'
                      ? ` · sync failed: ${(pullRequest.syncFailureCategory ?? 'unknown').replaceAll('_', ' ')}`
                      : ''}
                    {pullRequest.automationState !== 'not_evaluated'
                      ? ` · automation ${pullRequest.automationState}${pullRequest.automationReason ? `: ${pullRequest.automationReason.replaceAll('_', ' ')}` : ''}`
                      : ''}
                  </small>
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <p className="compact-note">No linked pull requests.</p>
        )}
      </div>
      {threadMutation.isError || repositoryMutation.isError || primaryMutation.isError ? <p className="form-error-summary" role="alert">The links changed before this action completed. Reload the Work Item and try again.</p> : null}
    </section>
  )
}

function WorkItemCollaboration({ disabled, item }: { disabled: boolean; item: WorkItem }) {
  const timeline = useWorkItemTimeline(item.projectId, item.id)
  const createComment = useCreateWorkItemComment()
  return (
    <section className="work-item-section" aria-labelledby="activity-heading">
      <header><div><h2 id="activity-heading">Activity</h2><p>Comments and durable changes in one ordered history.</p></div></header>
      {timeline.isPending ? <p className="compact-note">Loading activity…</p> : null}
      {timeline.isError ? <p className="form-error-summary" role="alert">Activity could not be loaded.</p> : null}
      {timeline.data?.length ? (
        <ol className="work-item-activity-list">
          {[...timeline.data].reverse().map((entry) => (
            <li key={entry.sequence}>
              <span className="activity-avatar" aria-hidden="true">{(entry.comment?.author.displayName ?? entry.activity?.actor.displayName ?? 'A').charAt(0)}</span>
              <div>
                <p><strong>{entry.comment?.author.displayName ?? entry.activity?.actor.displayName}</strong> {entry.comment ? 'commented' : entry.activity?.summary.toLowerCase()}</p>
                {entry.comment ? <div className="activity-comment"><MessageContent actors={entry.comment.mentionedActors.map((actor) => ({ ...actor, kind: actor.kind as 'human' | 'external_agent' | 'execution_worker' | 'integration' | 'system', avatarUrl: null }))} content={entry.comment.content} fallbackPlainText={entry.comment.plainText} workItems={entry.comment.referencedWorkItems} /></div> : null}
                <time dateTime={entry.createdAt}>{new Date(entry.createdAt).toLocaleString()}</time>
              </div>
            </li>
          ))}
        </ol>
      ) : timeline.isSuccess ? <p className="compact-note">No activity yet.</p> : null}
      {!disabled ? (
        <StructuredComposer
          disabled={createComment.isPending}
          label="Add a comment"
          onSubmit={async ({ content, idempotencyKey }) => {
            await createComment.mutateAsync({ projectId: item.projectId, workItemId: item.id, content, idempotencyKey })
          }}
          placeholder="Add context without changing the Work Item fields."
          searchActors={async (query) => (await searchMentionableActors(item.projectId, query)).actors}
          searchWorkItems={(query) => searchWorkItemReferences(item.projectId, query)}
          submitLabel="Post comment"
        />
      ) : null}
    </section>
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
      <p className="context-note">{item.linkedThreads.length} linked {item.linkedThreads.length === 1 ? 'Thread' : 'Threads'} · {item.linkedRepositories.length} linked {item.linkedRepositories.length === 1 ? 'repository' : 'repositories'}.</p>
    </aside>
  )
}

function DetailLoading() {
  return <div aria-busy="true" className="page-content"><div aria-label="Loading Work Item" className="page-skeleton"><span /><span /><span /></div></div>
}
