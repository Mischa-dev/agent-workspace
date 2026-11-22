import { isSpaceAddress } from '../app/href'
import { workItemHref } from './links'
import { requestJson } from '../api/client'
import { ApiError, isApiError } from '../api/errors'
import { callOp, ranOrThrow, type OpInput } from '../api/ops'
import { queueOp } from '../data/outbound'
import type { CriterionOperationArgument, TaskUpdateInput } from '../api/generated/operations'
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
import { normalizeReferences, normalizeValues, type FieldValue } from '../fields/fieldModel'
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
  const delegate = item.delegate ? record(item.delegate) : null
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
    delegate:
      delegate === null
        ? null
        : {
            actorId: string(delegate.actorId),
            displayName: string(delegate.displayName),
            handle: string(delegate.handle),
            kind: string(delegate.kind),
            active: true,
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
        headRef: string(pullRequest.headRef),
        headSha: string(pullRequest.headSha),
        baseRef: string(pullRequest.baseRef),
        authorLogin: string(pullRequest.authorLogin),
        requestedReviewers: (Array.isArray(pullRequest.requestedReviewers) ? pullRequest.requestedReviewers : []).map((login) => string(login)),
        checks: (Array.isArray(pullRequest.checks) ? pullRequest.checks : []).map((value) => {
          const check = record(value)
          return {
            name: string(check.name),
            status: string(check.status),
            conclusion: nullableString(check.conclusion),
            url: nullableString(check.url),
          }
        }),
      }
    }),
    capabilities: {
      view: boolean(capabilities.view),
      edit: boolean(capabilities.edit),
      suggest: boolean(capabilities.suggest),
      archive: boolean(capabilities.archive),
      mergePullRequests: boolean(capabilities.mergePullRequests),
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
    collectionId: nullableString(item.collectionId),
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

/** Links or unlinks a record on a task with `task_link`, then reads the task as it is now. */
async function linkOnTask(
  input: { projectId: string; workItemId: string; idempotencyKey: string; expectedVersion?: number },
  link: Omit<OpInput<'task_link'>, 'taskId' | 'expectedVersion'>,
) {
  ranOrThrow(
    await callOp('task_link', { taskId: input.workItemId, ...link }, { requestId: input.idempotencyKey, expectedVersion: input.expectedVersion }).catch(
      (error: unknown) => refuseAsMovedOn(error, input),
    ),
    'Linking it',
  )
  return getWorkItem(input.projectId, input.workItemId)
}

export async function setWorkItemThreadLink(input: {
  projectId: string
  workItemId: string
  threadId: string
  linked: boolean
  idempotencyKey: string
}) {
  return linkOnTask(input, { conversationId: input.threadId, ...(input.linked ? {} : { remove: true }) })
}

export async function setWorkItemRepositoryLink(input: {
  projectId: string
  workItemId: string
  repositoryId: string
  expectedVersion: number
  role?: 'primary' | 'related'
  idempotencyKey: string
}) {
  return linkOnTask(input, { repositoryId: input.repositoryId, ...(input.role ? { role: input.role } : { remove: true }) })
}

/** Links a pull request to a task (confirming a suggested one), or unlinks it (dismissing a suggestion). */
export async function setWorkItemPullRequestLink(input: {
  projectId: string
  workItemId: string
  pullRequestId: string
  linked: boolean
  expectedVersion: number
  idempotencyKey: string
}) {
  return linkOnTask(input, { pullRequestId: input.pullRequestId, ...(input.linked ? {} : { remove: true }) })
}

/** Merges a task's pull request on GitHub if its head is still the one shown. */
export async function mergeWorkItemPullRequest(input: {
  projectId: string
  workItemId: string
  pullRequestId: string
  expectedHeadSha: string
}) {
  return callOp('pull_request_merge', { pullRequestId: input.pullRequestId, expectedHeadSha: input.expectedHeadSha })
}

export async function setWorkItemPrimaryRepository(input: {
  projectId: string
  workItemId: string
  repositoryId: string
  expectedVersion: number
  idempotencyKey: string
}) {
  return linkOnTask(input, { repositoryId: input.repositoryId, role: 'primary' })
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
    collectionId: nullableString(payload.collectionId),
    config: payload.config ?? {},
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
    columnsBy: (COLUMNS_BY.has(columnsBy) || /^field:[A-Za-z0-9-]{1,64}$/.test(columnsBy) ? columnsBy : 'status') as BoardColumnsBy,
    columns: list(payload.columns).map((value) => {
      const column = record(value)
      const status = column.status ? record(column.status) : null
      const columnColor = nullableString(column.color)
      const items = list(column.items).map(normalizeCard)
      const filter: Record<string, string> = {}
      for (const [key, entry] of Object.entries(record(column.filter))) {
        if (typeof entry === 'string') filter[key] = entry
      }
      return {
        key: string(column.key) || string(status?.id),
        label: string(column.label) || string(status?.name),
        ...(columnColor ? { color: color(columnColor) } : {}),
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
    if (!id || !key || !title || !isSpaceAddress(href)) return []
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
  /** 1 is the top of the column. */
  targetPosition?: number
  beforeId?: string
  afterId?: string
  idempotencyKey: string
}) {
  const operation: TaskUpdateInput = { taskId: input.workItemId, expectedVersion: input.expectedVersion, status: input.targetStatusId }
  if (input.beforeId) operation.beforeTaskId = input.beforeId
  else if (input.afterId) operation.afterTaskId = input.afterId
  else if (input.targetPosition !== undefined) operation.position = String(input.targetPosition)
  ranOrThrow(
    await callOp('task_update', operation, { requestId: input.idempotencyKey }).catch((error: unknown) => refuseAsMovedOn(error, input)),
    'Moving it',
  )
  return getWorkItem(input.projectId, input.workItemId)
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
  /** Custom field values by field ID or name. */
  fieldValues?: Record<string, FieldValue>
  /** Made from a message: the task links back to it. */
  sourceMessageId?: string
  /** With a source message, say so in its conversation (C2.20). */
  saySo?: boolean
  idempotencyKey: string
}) {
  const { projectId, idempotencyKey } = input
  const created = await callOp(
      'task_create',
      {
        projectId,
        title: input.title,
        description: input.description,
        type: input.typeId,
        status: input.statusId,
        priority: input.priority,
        assignee: input.assigneeActorId,
        parentId: input.parentWorkItemId,
        milestone: input.milestoneId,
        labels: input.labelIds?.length ? input.labelIds : undefined,
        startDate: input.startDate,
        dueDate: input.dueDate,
        estimate: input.estimate,
        acceptanceCriteria: input.acceptanceCriteria?.length ? input.acceptanceCriteria : undefined,
        fromMessageId: input.sourceMessageId,
        saySo: input.saySo,
        // Fields go by ID or name.
        fields: input.fieldValues && Object.keys(input.fieldValues).length ? input.fieldValues : undefined,
      },
      { requestId: idempotencyKey },
    )
  ranOrThrow(created, 'Creating it')
  return getWorkItem(projectId, created.workItemId)
}

/**
 * The patch as `task_update` takes it, or null for a blocker change, which
 * the operation takes on its own (see `blockerInput`). A status change
 * there also moves the task to the bottom of its new column.
 */
export function taskUpdateInput(workItemId: string, patch: WorkItemPatch): TaskUpdateInput | null {
  if (patch.blockedReason !== undefined) return null
  const input: TaskUpdateInput = { taskId: workItemId, expectedVersion: patch.expectedVersion }
  if (patch.title !== undefined) input.title = patch.title
  if (patch.description !== undefined) input.description = patch.description
  if (patch.typeId !== undefined) input.type = patch.typeId
  if (patch.priority !== undefined) input.priority = patch.priority
  if (patch.assigneeActorId !== undefined) {
    if (patch.assigneeActorId === null) input.clearAssignee = true
    else input.assignee = patch.assigneeActorId
  }
  if (patch.delegateActorId !== undefined) input.delegate = patch.delegateActorId ?? 'none'
  if (patch.parentWorkItemId !== undefined) {
    if (patch.parentWorkItemId === null) input.clearParent = true
    else input.parentId = patch.parentWorkItemId
  }
  if (patch.milestoneId !== undefined) input.milestone = patch.milestoneId ?? 'none'
  if (patch.startDate !== undefined) input.startDate = patch.startDate ?? ''
  if (patch.dueDate !== undefined) input.dueDate = patch.dueDate ?? ''
  if (patch.estimate !== undefined) {
    if (patch.estimate === null) input.clearEstimate = true
    else input.estimate = patch.estimate
  }
  if (patch.addLabelIds?.length) input.addLabels = patch.addLabelIds
  if (patch.removeLabelIds?.length) input.removeLabels = patch.removeLabelIds
  if (patch.statusId !== undefined) input.status = patch.statusId
  // Fields go by ID or name; null clears one.
  if (patch.fieldValues !== undefined) input.fields = patch.fieldValues
  return input
}

/** Blocks a task with a reason, or (null) clears its blocker where it stands. */
function blockerInput(workItemId: string, expectedVersion: number, reason: string | null): TaskUpdateInput {
  return reason === null ? { taskId: workItemId, expectedVersion, unblock: true } : { taskId: workItemId, expectedVersion, blockedReason: reason }
}

/**
 * A task edit that may wait for the connection (W1.30): offline, it's kept on
 * this device and sent when the connection is back, and this returns null.
 * A blocker change goes straight away, and never waits.
 */
export async function queueWorkItemUpdate(input: {
  projectId: string
  workItemId: string
  patch: WorkItemPatch
  idempotencyKey: string
  /** What it was, in words, for the note if it's refused later. */
  label: string
}): Promise<WorkItem | null> {
  const operation = taskUpdateInput(input.workItemId, input.patch)
  if (!operation) return updateWorkItem(input)
  const meta = { thing: `task:${input.workItemId}`, href: workItemHref(input.projectId, input.workItemId), label: input.label }
  const sent = await queueOp('task_update', operation, meta, { requestId: input.idempotencyKey }).catch((error: unknown) =>
    refuseAsMovedOn(error, { ...input, expectedVersion: input.patch.expectedVersion }),
  )
  return sent === null ? null : getWorkItem(input.projectId, input.workItemId)
}

/**
 * Changes a task through `task_update`. A blocker change goes on its own
 * after the rest, against the version the rest left.
 */
type WorkItemUpdate = {
  projectId: string
  workItemId: string
  patch: WorkItemPatch
  idempotencyKey: string
}

/** Saves a change and returns the task as it now stands. */
export async function updateWorkItem(input: WorkItemUpdate) {
  await writeWorkItem(input)
  return getWorkItem(input.projectId, input.workItemId)
}

/** Saves a change without reading the task back: it resolves once the write
 * has landed, so a caller can tell a refused write from a failed read. */
export async function writeWorkItem(input: WorkItemUpdate): Promise<void> {
  const { blockedReason, ...rest } = input.patch
  const refuse = (expectedVersion: number) => (error: unknown) => refuseAsMovedOn(error, { ...input, expectedVersion })
  let version = rest.expectedVersion
  const changesMore = Object.keys(rest).some((key) => key !== 'expectedVersion')
  if (changesMore || blockedReason === undefined) {
    const written = ranOrThrow(
      await callOp('task_update', taskUpdateInput(input.workItemId, rest) as TaskUpdateInput, { requestId: input.idempotencyKey }).catch(
        refuse(version),
      ),
      'Saving it',
    )
    version = written.version
  }
  if (blockedReason !== undefined) {
    // Request ids take letters, digits, dashes, underscores and dots.
    const requestId = changesMore ? `${input.idempotencyKey}-blocker` : input.idempotencyKey
    ranOrThrow(
      await callOp('task_update', blockerInput(input.workItemId, version, blockedReason), { requestId }).catch(refuse(version)),
      blockedReason === null ? 'Unblocking it' : 'Marking it blocked',
    )
  }
}

// W1.9a: these writes go through the op client; its answers are short, so
// each reads the task back to hand callers the whole task, as before.

export async function setWorkItemArchived(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  archived: boolean
  idempotencyKey: string
}) {
  const task = { taskId: input.workItemId, expectedVersion: input.expectedVersion }
  ranOrThrow(
    await callOp(input.archived ? 'task_archive' : 'task_restore', task, { requestId: input.idempotencyKey }).catch((error: unknown) =>
      refuseAsMovedOn(error, input),
    ),
    input.archived ? 'Archiving it' : 'Restoring it',
  )
  return getWorkItem(input.projectId, input.workItemId)
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

/** One change to a task's acceptance criteria, through `task_criteria_update`. */
async function changeCriteria(
  input: { projectId: string; workItemId: string; expectedVersion: number; idempotencyKey: string },
  ...operations: CriterionOperationArgument[]
) {
  await callOp(
    'task_criteria_update',
    { taskId: input.workItemId, expectedVersion: input.expectedVersion, operations },
    { requestId: input.idempotencyKey },
  ).catch((error: unknown) => refuseAsMovedOn(error, input))
  return getWorkItem(input.projectId, input.workItemId)
}

/**
 * A task write's conflict, with the whole task as it is now: criteria and
 * links refuse without it, and `task_update` and archiving send only a few of
 * its fields. Reads the task, and if it moved on past the version the change
 * was made against, refuses with it, so the editor loads the newer one.
 */
async function refuseAsMovedOn(error: unknown, input: { projectId: string; workItemId: string; expectedVersion?: number }): Promise<never> {
  if (!isApiError(error) || error.code !== 'conflict' || input.expectedVersion === undefined) throw error
  const current = await getWorkItem(input.projectId, input.workItemId).catch(() => null)
  if (!current || current.version === input.expectedVersion) throw error
  throw new ApiError({
    status: error.status,
    code: 'conflict',
    message: error.message,
    details: { reason: 'version', currentVersion: current.version, current: { task: current, whole: true } },
    requestId: error.requestId,
  })
}

export async function createAcceptanceCriterion(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  text: string
  idempotencyKey: string
}) {
  return changeCriteria(input, { op: 'add', text: input.text })
}

/** Edits, checks and moves one criterion, all in one change the task's version covers. */
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
  const criterion = input.criterionId
  const operations: CriterionOperationArgument[] = []
  if (input.text !== undefined) operations.push({ op: 'edit', criterion, text: input.text })
  if (input.completed !== undefined) operations.push({ op: input.completed ? 'check' : 'uncheck', criterion })
  if (input.position !== undefined) operations.push({ op: 'move', criterion, position: input.position })
  return changeCriteria(input, ...operations)
}

export async function removeAcceptanceCriterion(input: {
  projectId: string
  workItemId: string
  criterionId: string
  expectedVersion: number
  idempotencyKey: string
}) {
  return changeCriteria(input, { op: 'remove', criterion: input.criterionId })
}

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
  return callOp(
    'task_comment_create',
    {
      taskId: input.workItemId,
      content: input.content,
      // Left out when there are none.
      attachmentIds: input.attachments?.length ? input.attachments.map((file) => file.id) : undefined,
      replyToCommentId: input.replyTo?.messageId,
    },
    { requestId: input.idempotencyKey },
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
  return callOp('task_comment_update', {
    taskId: input.workItemId,
    commentId: input.commentId,
    content: input.content,
    expectedRevision: input.expectedRevision,
  })
}

/** Deletes a comment and every earlier version of its text: your own, or anyone's as a project owner or admin. */
export async function deleteWorkItemComment(input: { projectId: string; workItemId: string; commentId: string }) {
  return callOp('task_comment_delete', { taskId: input.workItemId, commentId: input.commentId })
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
                if (!string(item.id) || !isSpaceAddress(href)) return []
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
