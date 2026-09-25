import { useLayoutEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent, type RefObject } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { searchMentionableActors } from '../conversations/api'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { MessageContent } from '../conversations/MessageContent'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { formatFullDate } from '../conversations/timeFormat'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageLoading } from '../projects/ProjectPageStates'
import { humanize } from '../projects/format'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { listRepositories } from '../repositories/api'
import { AppShell } from '../shell/AppShell'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  Checkbox,
  Dialog,
  ErrorState,
  Icon,
  IconButton,
  MessageSkeleton,
  Row,
  RowList,
  SelectField,
  StatusChip,
  TextArea,
  TextField,
  actorKind,
} from '../ui'
import { normalizeWorkItem, searchWorkItemReferences } from './api'
import { buildOpenInCodexLink } from './codexDeepLink'
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
  useWorkItemTimeline,
  useWorkItemWorkflow,
  useWorkWithWorker,
} from './hooks'
import { workItemsHref } from './links'
import type { AssignableActor, WorkItem, WorkItemPriority } from './types'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

type Draft = {
  title: string
  description: string
  typeId: string
  statusId: string
  priority: WorkItemPriority
  assigneeActorId: string
}

type Workflow = NonNullable<ReturnType<typeof useWorkItemWorkflow>['data']>

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

function shortDate(iso: string) {
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
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
    project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

  if (project.isPending || itemQuery.isPending || workflow.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading work item" />
      </AppShell>
    )
  }
  if (project.isError || itemQuery.isError || workflow.isError) {
    return (
      <AppShell user={session.user}>
        <div className="aw-page">
          <ErrorState
            action={
              <Button onClick={() => void Promise.all([project.refetch(), itemQuery.refetch(), workflow.refetch()])}>Try again</Button>
            }
            title="This work item isn’t available"
          >
            It may have been removed, or your access may have changed. If your connection dropped, try again.
          </ErrorState>
        </div>
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
            {itemQuery.data.key}
          </span>
        </nav>
        <WorkItemEditor
          item={itemQuery.data}
          key={itemQuery.data.id}
          projectArchived={Boolean(project.data.archivedAt)}
          workflow={workflow.data}
        />
      </div>
    </AppShell>
  )
}

function WorkItemEditor({ item, projectArchived, workflow }: { item: WorkItem; projectArchived: boolean; workflow: Workflow }) {
  const update = useUpdateWorkItem()
  const archive = useSetWorkItemArchived()
  const [draft, setDraft] = useState(() => draftFromItem(item))
  const [base, setBase] = useState(item)
  const [conflict, setConflict] = useState<WorkItem | null>(null)
  const [assigneeQuery, setAssigneeQuery] = useState('')
  const [runOpen, setRunOpen] = useState(false)
  const runButton = useRef<HTMLButtonElement>(null)
  const titleRef = useRef<HTMLTextAreaElement>(null)
  const assignees = useAssignableActors(item.projectId, assigneeQuery)
  const idempotencyKey = useRef(crypto.randomUUID())
  const archiveKey = useRef(crypto.randomUUID())
  const formId = `work-item-form-${item.id}`

  const allowedStatuses = useMemo(() => {
    const allowed = new Set(
      workflow.transitions.filter((transition) => transition.fromStatusId === base.status.id).map((transition) => transition.toStatusId),
    )
    allowed.add(base.status.id)
    return workflow.statuses.filter((status) => allowed.has(status.id))
  }, [base.status.id, workflow])
  const dirty = JSON.stringify(draft) !== JSON.stringify(draftFromItem(base))
  const disabled = projectArchived || Boolean(base.archivedAt) || !base.capabilities.edit

  useLayoutEffect(() => {
    const element = titleRef.current
    if (!element) return
    element.style.height = 'auto'
    element.style.height = `${element.scrollHeight}px`
  }, [draft.title])

  const replaceBase = (current: WorkItem) => {
    setBase(current)
    setDraft(draftFromItem(current))
    setConflict(null)
  }

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
          replaceBase(current)
        },
        onError: (error) => setConflict(conflictItem(error)),
      },
    )
  }

  const onTitleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return
    event.preventDefault()
    event.currentTarget.form?.requestSubmit()
  }

  const selectedStatus = workflow.statuses.find((status) => status.id === draft.statusId) ?? base.status
  const currentAssignee: AssignableActor | null =
    assignees.data?.find((actor) => actor.id === draft.assigneeActorId) ??
    (base.assignee && base.assignee.actorId === draft.assigneeActorId
      ? { id: base.assignee.actorId, displayName: base.assignee.displayName, handle: base.assignee.handle, kind: base.assignee.kind }
      : null)
  const showSaveBar = !disabled && (dirty || update.isPending || (update.isError && !conflict))

  return (
    <div className="aw-workitem">
      <div className="aw-workitem__head">
        <div className="aw-workitem__top">
          <span className="aw-workitem__key">{base.key}</span>
          {base.archivedAt ? <StatusChip icon="archive">Archived</StatusChip> : null}
        </div>
        {disabled ? (
          <>
            <h1 className="aw-workitem__heading" id="workspace-heading">
              {base.title}
            </h1>
            <p className={base.description ? 'aw-workitem__text' : 'aw-workitem__text is-empty'}>{base.description || 'No description.'}</p>
          </>
        ) : (
          <form className="aw-workitem__form" id={formId} onSubmit={save}>
            <h1 className="visually-hidden" id="workspace-heading">
              {base.key}: {base.title}
            </h1>
            <label className="visually-hidden" htmlFor={`${formId}-title`}>
              Title
            </label>
            <textarea
              className="aw-workitem__title"
              id={`${formId}-title`}
              maxLength={240}
              onChange={(event) => setDraft({ ...draft, title: event.target.value })}
              onKeyDown={onTitleKeyDown}
              placeholder="Work item title"
              ref={titleRef}
              required
              rows={1}
              value={draft.title}
            />
            <TextArea
              aria-label="Description"
              className="aw-workitem__desc"
              maxLength={100000}
              onChange={(event) => setDraft({ ...draft, description: event.target.value })}
              placeholder="Add a description: the outcome you want, any limits, and useful context."
              rows={6}
              value={draft.description}
            />
          </form>
        )}

        {base.archivedAt && (
          <Banner icon="archive" title="This work item is archived.">
            It’s hidden from lists and can’t change until it’s restored.
          </Banner>
        )}
        {conflict ? (
          <Banner
            action={
              <Button
                onClick={() => {
                  replaceBase(conflict)
                  update.reset()
                }}
                size="sm"
              >
                Load latest version
              </Button>
            }
            role="alert"
            title="A newer version exists."
            tone="warning"
          >
            Someone else changed this work item. Your edits are still here: load the latest version, then make them again.
          </Banner>
        ) : null}
      </div>

      <aside aria-label="Work item details" className="aw-workitem__side">
        <div className="aw-props">
          <SelectField
            disabled={disabled}
            form={formId}
            id={`${formId}-status`}
            label="Status"
            onChange={(event) => setDraft({ ...draft, statusId: event.target.value })}
            prefix={<StatusIcon category={selectedStatus.semanticCategory} />}
            value={draft.statusId}
          >
            {allowedStatuses.map((status) => (
              <option key={status.id} value={status.id}>
                {status.name}
              </option>
            ))}
          </SelectField>
          <SelectField
            disabled={disabled}
            form={formId}
            id={`${formId}-priority`}
            label="Priority"
            onChange={(event) => setDraft({ ...draft, priority: event.target.value as WorkItemPriority })}
            prefix={<PriorityIcon decorative priority={draft.priority} />}
            value={draft.priority}
          >
            {workItemPriorities.map((priority) => (
              <option key={priority.value} value={priority.value}>
                {priority.label}
              </option>
            ))}
          </SelectField>
          <SelectField
            disabled={disabled}
            form={formId}
            id={`${formId}-type`}
            label="Type"
            onChange={(event) => setDraft({ ...draft, typeId: event.target.value })}
            value={draft.typeId}
          >
            {workflow.types.map((type) => (
              <option key={type.id} value={type.id}>
                {type.name}
              </option>
            ))}
          </SelectField>
          <div className="aw-props__assignee">
            <SelectField
              disabled={disabled}
              form={formId}
              id={`${formId}-assignee`}
              label="Assignee"
              onChange={(event) => setDraft({ ...draft, assigneeActorId: event.target.value })}
              prefix={
                currentAssignee ? (
                  <Avatar kind={actorKind(currentAssignee.kind)} name={currentAssignee.displayName} seed={currentAssignee.id} size={18} />
                ) : (
                  <Icon name="unassigned" size={16} />
                )
              }
              value={draft.assigneeActorId}
            >
              <option value="">Unassigned</option>
              {base.assignee && !assignees.data?.some((actor) => actor.id === base.assignee?.actorId) ? (
                <option value={base.assignee.actorId}>
                  {base.assignee.displayName}
                  {base.assignee.active ? '' : ' (inactive)'}
                </option>
              ) : null}
              {assignees.data?.map((actor) => (
                <option key={actor.id} value={actor.id}>
                  {actor.displayName} · @{actor.handle}
                </option>
              ))}
            </SelectField>
            {!disabled ? (
              <TextField
                aria-label="Search people and agents to assign"
                className="aw-props__search"
                maxLength={120}
                onChange={(event) => setAssigneeQuery(event.target.value)}
                placeholder="Search people and agents"
                prefix={<Icon name="search" size={14} />}
                type="search"
                value={assigneeQuery}
              />
            ) : null}
          </div>
          <p aria-live="polite" className="aw-props__status">
            {update.isSuccess && !dirty ? 'Saved.' : `Updated ${shortDate(base.updatedAt)}`}
          </p>
        </div>

        <OpenInCodex item={base} projectArchived={projectArchived} />

        {!disabled ? (
          <div className="aw-sidecard">
            <h2 className="aw-sidecard__title">Run with Codex</h2>
            <p className="aw-sidecard__text">Start a thread where Codex works on this on the server, with this work item as context.</p>
            <Button icon="play" onClick={() => setRunOpen(true)} ref={runButton}>
              Start a run
            </Button>
          </div>
        ) : null}

        {base.capabilities.archive && !projectArchived ? (
          <div className="aw-sidecard aw-sidecard--quiet">
            <Button
              busy={archive.isPending}
              icon={base.archivedAt ? 'undo' : 'archive'}
              onClick={toggleArchived}
              size="sm"
              variant="ghost"
            >
              {base.archivedAt ? 'Restore work item' : 'Archive work item'}
            </Button>
            {archive.isError && !conflict ? <Banner tone="danger">{archive.error.message}</Banner> : null}
          </div>
        ) : null}
      </aside>

      <div className="aw-workitem__rest">
        <AcceptanceCriteria disabled={disabled} item={base} onItemChange={replaceBase} />
        <WorkItemLinks disabled={disabled} item={base} onItemChange={replaceBase} />
        <WorkItemActivity disabled={disabled} item={base} />

        {showSaveBar ? (
          <div aria-label="Unsaved changes" className="aw-savebar" role="region">
            <span className="aw-savebar__text" role={update.isError ? 'alert' : undefined}>
              {update.isPending ? 'Saving…' : update.isError && !conflict ? update.error.message : 'You have unsaved changes.'}
            </span>
            <Button disabled={!dirty || update.isPending} onClick={() => setDraft(draftFromItem(base))} variant="ghost">
              Discard
            </Button>
            <Button busy={update.isPending} disabled={!dirty || !draft.title.trim()} form={formId} type="submit" variant="primary">
              Save changes
            </Button>
          </div>
        ) : null}
      </div>


      {runOpen ? <StartRunDialog item={base} onClose={() => setRunOpen(false)} returnFocus={runButton} /> : null}
    </div>
  )
}

function OpenInCodex({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  const result = projectArchived
    ? { href: null, reason: 'Restore this Project before opening a work item in Codex.' }
    : buildOpenInCodexLink(item, window.location.origin)
  const reasonId = `open-in-codex-reason-${item.id}`

  return (
    <section aria-labelledby={`open-in-codex-heading-${item.id}`} className="aw-sidecard">
      <h2 className="aw-sidecard__title" id={`open-in-codex-heading-${item.id}`}>
        Work on it locally
      </h2>
      <p className="aw-sidecard__text">Opens Codex on your computer with this work item ready. Nothing starts until you send it there.</p>
      {result.href ? (
        <a className="aw-btn aw-btn--secondary aw-btn--md" href={result.href}>
          <Icon name="terminal" />
          <span className="aw-btn__label">Open in Codex</span>
        </a>
      ) : (
        <>
          <Button aria-describedby={reasonId} disabled icon="terminal">
            Open in Codex
          </Button>
          <p className="aw-sidecard__note" id={reasonId}>
            {result.reason}
          </p>
        </>
      )}
    </section>
  )
}

function StartRunDialog({
  item,
  onClose,
  returnFocus,
}: {
  item: WorkItem
  onClose: () => void
  returnFocus: RefObject<HTMLButtonElement | null>
}) {
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
    (repository) => repository.enabled && repository.trusted && repository.preparationState === 'ready' && !repository.archivedAt,
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
          navigate(`/projects/${encodeURIComponent(item.projectId)}/threads/${encodeURIComponent(response.threadId)}`)
        },
      },
    )
  }

  return (
    <Dialog
      description={`Codex works in a new thread in the channel you pick. It gets ${item.key} and your instructions, nothing else.`}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={mutation.isPending} disabled={!channelId || !prompt.trim()} icon="play" type="submit" variant="primary">
            Start run
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      returnFocus={returnFocus}
      title="Start a run"
      width={560}
    >
      <div className="aw-form__grid">
        <SelectField id="start-run-channel" label="Channel" onChange={(event) => setChannelId(event.target.value)} required value={channelId}>
          <option value="">Choose a channel</option>
          {channels.map((channel) => (
            <option key={channel.id} value={channel.id}>
              #{channel.name}
            </option>
          ))}
        </SelectField>
        <SelectField id="start-run-repository" label="Repository" onChange={(event) => setRepositoryId(event.target.value)} value={repositoryId}>
          <option value="">Pick automatically</option>
          {availableRepositories.map((repository) => (
            <option key={repository.id} value={repository.id}>
              {repository.owner}/{repository.name}
            </option>
          ))}
        </SelectField>
      </div>
      <TextArea
        data-autofocus
        id="start-run-instructions"
        label="Instructions"
        maxLength={18000}
        onChange={(event) => setPrompt(event.target.value)}
        placeholder="Describe the change or investigation you want."
        required
        rows={5}
        value={prompt}
      />
      {mutation.isError ? <Banner tone="danger">{mutation.error.message}</Banner> : null}
    </Dialog>
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
  const total = item.acceptanceCriteria.length
  const done = item.acceptanceCriteria.filter((criterion) => criterion.completed).length

  const nextKey = () => {
    const key = operationKey.current
    operationKey.current = crypto.randomUUID()
    return key
  }
  const base = { projectId: item.projectId, workItemId: item.id, expectedVersion: item.version }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!text.trim() || busy) return
    create.mutate(
      { ...base, text: text.trim(), idempotencyKey: nextKey() },
      {
        onSuccess: (current) => {
          setText('')
          onItemChange(current)
        },
      },
    )
  }

  return (
    <section aria-labelledby="acceptance-heading" className="aw-worksection">
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id="acceptance-heading">
          <Icon name="checklist" size={16} />
          Acceptance criteria
        </h2>
        {total ? (
          <span className="aw-progress">
            <span className="aw-progress__text">
              {done} of {total} done
            </span>
            <progress aria-hidden="true" className="aw-progress__bar" max={total} value={done} />
          </span>
        ) : null}
      </header>
      {total ? (
        <ol className="aw-criteria">
          {item.acceptanceCriteria.map((criterion, index) => (
            <li className="aw-criterion" key={criterion.id}>
              {editingId === criterion.id ? (
                <form
                  className="aw-inline-add"
                  onSubmit={(event) => {
                    event.preventDefault()
                    if (!editingText.trim() || busy) return
                    update.mutate(
                      { ...base, criterionId: criterion.id, text: editingText.trim(), idempotencyKey: nextKey() },
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
                  <TextField
                    aria-label="Criterion text"
                    autoFocus
                    maxLength={1000}
                    onChange={(event) => setEditingText(event.target.value)}
                    value={editingText}
                  />
                  <Button busy={update.isPending} disabled={!editingText.trim()} size="sm" type="submit" variant="primary">
                    Save
                  </Button>
                  <Button disabled={busy} onClick={() => setEditingId(null)} size="sm" variant="ghost">
                    Cancel
                  </Button>
                </form>
              ) : (
                <>
                  <Checkbox
                    checked={criterion.completed}
                    description={criterion.completedBy ? `Done by ${criterion.completedBy.displayName}` : undefined}
                    disabled={disabled || busy}
                    label={criterion.text}
                    onChange={(completed) =>
                      update.mutate(
                        { ...base, criterionId: criterion.id, completed, idempotencyKey: nextKey() },
                        { onSuccess: onItemChange },
                      )
                    }
                  />
                  {!disabled ? (
                    <div className="aw-criterion__actions">
                      <IconButton
                        disabled={busy}
                        icon="pencil"
                        label={`Edit ${criterion.text}`}
                        onClick={() => {
                          setEditingId(criterion.id)
                          setEditingText(criterion.text)
                        }}
                        size="sm"
                      />
                      <IconButton
                        disabled={busy || index === 0}
                        icon="arrow-up"
                        label={`Move ${criterion.text} up`}
                        onClick={() =>
                          update.mutate({ ...base, criterionId: criterion.id, position: index, idempotencyKey: nextKey() }, { onSuccess: onItemChange })
                        }
                        size="sm"
                      />
                      <IconButton
                        disabled={busy || index === total - 1}
                        icon="arrow-down"
                        label={`Move ${criterion.text} down`}
                        onClick={() =>
                          update.mutate(
                            { ...base, criterionId: criterion.id, position: index + 2, idempotencyKey: nextKey() },
                            { onSuccess: onItemChange },
                          )
                        }
                        size="sm"
                      />
                      <IconButton
                        disabled={busy}
                        icon="trash"
                        label={`Remove ${criterion.text}`}
                        onClick={() =>
                          remove.mutate({ ...base, criterionId: criterion.id, idempotencyKey: nextKey() }, { onSuccess: onItemChange })
                        }
                        size="sm"
                      />
                    </div>
                  ) : null}
                </>
              )}
            </li>
          ))}
        </ol>
      ) : (
        <p className="aw-quiet">No acceptance criteria yet. Add what has to be true for this to count as done.</p>
      )}
      {!disabled ? (
        <form className="aw-inline-add" onSubmit={submit}>
          <TextField
            aria-label="New acceptance criterion"
            id={`criterion-${item.id}`}
            maxLength={1000}
            onChange={(event) => setText(event.target.value)}
            placeholder="Add a criterion, like “The export includes archived items”"
            value={text}
          />
          <Button busy={create.isPending} disabled={!text.trim() || busy} icon="plus" size="sm" type="submit">
            Add
          </Button>
        </form>
      ) : null}
      {create.isError || update.isError || remove.isError ? (
        <Banner tone="warning">The checklist changed before that finished. Reload the page and try again.</Banner>
      ) : null}
    </section>
  )
}

function pullRequestSummary(pullRequest: WorkItem['linkedPullRequests'][number]) {
  const parts = [
    pullRequest.draft ? 'Draft' : humanize(pullRequest.state),
    `checks ${humanize(pullRequest.checksState).toLocaleLowerCase()}`,
    `review ${humanize(pullRequest.reviewState).toLocaleLowerCase()}`,
    humanize(pullRequest.mergeabilityState).toLocaleLowerCase(),
  ]
  if (!pullRequest.confirmed) parts.push('needs confirmation')
  if (pullRequest.syncState === 'failed') {
    parts.push(`sync failed: ${humanize(pullRequest.syncFailureCategory ?? 'unknown').toLocaleLowerCase()}`)
  }
  if (pullRequest.automationState !== 'not_evaluated') {
    parts.push(
      `automation ${humanize(pullRequest.automationState).toLocaleLowerCase()}${
        pullRequest.automationReason ? `: ${humanize(pullRequest.automationReason).toLocaleLowerCase()}` : ''
      }`,
    )
  }
  return parts.join(' · ')
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
  const ids = { projectId: item.projectId, workItemId: item.id }

  return (
    <section aria-labelledby="links-heading" className="aw-worksection">
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id="links-heading">
          <Icon name="link" size={16} />
          Links
        </h2>
      </header>

      <div className="aw-links">
        <div className="aw-links__group">
          <h3 className="aw-links__title">Threads</h3>
          {item.linkedThreads.length ? (
            <RowList label="Linked threads">
              {item.linkedThreads.map((thread) => (
                <Row
                  actions={
                    !disabled ? (
                      <Button
                        disabled={busy}
                        icon="unlink"
                        onClick={() =>
                          threadMutation.mutate(
                            { ...ids, threadId: thread.id, linked: false, idempotencyKey: crypto.randomUUID() },
                            { onSuccess: onItemChange },
                          )
                        }
                        size="sm"
                        variant="ghost"
                      >
                        Unlink
                      </Button>
                    ) : null
                  }
                  key={thread.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name="messages" size={16} />
                    </span>
                  }
                  meta={`#${thread.parentChannelName}${thread.newestRunState ? ` · run ${humanize(thread.newestRunState).toLocaleLowerCase()}` : ''}${
                    thread.archivedAt ? ' · archived' : ''
                  }`}
                  title={<Link to={thread.href}>{thread.name}</Link>}
                />
              ))}
            </RowList>
          ) : (
            <p className="aw-quiet">No threads linked.</p>
          )}
          {!disabled ? (
            <form
              className="aw-inline-add"
              onSubmit={(event) => {
                event.preventDefault()
                if (!threadId) return
                threadMutation.mutate(
                  { ...ids, threadId, linked: true, idempotencyKey: crypto.randomUUID() },
                  {
                    onSuccess: (current) => {
                      setThreadId('')
                      onItemChange(current)
                    },
                  },
                )
              }}
            >
              <SelectField aria-label="Thread to link" id={`thread-link-${item.id}`} onChange={(event) => setThreadId(event.target.value)} value={threadId}>
                <option value="">Link a thread…</option>
                {availableThreads.map((thread) => (
                  <option key={thread.id} value={thread.id}>
                    #{thread.channelName} / {thread.name}
                  </option>
                ))}
              </SelectField>
              <Button disabled={!threadId || busy} size="sm" type="submit">
                Link
              </Button>
            </form>
          ) : null}
        </div>

        <div className="aw-links__group">
          <h3 className="aw-links__title">Repositories</h3>
          {item.linkedRepositories.length ? (
            <RowList label="Linked repositories">
              {item.linkedRepositories.map((repository) => (
                <Row
                  actions={
                    !disabled ? (
                      <>
                        {repository.role !== 'primary' && repository.available ? (
                          <Button
                            disabled={busy}
                            onClick={() =>
                              primaryMutation.mutate(
                                { ...ids, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() },
                                { onSuccess: onItemChange },
                              )
                            }
                            size="sm"
                            variant="ghost"
                          >
                            Make primary
                          </Button>
                        ) : null}
                        <Button
                          disabled={busy}
                          icon="unlink"
                          onClick={() =>
                            repositoryMutation.mutate(
                              { ...ids, repositoryId: repository.id, expectedVersion: item.version, idempotencyKey: crypto.randomUUID() },
                              { onSuccess: onItemChange },
                            )
                          }
                          size="sm"
                          variant="ghost"
                        >
                          Unlink
                        </Button>
                      </>
                    ) : null
                  }
                  key={repository.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name="repo" size={16} />
                    </span>
                  }
                  meta={`${repository.role === 'primary' ? 'Primary' : 'Related'}${
                    repository.available ? '' : ` · ${humanize(repository.preparationState).toLocaleLowerCase()}`
                  }${repository.archivedAt ? ' · archived' : ''}`}
                  title={
                    <Link to={repository.href}>
                      {repository.owner}/{repository.name}
                    </Link>
                  }
                />
              ))}
            </RowList>
          ) : (
            <p className="aw-quiet">No repositories linked.</p>
          )}
          {!disabled ? (
            <form
              className="aw-inline-add"
              onSubmit={(event) => {
                event.preventDefault()
                if (!repositoryId) return
                repositoryMutation.mutate(
                  {
                    ...ids,
                    repositoryId,
                    expectedVersion: item.version,
                    role: repositoryRole,
                    idempotencyKey: crypto.randomUUID(),
                  },
                  {
                    onSuccess: (current) => {
                      setRepositoryId('')
                      onItemChange(current)
                    },
                  },
                )
              }}
            >
              <SelectField
                aria-label="Repository to link"
                id={`repository-link-${item.id}`}
                onChange={(event) => setRepositoryId(event.target.value)}
                value={repositoryId}
              >
                <option value="">Link a repository…</option>
                {availableRepositories.map((repository) => (
                  <option key={repository.id} value={repository.id}>
                    {repository.owner}/{repository.name}
                  </option>
                ))}
              </SelectField>
              <SelectField
                aria-label="Repository role"
                onChange={(event) => setRepositoryRole(event.target.value as 'primary' | 'related')}
                value={repositoryRole}
              >
                <option value="related">Related</option>
                <option value="primary">Primary</option>
              </SelectField>
              <Button disabled={!repositoryId || busy} size="sm" type="submit">
                Link
              </Button>
            </form>
          ) : null}
        </div>

        <div className="aw-links__group">
          <h3 className="aw-links__title">Pull requests</h3>
          {item.linkedPullRequests.length ? (
            <RowList label="Linked pull requests">
              {item.linkedPullRequests.map((pullRequest) => (
                <Row
                  key={pullRequest.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name="pull-request" size={16} />
                    </span>
                  }
                  meta={pullRequestSummary(pullRequest)}
                  title={
                    <a href={pullRequest.url} rel="noreferrer" target="_blank">
                      #{pullRequest.number} {pullRequest.title}
                    </a>
                  }
                />
              ))}
            </RowList>
          ) : (
            <p className="aw-quiet">No pull requests linked. Runs link the ones they open.</p>
          )}
        </div>
      </div>
      {threadMutation.isError || repositoryMutation.isError || primaryMutation.isError ? (
        <Banner tone="warning">The links changed before that finished. Reload the page and try again.</Banner>
      ) : null}
    </section>
  )
}

function WorkItemActivity({ disabled, item }: { disabled: boolean; item: WorkItem }) {
  const timeline = useWorkItemTimeline(item.projectId, item.id)
  const createComment = useCreateWorkItemComment()
  return (
    <section aria-labelledby="activity-heading" className="aw-worksection">
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id="activity-heading">
          <Icon name="activity" size={16} />
          Activity
        </h2>
      </header>
      {timeline.isPending ? <MessageSkeleton label="Loading activity" rows={2} /> : null}
      {timeline.isError ? <Banner tone="danger">Activity didn’t load.</Banner> : null}
      {timeline.data?.length ? (
        <ol className="aw-activity">
          {[...timeline.data].reverse().map((entry) => {
            const actor = entry.comment?.author ?? entry.activity?.actor
            const name = actor?.displayName ?? 'Someone'
            const kind = actorKind(actor?.kind ?? 'human')
            const when = (
              <time className="aw-activity__time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
                {shortDate(entry.createdAt)}
              </time>
            )
            if (entry.comment) {
              return (
                <li className="aw-activity__comment" key={entry.sequence}>
                  <Avatar kind={kind} name={name} seed={actor?.id} size={28} />
                  <div className="aw-activity__body">
                    <p className="aw-activity__line">
                      <strong>{name}</strong>
                      <ActorTag kind={kind} />
                      {when}
                    </p>
                    <div className="aw-activity__content">
                      <MessageContent
                        actors={entry.comment.mentionedActors.map((mentioned) => ({
                          ...mentioned,
                          kind: mentioned.kind as 'human' | 'external_agent' | 'execution_worker' | 'integration' | 'system',
                          avatarUrl: null,
                        }))}
                        content={entry.comment.content}
                        fallbackPlainText={entry.comment.plainText}
                        workItems={entry.comment.referencedWorkItems}
                      />
                    </div>
                  </div>
                </li>
              )
            }
            return (
              <li className="aw-activity__event" key={entry.sequence}>
                <span aria-hidden="true" className="aw-activity__dot" />
                <p className="aw-activity__line">
                  <strong>{name}</strong>
                  <span aria-hidden="true">·</span>
                  {entry.activity?.summary}
                  {when}
                </p>
              </li>
            )
          })}
        </ol>
      ) : timeline.isSuccess ? (
        <p className="aw-quiet">No activity yet.</p>
      ) : null}
      {!disabled ? (
        <StructuredComposer
          disabled={createComment.isPending}
          label="Add a comment"
          onSubmit={async ({ content, idempotencyKey }) => {
            await createComment.mutateAsync({ projectId: item.projectId, workItemId: item.id, content, idempotencyKey })
          }}
          placeholder="Add a comment. @mention someone or reference a work item."
          searchActors={async (query) => (await searchMentionableActors(item.projectId, query)).actors}
          searchWorkItems={(query) => searchWorkItemReferences(item.projectId, query)}
          submitLabel="Comment"
        />
      ) : null}
    </section>
  )
}
