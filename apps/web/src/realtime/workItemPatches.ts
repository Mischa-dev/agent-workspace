import type { Query, QueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { ApiError } from '../api/errors'
import { currentQueryScopeGeneration } from '../api/queryScope'
import { getWorkItem } from '../work-items/api'
import type { WorkItem, WorkItemCard, WorkItemFilters, WorkItemListResponse } from '../work-items/types'

/** Where a task's card sits on the board, from a work_item.updated event. */
export type WorkItemPlacement = {
  statusId: string
  archived: boolean
  afterId: string | null
  beforeId: string | null
}

export type WorkItemChange = {
  workItemId: string
  version: number
  placement: WorkItemPlacement | null
}

const uuidPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

function uuid(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.toLowerCase()
  return uuidPattern.test(normalized) ? normalized : null
}

function optionalUuid(value: unknown): string | null | undefined {
  if (value === undefined || value === null) return null
  return uuid(value) ?? undefined
}

/** Reads a work_item.updated event. A malformed placement is dropped, not trusted. */
export function workItemChange(event: {
  type: string
  payload: Record<string, unknown>
}): WorkItemChange | null {
  if (event.type !== 'work_item.updated') return null
  const workItemId = uuid(event.payload.workItemId)
  const version = event.payload.version
  if (!workItemId || !Number.isSafeInteger(version) || (version as number) < 1) return null
  const raw = event.payload.placement
  let placement: WorkItemPlacement | null = null
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    const record = raw as Record<string, unknown>
    const statusId = uuid(record.statusId)
    const afterId = optionalUuid(record.afterId)
    const beforeId = optionalUuid(record.beforeId)
    if (statusId && typeof record.archived === 'boolean' && afterId !== undefined && beforeId !== undefined) {
      placement = { statusId, archived: record.archived, afterId, beforeId }
    }
  }
  return { workItemId, version: version as number, placement }
}

/** `base` is the card already on screen, for the board order only a list response carries. */
export function toCard(item: WorkItem, base?: WorkItemCard): WorkItemCard {
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
    parentId: item.parent?.id ?? null,
    blockedReason: item.blocked?.reason ?? null,
    openBlockerCount: item.blockedBy.filter((blocker) => !blocker.finished && !blocker.archived).length,
    subtaskCount: item.subtasks.length,
    subtasksDone: item.subtasks.filter((subtask) => subtask.finished).length,
    claimedBy: item.claim?.actor ?? null,
    boardOrder: base?.boardOrder ?? '',
  }
}

/** Filters this module can't evaluate on a card; a view using any of them refetches. */
const SERVER_ONLY_FILTERS = [
  'labelIds',
  'milestoneId',
  'parentId',
  'blocked',
  'ready',
  'claimed',
  'dueBefore',
  'dueAfter',
  'overdue',
  'includeDone',
  'sort',
  'groupBy',
] as const

/**
 * Whether the server would list this card under these filters, mirroring the
 * list query. `null` when it can't be told here (a search with wildcards).
 */
export function cardMatchesFilters(card: WorkItemCard, filters: WorkItemFilters): boolean | null {
  if (SERVER_ONLY_FILTERS.some((name) => filters[name as keyof WorkItemFilters] !== undefined)) return null
  if (Boolean(filters.archived) !== (card.archivedAt !== null)) return false
  if (filters.typeId && card.itemType.id !== filters.typeId) return false
  if (filters.statusId && card.status.id !== filters.statusId) return false
  if (filters.semanticCategory && card.status.semanticCategory !== filters.semanticCategory) return false
  if (filters.priority && card.priority !== filters.priority) return false
  if (filters.assigneeActorId && card.assignee?.actorId !== filters.assigneeActorId) return false
  if (filters.unassigned && card.assignee !== null) return false
  const search = filters.q?.trim()
  if (search) {
    if (/[%_\\]/.test(search)) return null
    const needle = search.toLowerCase()
    if (!card.title.toLowerCase().includes(needle) && !card.key.toLowerCase().includes(needle)) {
      return false
    }
  }
  return true
}

type Patched<T> = { data: T } | 'refetch' | 'unchanged'

/** Lists are newest number first. Replaces, removes or inserts the card in one page. */
export function patchList(
  list: WorkItemListResponse,
  card: WorkItemCard | null,
  workItemId: string,
  filters: WorkItemFilters,
): Patched<WorkItemListResponse> {
  const index = list.items.findIndex((item) => item.id === workItemId)
  const present = index >= 0 ? list.items[index] : undefined
  if (present && card && present.version > card.version) return 'unchanged'
  const matches = card ? cardMatchesFilters(card, filters) : false
  // A wildcard search: only the server knows whether the card still belongs.
  if (matches === null) return 'refetch'
  if (!matches) {
    if (!present) return 'unchanged'
    // A shorter page with more behind it would leave a gap: let the next task in.
    if (list.nextCursor !== null) return 'refetch'
    return {
      data: {
        ...list,
        items: list.items.filter((item) => item.id !== workItemId),
        total: Math.max(0, list.total - 1),
      },
    }
  }
  if (!card) return 'unchanged'
  if (present) return replaceAt(list, index, card)
  // Only the first page knows where newer tasks go; later pages refetch.
  if (filters.cursor) return 'refetch'
  const last = list.items.at(-1)
  if (list.nextCursor !== null) {
    // Belongs on a later page, or pushes this page's last task onto one.
    return last && card.number < last.number ? 'unchanged' : 'refetch'
  }
  const at = list.items.findIndex((item) => item.number < card.number)
  const items = [...list.items]
  items.splice(at < 0 ? items.length : at, 0, card)
  return { data: { ...list, items, total: list.total + 1 } }
}

function replaceAt(list: WorkItemListResponse, index: number, card: WorkItemCard): Patched<WorkItemListResponse> {
  const items = [...list.items]
  items[index] = card
  return { data: { ...list, items } }
}

function isListKey(key: readonly unknown[]) {
  return key[2] === 'work-items' && key[3] === 'list' && key.length === 5
}

/**
 * The task list page's views: pages of the list, its groups, the board, and
 * the label and milestone task counts. Their sorts, groups, totals and manual
 * board order are computed by the server, so they refetch rather than patch.
 */
const SERVER_ORDERED_VIEWS = new Set(['pages', 'groups', 'board', 'labels', 'milestones'])

function isServerOrderedKey(key: readonly unknown[]) {
  return key[2] === 'work-items' && SERVER_ORDERED_VIEWS.has(key[3] as string)
}

function filtersOf(key: readonly unknown[]): WorkItemFilters {
  const filters = key[4]
  return filters && typeof filters === 'object' ? (filters as WorkItemFilters) : {}
}

function cardIn(query: Query, workItemId: string): WorkItemCard | undefined {
  const data = query.state.data as Partial<WorkItemListResponse> | undefined
  return data?.items?.find((item) => item.id === workItemId)
}

/**
 * Patches every cached plain list and detail view for one changed task, and
 * refetches the views it can't place (including the task list page's own). Returns once the caches are set.
 */
export async function applyWorkItemChange(
  queryClient: QueryClient,
  projectId: string,
  change: WorkItemChange,
): Promise<void> {
  const root = queryKeys.workItemsRoot(projectId)
  const cached = queryClient.getQueryCache().findAll({ queryKey: root })
  for (const query of cached) {
    if (isServerOrderedKey(query.queryKey)) {
      void queryClient.invalidateQueries({ queryKey: query.queryKey, exact: true })
    }
  }
  const views = cached.filter((query) => isListKey(query.queryKey))
  const detailKey = queryKeys.workItem(projectId, change.workItemId)
  const cachedDetail = queryClient.getQueryData<WorkItem>(detailKey)
  const shown = views.some((query) => cardIn(query, change.workItemId))
  // Nothing on screen holds this task and no view could gain it: nothing to do.
  if (!cachedDetail && views.length === 0) return

  const generation = currentQueryScopeGeneration()
  let item: WorkItem | null
  if (cachedDetail && cachedDetail.version >= change.version) {
    item = cachedDetail
  } else if (!cachedDetail && !shown && views.every((query) => isListKey(query.queryKey) && filtersOf(query.queryKey).cursor)) {
    return
  } else {
    try {
      item = await getWorkItem(projectId, change.workItemId)
    } catch (error) {
      if (error instanceof ApiError && (error.status === 404 || error.status === 403)) {
        item = null
      } else {
        if (generation !== currentQueryScopeGeneration()) return
        void queryClient.invalidateQueries({ queryKey: root })
        return
      }
    }
    // The session ended or changed while this waited: its caches are gone.
    if (generation !== currentQueryScopeGeneration()) return
  }

  if (item) {
    // Read the cache again: a newer event's fetch may have landed while this one waited.
    const latest = queryClient.getQueryData<WorkItem>(detailKey)
    if (latest && latest.version > item.version) item = latest
    else queryClient.setQueryData(detailKey, item)
  } else if (cachedDetail) {
    void queryClient.invalidateQueries({ queryKey: detailKey, exact: true })
  }

  // A newer version outran this event: its neighbours are stale, and the newer
  // event places the card itself.
  if (item && item.version > change.version) return
  for (const query of views) {
    const data = query.state.data
    if (!data) continue
    const filters = filtersOf(query.queryKey)
    const card = item ? toCard(item, cardIn(query, change.workItemId)) : null
    const result = patchList(data as WorkItemListResponse, card, change.workItemId, filters)
    if (result === 'refetch') void queryClient.invalidateQueries({ queryKey: query.queryKey, exact: true })
    else if (result !== 'unchanged') queryClient.setQueryData(query.queryKey, result.data)
  }
}
