import { requestJson } from '../api/client'
import type { ConversationWorkItemReference } from '../conversations/types'
import type { MessageContentV1 } from '../conversations/types'
import { parseMessageContent } from '../conversations/messageContentModel'
import type {
  AssignableActor,
  WorkItem,
  WorkItemCard,
  WorkItemFilters,
  WorkItemListResponse,
  WorkItemBoardResponse,
  WorkItemPatch,
  WorkItemPriority,
  WorkItemTimelineEntry,
  WorkItemWorkflow,
  WorkWithWorkerResponse,
} from './types'

type WireRecord = Record<string, unknown>

function record(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as WireRecord)
    : {}
}

function string(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

function nullableString(value: unknown) {
  return typeof value === 'string' ? value : null
}

function number(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function boolean(value: unknown) {
  return value === true
}

function priority(value: unknown): WorkItemPriority {
  return value === 'low' ||
    value === 'medium' ||
    value === 'high' ||
    value === 'urgent'
    ? value
    : 'no_priority'
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
        role: repository.role === 'primary' ? 'primary' : 'related',
        preparationState: string(repository.preparationState),
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
      }
    }),
    capabilities: {
      view: boolean(capabilities.view),
      edit: boolean(capabilities.edit),
      archive: boolean(capabilities.archive),
    },
  }
}

function normalizeCard(value: unknown): WorkItemCard {
  const item = normalizeWorkItem(value)
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
    capabilities: item.capabilities,
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

export async function listWorkItems(
  projectId: string,
  filters: WorkItemFilters,
): Promise<WorkItemListResponse> {
  const parameters = new URLSearchParams()
  for (const [key, value] of Object.entries(filters)) {
    if (value !== undefined && value !== '' && value !== false) {
      parameters.set(key, String(value))
    }
  }
  const payload = record(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-items?${parameters}`,
    ),
  )
  return {
    items: Array.isArray(payload.items)
      ? payload.items.map(normalizeCard)
      : [],
    nextCursor: nullableString(payload.nextCursor),
  }
}

export async function getWorkItemBoard(
  projectId: string,
  filters: WorkItemFilters,
): Promise<WorkItemBoardResponse> {
  const parameters = new URLSearchParams()
  for (const [key, value] of Object.entries({ ...filters, cursor: undefined })) {
    if (value !== undefined && value !== '' && value !== false) parameters.set(key, String(value))
  }
  const payload = record(await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/board?${parameters}`,
  ))
  return {
    columns: (Array.isArray(payload.columns) ? payload.columns : []).map((value) => {
      const column = record(value)
      const status = record(column.status)
      return {
        status: {
          id: string(status.id),
          name: string(status.name),
          semanticCategory: string(status.semanticCategory),
        },
        items: (Array.isArray(column.items) ? column.items : []).map(normalizeCard),
        hasMore: boolean(column.hasMore),
      }
    }),
  }
}

export async function searchWorkItemReferences(
  projectId: string,
  query: string,
): Promise<ConversationWorkItemReference[]> {
  const parameters = new URLSearchParams({ q: query, limit: '20' })
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
  targetPosition: number
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

export async function getWorkItemWorkflow(
  projectId: string,
): Promise<WorkItemWorkflow> {
  const payload = record(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-item-workflow`,
    ),
  )
  return {
    types: (Array.isArray(payload.types) ? payload.types : []).map((value) => {
      const item = record(value)
      return {
        id: string(item.id),
        name: string(item.name),
        position: number(item.position),
        isDefault: boolean(item.isDefault),
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

export async function searchAssignableActors(
  projectId: string,
  query: string,
): Promise<AssignableActor[]> {
  const parameters = new URLSearchParams({ q: query, limit: '20' })
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

export async function createWorkItemComment(input: {
  projectId: string
  workItemId: string
  content: MessageContentV1
  idempotencyKey: string
}) {
  return requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(input.projectId)}/work-items/${encodeURIComponent(input.workItemId)}/comments`,
    {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: {
        content: input.content,
      },
    },
  )
}

export async function getWorkItemTimeline(
  projectId: string,
  workItemId: string,
): Promise<WorkItemTimelineEntry[]> {
  const payload = record(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}/timeline?limit=100`,
    ),
  )
  return (Array.isArray(payload.entries) ? payload.entries : []).map((value) => {
    const entry = record(value)
    const activity = entry.activity ? record(entry.activity) : null
    const activityActor = activity?.actor ? record(activity.actor) : null
    const comment = entry.comment ? record(entry.comment) : null
    const commentAuthor = comment?.author ? record(comment.author) : null
    const commentContent = comment ? parseMessageContent(comment.content) : null
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
            }
          : null,
    }
  })
}
