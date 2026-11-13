import { requestJson } from '../api/client'
import type { ConversationWorkItemReference } from '../conversations/types'
import type { MessageContentV1 } from '../conversations/types'
import { parseMessageContent } from '../conversations/messageContentModel'
import {
  activityChanges,
  actorRef,
  actorRefs,
  boolean,
  claim,
  color,
  date,
  estimateUnit,
  handoff,
  list,
  milestoneRef,
  nullableNumber,
  nullableString,
  number,
  priority,
  record,
  relations,
  string,
  taskRef,
  taskRefs,
} from './wire'
import { normalizeReferences, normalizeValues } from '../fields/fieldModel'
import type {
  AssignableActor,
  CommentRevisions,
  WorkItem,
  BoardColumnsBy,
  WorkItemCard,
  WorkItemGroup,
  WorkItemLabel,
  WorkItemFilters,
  WorkItemListResponse,
  WorkItemBoardResponse,
  WorkItemPatch,
  WorkItemPriority,
  WorkItemTimelineEntry,
  WorkItemWorkflow,
  WorkWithWorkerResponse,
} from './types'

function labels(value: unknown): WorkItemLabel[] {
  return list(value).map((entry) => {
    const label = record(entry)
    return { id: string(label.id), name: string(label.name), color: color(label.color) }
  })
}

export function normalizeWorkItem(value: unknown): WorkItem {
  const item = record(value)
  const itemType = record(item.itemType)
  const status = record(item.status)
  const assignee = item.assignee === null ? null : record(item.assignee)
  const capabilities = record(item.capabilities)
  return {
    id: string(item.id),
    projectId: string(item.projectId),
    key: string(item.key),
    number: number(item.number),
    title: string(item.title),
    description: string(item.description),
    itemType: { id: string(itemType.id), name: string(itemType.name) },
    status: {
      id: string(status.id),
      name: string(status.name),
      semanticCategory: string(status.semanticCategory),
    },
    priority: priority(item.priority),
    assignee:
      assignee === null
        ? null
        : {
            actorId: string(assignee.actorId),
            displayName: string(assignee.displayName),
            handle: string(assignee.handle),
            kind: string(assignee.kind),
            active: boolean(assignee.active),
          },
    version: number(item.version),
    archivedAt: nullableString(item.archivedAt),
    createdAt: string(item.createdAt),
    updatedAt: string(item.updatedAt),
    labels: labels(item.labels),
    acceptanceCriteria: (Array.isArray(item.acceptanceCriteria)
      ? item.acceptanceCriteria
      : []
    ).map((value) => {
      const criterion = record(value)
      const completedBy = criterion.completedBy
        ? record(criterion.completedBy)
        : null
      return {
        id: string(criterion.id),
        text: string(criterion.text),
        position: number(criterion.position),
        completed: boolean(criterion.completed),
        completedBy:
          completedBy === null
            ? null
            : {
                id: string(completedBy.id),
                displayName: string(completedBy.displayName),
                handle: string(completedBy.handle),
                kind: string(completedBy.kind),
              },
        completedAt: nullableString(criterion.completedAt),
        createdAt: string(criterion.createdAt),
        updatedAt: string(criterion.updatedAt),
      }
    }),
    linkedThreads: (Array.isArray(item.linkedThreads) ? item.linkedThreads : []).map(
      (value) => {
        const thread = record(value)
        return {
          id: string(thread.id),
          name: string(thread.name),
          parentChannelId: string(thread.parentChannelId),
          parentChannelName: string(thread.parentChannelName),
          archivedAt: nullableString(thread.archivedAt),
          newestRunState: nullableString(thread.newestRunState),
          href: string(thread.href),
        }
      },
    ),
    linkedRepositories: (Array.isArray(item.linkedRepositories)
      ? item.linkedRepositories
      : []
    ).map((value) => {
      const repository = record(value)
      return {
        id: string(repository.id),
        owner: string(repository.owner),
        name: string(repository.name),
        cloneUrl: string(repository.cloneUrl),
        role: repository.role === 'primary' ? 'primary' : 'related',
        preparationState: string(repository.preparationState),
        trusted: boolean(repository.trusted),
        available: boolean(repository.available),
        archivedAt: nullableString(repository.archivedAt),
        href: string(repository.href),
      }
    }),
    linkedPullRequests: (Array.isArray(item.linkedPullRequests)
      ? item.linkedPullRequests
      : []
    ).map((value) => {
      const pullRequest = record(value)
      return {
        id: string(pullRequest.id),
        number: number(pullRequest.number),
        title: string(pullRequest.title),
        url: string(pullRequest.url),
        state: string(pullRequest.state),
        draft: boolean(pullRequest.draft),
        source: string(pullRequest.source),
        confirmed: boolean(pullRequest.confirmed),
        checksState: string(pullRequest.checksState),
        reviewState: string(pullRequest.reviewState),
        mergeabilityState: string(pullRequest.mergeabilityState),
        syncState: string(pullRequest.syncState),
        syncFailureCategory: nullableString(pullRequest.syncFailureCategory),
        automationState: string(pullRequest.automationState),
        automationReason: nullableString(pullRequest.automationReason),
      }
    }),
    capabilities: {
      view: boolean(capabilities.view),
      edit: boolean(capabilities.edit),
      archive: boolean(capabilities.archive),
    },
    parent: taskRef(item.parent),
    milestone: milestoneRef(item.milestone),
    startDate: date(item.startDate),
    dueDate: date(item.dueDate),
    estimate: nullableNumber(item.estimate),
    blocked: blockedFlag(item.blocked),
    blockedBy: taskRefs(item.blockedBy),
    blocks: taskRefs(item.blocks),
    ready: boolean(item.ready),
    relations: relations(item.relations),
    subtasks: list(item.subtasks).flatMap((value) => {
      const task = taskRef(value)
      return task ? [{ ...task, assignee: actorRef(record(value).assignee) }] : []
    }),
    claim: claim(item.claim),
    handoff: handoff(item.handoff),
    watchers: actorRefs(item.watchers),
    watching: boolean(item.watching),
    estimateUnit: estimateUnit(item.estimateUnit),
    fieldValues: normalizeValues(item.fieldValues),
    fieldReferences: normalizeReferences(item.fieldReferences),
  }
}

function blockedFlag(value: unknown): WorkItem['blocked'] {
  if (!value || typeof value !== 'object') return null
  const blocked = record(value)
  return { reason: string(blocked.reason), at: string(blocked.at), by: actorRef(blocked.by) }
}

export function normalizeCard(value: unknown): WorkItemCard {
  const item = normalizeWorkItem(value)
  const wire = record(value)
  const group = wire.group ? record(wire.group) : null
  return {
    id: item.id,
    projectId: item.projectId,
    key: item.key,
    number: item.number,
    title: item.title,
    itemType: item.itemType,
    status: item.status,
    priority: item.priority,
    assignee: item.assignee,
    version: item.version,
    archivedAt: item.archivedAt,
    updatedAt: item.updatedAt,
    labels: item.labels,
    capabilities: item.capabilities,
    createdAt: item.createdAt,
    milestone: item.milestone,
    startDate: item.startDate,
    dueDate: item.dueDate,
    estimate: item.estimate,
    ready: item.ready,
    fieldValues: item.fieldValues,
    fieldReferences: item.fieldReferences,
    parentId: nullableString(wire.parentId),
    blockedReason: nullableString(wire.blockedReason),
    openBlockerCount: number(wire.openBlockerCount),
    subtaskCount: number(wire.subtaskCount),
    subtasksDone: number(wire.subtasksDone),
    claimedBy: actorRef(wire.claimedBy),
    boardOrder: nullableString(wire.boardOrder) ?? '',
    ...(group ? { group: { key: string(group.key), label: string(group.label) } } : {}),
  }
}

function workItemLinkPath(
  projectId: string,
  workItemId: string,
  collection: 'threads' | 'repositories',
  targetId: string,
) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/${collection}/${encodeURIComponent(targetId)}`
}

export async function setWorkItemThreadLink(input: {
  projectId: string
  workItemId: string
  threadId: string
  linked: boolean
  idempotencyKey: string
}) {
  return normalizeWorkItem(await requestJson<unknown>(
    workItemLinkPath(input.projectId, input.workItemId, 'threads', input.threadId),
    { method: input.linked ? 'POST' : 'DELETE', idempotencyKey: input.idempotencyKey, json: {} },
  ))
}

export async function setWorkItemRepositoryLink(input: {
  projectId: string
  workItemId: string
  repositoryId: string
  expectedVersion: number
  role?: 'primary' | 'related'
  idempotencyKey: string
}) {
  const path = workItemLinkPath(input.projectId, input.workItemId, 'repositories', input.repositoryId)
  return normalizeWorkItem(await requestJson<unknown>(path, {
    method: input.role ? 'POST' : 'DELETE',
    idempotencyKey: input.idempotencyKey,
    json: input.role ? { expectedVersion: input.expectedVersion, role: input.role } : { expectedVersion: input.expectedVersion },
  }))
}

export async function setWorkItemPrimaryRepository(input: {
  projectId: string
  workItemId: string
  repositoryId: string
  expectedVersion: number
  idempotencyKey: string
}) {
  const path = `${workItemLinkPath(input.projectId, input.workItemId, 'repositories', input.repositoryId)}/primary`
  return normalizeWorkItem(await requestJson<unknown>(path, {
    method: 'POST', idempotencyKey: input.idempotencyKey, json: { expectedVersion: input.expectedVersion },
  }))
}

export async function workWithWorker(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  channelId: string
  prompt: string
  repositoryId?: string
  idempotencyKey: string
}): Promise<WorkWithWorkerResponse> {
  const { projectId, workItemId, idempotencyKey, ...json } = input
  const payload = record(await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/work-with-worker`,
    { method: 'POST', idempotencyKey, json },
  ))
  const run = record(payload.run)
  return {
    threadId: string(payload.threadId),
    run: {
      runId: string(run.runId),
      state: string(run.state),
      repositoryId: string(run.repositoryId),
    },
  }
}

/** The non-empty filters as query parameters, in a stable order. */
export function filterParameters(filters: Record<string, unknown>) {
  const parameters = new URLSearchParams()
  for (const key of Object.keys(filters).sort()) {
    const value = filters[key]
    if (value === undefined || value === null || value === '' || value === false) continue
    parameters.set(key, String(value))
  }
  return parameters
}

function projectPath(projectId: string, path: string) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/${path}`
}

export async function listWorkItems(
  projectId: string,
  filters: WorkItemFilters | Record<string, unknown>,
): Promise<WorkItemListResponse> {
  const payload = record(await requestJson<unknown>(projectPath(projectId, `work-items?${filterParameters(filters)}`)))
  const items = list(payload.items).map(normalizeCard)
  return {
    items,
    nextCursor: nullableString(payload.nextCursor),
    total: typeof payload.total === 'number' ? payload.total : items.length,
  }
}

export async function getWorkItemGroups(projectId: string, filters: WorkItemFilters): Promise<WorkItemGroup[]> {
  const rest: Record<string, unknown> = { ...filters }
  delete rest.cursor
  delete rest.limit
  delete rest.sort
  const payload = record(await requestJson<unknown>(projectPath(projectId, `work-items/groups?${filterParameters(rest)}`)))
  return list(payload.groups).map((value) => {
    const group = record(value)
    const groupColor = nullableString(group.color)
    return {
      key: string(group.key),
      label: string(group.label),
      count: number(group.count),
      ...(groupColor ? { color: color(groupColor) } : {}),
    }
  })
}

const COLUMNS_BY = new Set(['status', 'assignee', 'priority', 'milestone', 'type', 'label'])

export async function getWorkItemBoard(
  projectId: string,
  filters: WorkItemFilters & { columnsBy?: BoardColumnsBy },
): Promise<WorkItemBoardResponse> {
  const rest: Record<string, unknown> = { ...filters }
  delete rest.cursor
  delete rest.groupBy
  delete rest.sort
  const payload = record(await requestJson<unknown>(projectPath(projectId, `work-items/board?${filterParameters(rest)}`)))
  const columnsBy = string(payload.columnsBy)
  return {
    columnsBy: (COLUMNS_BY.has(columnsBy) ? columnsBy : 'status') as BoardColumnsBy,
    columns: list(payload.columns).map((value) => {
      const column = record(value)
      const status = column.status ? record(column.status) : null
      const items = list(column.items).map(normalizeCard)
      const filter: Record<string, string> = {}
      for (const [key, entry] of Object.entries(record(column.filter))) {
        if (typeof entry === 'string') filter[key] = entry
      }
      return {
        key: string(column.key) || string(status?.id),
        label: string(column.label) || string(status?.name),
        status: status
          ? { id: string(status.id), name: string(status.name), semanticCategory: string(status.semanticCategory) }
          : null,
        items,
        hasMore: boolean(column.hasMore),
        total: typeof column.total === 'number' ? column.total : items.length,
        nextCursor: nullableString(column.nextCursor),
        filter,
        droppable: column.droppable !== false,
      }
    }),
  }
}

export async function searchWorkItemReferences(
  projectId: string,
  query: string,
): Promise<ConversationWorkItemReference[]> {
  const parameters = new URLSearchParams({ q: query, limit: String(ASSIGNEE_PAGE) })
  const payload = record(await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/work-item-references?${parameters}`,
  ))
  return (Array.isArray(payload.items) ? payload.items : []).flatMap((value) => {
    const item = record(value)
    const id = string(item.id)
    const key = string(item.key)
    const title = string(item.title)
    const href = string(item.href)
    if (!id || !key || !title || !href.startsWith('/projects/')) return []
    return [{
      id,
      key,
      title,
      statusCategory: string(item.statusCategory),
      archivedAt: nullableString(item.archivedAt),
      href,
    }]
  })
}

export async function moveWorkItem(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  targetStatusId: string
  targetPosition?: number
  beforeId?: string
  afterId?: string
  idempotencyKey: string
}) {
  const { projectId, workItemId, idempotencyKey, ...json } = input
  return normalizeWorkItem(await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/move`,
    { method: 'POST', idempotencyKey, json },
  ))
}

export async function getWorkItem(projectId: string, workItemId: string) {
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}`,
    ),
  )
}

export async function getWorkItemWorkflow(projectId: string): Promise<WorkItemWorkflow> {
  return normalizeWorkflow(
    await requestJson<unknown>(`/api/v1/projects/${encodeURIComponent(projectId)}/work-item-workflow`),
  )
}

export function normalizeWorkflow(value: unknown): WorkItemWorkflow {
  const payload = record(value)
  return {
    types: (Array.isArray(payload.types) ? payload.types : []).map((value) => {
      const item = record(value)
      return {
        id: string(item.id),
        name: string(item.name),
        position: number(item.position),
        isDefault: boolean(item.isDefault),
        taskCount: number(item.taskCount),
      }
    }),
    statuses: (Array.isArray(payload.statuses) ? payload.statuses : []).map(
      (value) => {
        const item = record(value)
        return {
          id: string(item.id),
          name: string(item.name),
          semanticCategory: string(item.semanticCategory),
          position: number(item.position),
          isDefault: boolean(item.isDefault),
          taskCount: number(item.taskCount),
        }
      },
    ),
    transitions: (Array.isArray(payload.transitions)
      ? payload.transitions
      : []
    ).map((value) => {
      const item = record(value)
      return {
        fromStatusId: string(item.fromStatusId),
        toStatusId: string(item.toStatusId),
      }
    }),
    canEdit: boolean(payload.canEdit),
    version: number(payload.version),
    estimateUnit: estimateUnit(payload.estimateUnit),
  }
}

export async function createWorkItem(input: {
  projectId: string
  title: string
  description?: string
  typeId?: string
  statusId?: string
  priority: WorkItemPriority
  assigneeActorId?: string
  parentWorkItemId?: string
  milestoneId?: string
  labelIds?: string[]
  startDate?: string
  dueDate?: string
  estimate?: number
  acceptanceCriteria?: string[]
  /** Made from a message: the task links back to it. */
  sourceMessageId?: string
  idempotencyKey: string
}) {
  const { projectId, idempotencyKey, ...json } = input
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-items`,
      { method: 'POST', idempotencyKey, json },
    ),
  )
}

export async function updateWorkItem(input: {
  projectId: string
  workItemId: string
  patch: WorkItemPatch
  idempotencyKey: string
}) {
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(input.projectId)}/work-items/${encodeURIComponent(input.workItemId)}`,
      { method: 'PATCH', idempotencyKey: input.idempotencyKey, json: input.patch },
    ),
  )
}

export async function setWorkItemArchived(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  archived: boolean
  idempotencyKey: string
}) {
  const command = input.archived ? 'archive' : 'restore'
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(input.projectId)}/work-items/${encodeURIComponent(input.workItemId)}/${command}`,
      {
        method: 'POST',
        idempotencyKey: input.idempotencyKey,
        json: { expectedVersion: input.expectedVersion },
      },
    ),
  )
}

/** How many assignable people and agents one search returns. */
export const ASSIGNEE_PAGE = 20

export async function searchAssignableActors(
  projectId: string,
  query: string,
): Promise<AssignableActor[]> {
  return assignableActors(projectId, new URLSearchParams({ q: query, limit: '20' }))
}

/** The project's members (past ones too) with these IDs, such as the people a filter picks. */
export async function namedAssignableActors(projectId: string, ids: readonly string[]): Promise<AssignableActor[]> {
  return assignableActors(projectId, new URLSearchParams({ ids: ids.join(',') }))
}

async function assignableActors(projectId: string, parameters: URLSearchParams): Promise<AssignableActor[]> {
  const payload = record(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-item-assignees?${parameters}`,
    ),
  )
  return (Array.isArray(payload.actors) ? payload.actors : []).map((value) => {
    const actor = record(value)
    return {
      id: string(actor.id),
      displayName: string(actor.displayName),
      handle: string(actor.handle),
      kind: string(actor.kind),
    }
  })
}

export async function createAcceptanceCriterion(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  text: string
  idempotencyKey: string
}) {
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(input.projectId)}/work-items/${encodeURIComponent(input.workItemId)}/acceptance-criteria`,
      {
        method: 'POST',
        idempotencyKey: input.idempotencyKey,
        json: { expectedVersion: input.expectedVersion, text: input.text },
      },
    ),
  )
}

export async function updateAcceptanceCriterion(input: {
  projectId: string
  workItemId: string
  criterionId: string
  expectedVersion: number
  text?: string
  position?: number
  completed?: boolean
  idempotencyKey: string
}) {
  const { projectId, workItemId, criterionId, idempotencyKey, ...json } = input
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/acceptance-criteria/${encodeURIComponent(criterionId)}`,
      { method: 'PATCH', idempotencyKey, json },
    ),
  )
}

export async function removeAcceptanceCriterion(input: {
  projectId: string
  workItemId: string
  criterionId: string
  expectedVersion: number
  idempotencyKey: string
}) {
  return normalizeWorkItem(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(input.projectId)}/work-items/${encodeURIComponent(input.workItemId)}/acceptance-criteria/${encodeURIComponent(input.criterionId)}`,
      {
        method: 'DELETE',
        idempotencyKey: input.idempotencyKey,
        json: { expectedVersion: input.expectedVersion },
      },
    ),
  )
}

/** A comment needs words; one that is only files says what it attaches. */
export function commentContent({ attachments, content, text }: { attachments?: { name: string }[]; content: MessageContentV1; text?: string }): MessageContentV1 {
  return text || !attachments?.length
    ? content
    : { version: 1, nodes: [{ type: 'text', text: `Attached ${attachments.map((file) => file.name).join(', ')}`.slice(0, 2000) }] }
}

export async function createWorkItemComment(input: {
  projectId: string
  workItemId: string
  content: MessageContentV1
  idempotencyKey: string
  attachments?: { id: string }[]
  /** The comment on this task it answers (`messageId` is the comment's id). */
  replyTo?: { messageId: string }
}) {
  return requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(input.projectId)}/work-items/${encodeURIComponent(input.workItemId)}/comments`,
    {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      // JSON leaves out `attachments` and `replyToCommentId` when there are none.
      json: {
        content: input.content,
        attachments: input.attachments?.length ? input.attachments.map((file) => file.id) : undefined,
        replyToCommentId: input.replyTo?.messageId,
      },
    },
  )
}

const EMPTY_CONTENT: MessageContentV1 = { version: 1, nodes: [] }

function commentActor(value: unknown): AssignableActor {
  const actor = record(value)
  return { id: string(actor.id), displayName: string(actor.displayName), handle: string(actor.handle), kind: string(actor.kind) }
}

function commentRemoval(removal: Record<string, unknown>) {
  return { removedAt: string(removal.removedAt), removedBy: removal.removedBy ? commentActor(removal.removedBy) : null }
}

function commentReply(value: unknown) {
  if (!value || typeof value !== 'object') return null
  const reply = record(value)
  if (!string(reply.commentId)) return null
  return {
    commentId: string(reply.commentId),
    authorName: nullableString(reply.authorName),
    text: reply.removed === true ? null : nullableString(reply.text),
    removed: reply.removed === true,
  }
}

const commentPath = (projectId: string, workItemId: string, commentId: string) =>
  `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/comments/${encodeURIComponent(commentId)}`

/** Saves new text for your own comment, against the revision you opened it at. */
export async function editWorkItemComment(input: {
  projectId: string
  workItemId: string
  commentId: string
  content: MessageContentV1
  expectedRevision: number
}) {
  return requestJson<unknown>(commentPath(input.projectId, input.workItemId, input.commentId), {
    method: 'PATCH',
    json: { content: input.content, expectedRevision: input.expectedRevision },
  })
}

/** Deletes a comment and every earlier version of its text: your own, or anyone's as a project owner or admin. */
export async function deleteWorkItemComment(input: { projectId: string; workItemId: string; commentId: string }) {
  return requestJson<unknown>(commentPath(input.projectId, input.workItemId, input.commentId), { method: 'DELETE' })
}

export async function getWorkItemCommentRevisions(projectId: string, workItemId: string, commentId: string): Promise<CommentRevisions> {
  const payload = record(await requestJson<unknown>(`${commentPath(projectId, workItemId, commentId)}/revisions`))
  return {
    commentId: string(payload.commentId),
    revision: number(payload.revision),
    removed: payload.removed === true,
    revisions: (Array.isArray(payload.revisions) ? payload.revisions : []).map((value) => {
      const revision = record(value)
      return {
        revision: number(revision.revision),
        content: revision.content ? parseMessageContent(revision.content) : null,
        plainText: nullableString(revision.plainText),
        writtenBy: commentActor(revision.writtenBy),
        writtenAt: string(revision.writtenAt),
        redacted: revision.redacted === true,
      }
    }),
  }
}

/** One page of a task's timeline, newest first; `nextCursor` reaches older entries. */
export async function getWorkItemTimeline(
  projectId: string,
  workItemId: string,
  cursor?: string | null,
): Promise<{ entries: WorkItemTimelineEntry[]; nextCursor: string | null }> {
  const query = new URLSearchParams({ limit: '100' })
  if (cursor) query.set('cursor', cursor)
  const payload = record(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/timeline?${query.toString()}`,
    ),
  )
  const nextCursor = typeof payload.nextCursor === 'string' && payload.nextCursor ? payload.nextCursor : null
  const entries = (Array.isArray(payload.entries) ? payload.entries : []).map((value): WorkItemTimelineEntry => {
    const entry = record(value)
    const activity = entry.activity ? record(entry.activity) : null
    const activityActor = activity?.actor ? record(activity.actor) : null
    const comment = entry.comment ? record(entry.comment) : null
    const commentAuthor = comment?.author ? record(comment.author) : null
    // A deleted comment keeps its place with no text left.
    const commentContent = comment ? (comment.removed ? EMPTY_CONTENT : parseMessageContent(comment.content)) : null
    return {
      sequence: number(entry.sequence),
      kind: entry.kind === 'comment' ? 'comment' : 'activity',
      createdAt: string(entry.createdAt),
      activity:
        activity && activityActor
          ? {
              id: string(activity.id),
              actor: {
                id: string(activityActor.id),
                displayName: string(activityActor.displayName),
                handle: string(activityActor.handle),
                kind: string(activityActor.kind),
              },
              kind: string(activity.kind),
              summary: string(activity.summary),
              via: typeof activity.via === 'string' ? activity.via.slice(0, 80) : null,
              changes: activityChanges(record(activity.payload).changes),
            }
          : null,
      comment:
        comment && commentAuthor && commentContent
          ? {
              id: string(comment.id),
              author: {
                id: string(commentAuthor.id),
                displayName: string(commentAuthor.displayName),
                handle: string(commentAuthor.handle),
                kind: string(commentAuthor.kind),
              },
              content: commentContent,
              plainText: string(comment.plainText),
              mentionedActors: (Array.isArray(comment.mentionedActors) ? comment.mentionedActors : []).map((value) => {
                const actor = record(value)
                return { id: string(actor.id), displayName: string(actor.displayName), handle: string(actor.handle), kind: string(actor.kind) }
              }),
              referencedWorkItems: (Array.isArray(comment.referencedWorkItems) ? comment.referencedWorkItems : []).flatMap((value) => {
                const item = record(value)
                const href = string(item.href)
                if (!string(item.id) || !href.startsWith('/projects/')) return []
                return [{ id: string(item.id), key: string(item.key), title: string(item.title), statusCategory: string(item.statusCategory), archivedAt: nullableString(item.archivedAt), href }]
              }),
              createdAt: string(comment.createdAt),
              via: typeof comment.via === 'string' ? comment.via.slice(0, 80) : null,
              revision: typeof comment.revision === 'number' ? comment.revision : 1,
              editedAt: nullableString(comment.editedAt),
              removed: comment.removed ? commentRemoval(record(comment.removed)) : null,
              replyTo: commentReply(comment.replyTo),
            }
          : null,
    }
  })
  return { entries, nextCursor }
}
