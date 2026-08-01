import { requestJson } from '../api/client'
import type {
  AssignableActor,
  WorkItem,
  WorkItemCard,
  WorkItemFilters,
  WorkItemListResponse,
  WorkItemPatch,
  WorkItemPriority,
  WorkItemWorkflow,
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
