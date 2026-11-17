import { keepPreviousData, useInfiniteQuery, useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { callOp, type OpInput, type OpOutput } from '../api/ops'
import { normalizeField, normalizeReferences, normalizeValues, type DateBucket, type FieldClause, type FieldDefinition, type FieldReference, type FieldValue, type FieldValues } from '../fields/fieldModel'
import type { ViewKind } from '../views/kinds'
import { collectionKeys } from './keys'

/**
 * Collections (AWD-90) on the web side: a project's own record types, each
 * with fields (the same types as task fields), records and saved views. The
 * server's `collections::http` is the other half; it checks every value,
 * finds records with its own query and pages through them with a cursor.
 */

export type CollectionSummary = {
  id: string
  name: string
  description: string
  /** What the title column is called, such as Name or Version. */
  titleName: string
  /** Records that aren't archived. */
  records: number
  position: number
  archived: boolean
  version: number
  updatedAt: string
  /** "tasks" for the project's built-in Tasks collection: its records are tasks, changed on task screens. */
  builtin?: 'tasks' | null
  /** Its records are tasks (AWD-110): a status, assignee and the rest, a key like AWD-12, and a task page. */
  actsLikeTask?: boolean
}

export type SortRule = { field: string; direction: 'asc' | 'desc' }
export type ViewColumn = { field: string; hidden?: boolean; width?: number }
/** A group of a view's filter: clauses (or further groups) joined by "and" or by "or". */
export type FilterGroup = { and: FilterItem[] } | { or: FilterItem[] }
export type FilterItem = FieldClause | FilterGroup
export type ViewConfig = {
  /** The clauses that must all hold, which the toolbar shows and edits one by one. */
  filters: FieldClause[]
  /**
   * Groups that must hold beside them ("this or that", T1.5), kept as the
   * server sent them: the toolbar shows them but doesn't edit inside them,
   * and they go back with every change, so changing a sort never drops them.
   */
  filterGroups?: FilterGroup[]
  sort: SortRule[]
  groupBy: string | null
  /** Grouped by a date: by day, week or month (the server's default). */
  groupBucket?: DateBucket | null
  columns: ViewColumn[]
  titleWidth: number | null
  /** The date field a calendar puts records on. */
  dateField: string | null
}

/** How a view lays out its records. */
/** The kinds of view a collection draws today, of the one list (views/kinds.ts). */
export type ViewType = Extract<ViewKind, 'table' | 'board' | 'gallery' | 'calendar'>
export const VIEW_TYPES: readonly ViewType[] = ['table', 'board', 'gallery', 'calendar']
const asViewType = (value: unknown): ViewType => (VIEW_TYPES as readonly unknown[]).includes(value) ? (value as ViewType) : 'table'

export type CollectionView = { id: string; name: string; type: ViewType; config: ViewConfig; position: number; version: number; /** Everyone's; false for the person's own. */ shared: boolean }

export type CollectionBody = {
  collection: CollectionSummary
  /** Archived ones too, marked. */
  fields: FieldDefinition[]
  views: CollectionView[]
  archivedRecords: number
  canEditRecords: boolean
  canBuild: boolean
  /** Adds, changes and archives its fields; for Tasks, the task field settings' permission too. */
  canChangeFields: boolean
}

/** The built-in fields of a collection whose records are tasks (status, assignee, priority, dates, labels): changed in the task settings, not here. */
export const BUILT_IN_TASK_FIELDS: ReadonlySet<string> = new Set(['1', '2', '3', '4', '5', '6'].map((n) => `0192f5c0-0000-7000-8000-00000000000${n}`))

export type PersonRef = { id: string; name: string }

export type CollectionRecord = {
  id: string
  number: number
  title: string
  values: FieldValues
  createdBy: PersonRef
  updatedBy: PersonRef
  archived: boolean
  version: number
  createdAt: string
  updatedAt: string
  /** The group it's listed under, when the list is grouped. */
  group: { key: string; label: string } | null
}

export type GroupCount = { key: string; label: string; count: number; color: string | null }

export type RecordPage = {
  records: CollectionRecord[]
  references: FieldReference[]
  total: number
  groups: GroupCount[]
  nextCursor: string | null
}

/** One value as it read before and after a change. */
export type HistoryChange = { field: string; name: string; before: unknown; after: unknown }
export type HistoryEntry = { id: string; action: string; actor: string; via: string | null; changes: HistoryChange[]; version: number; at: string }

export type RecordBody = { record: CollectionRecord; references: FieldReference[]; history: HistoryEntry[] }

// ---------------------------------------------------------------------------
// Reading the server's JSON

type Wire = Record<string, unknown>
const asRecord = (value: unknown): Wire => (value && typeof value === 'object' && !Array.isArray(value) ? (value as Wire) : {})
const asString = (value: unknown) => (typeof value === 'string' ? value : '')
const asNumber = (value: unknown, fallback = 0) => (typeof value === 'number' && Number.isFinite(value) ? value : fallback)
const asArray = (value: unknown) => (Array.isArray(value) ? value : [])

export function normalizeCollection(value: unknown): CollectionSummary {
  const raw = asRecord(value)
  return {
    id: asString(raw.id),
    name: asString(raw.name),
    description: asString(raw.description),
    titleName: asString(raw.titleName) || 'Name',
    records: asNumber(raw.records),
    position: asNumber(raw.position),
    archived: raw.archived === true,
    version: asNumber(raw.version, 1),
    updatedAt: asString(raw.updatedAt),
    builtin: raw.builtin === 'tasks' ? 'tasks' : null,
    actsLikeTask: raw.actsLikeTask === true || raw.builtin === 'tasks',
  }
}

function asClause(value: unknown): FieldClause | null {
  const clause = asRecord(value)
  if (typeof clause.field !== 'string' || typeof clause.op !== 'string') return null
  const values = asArray(clause.values).filter((item): item is string => typeof item === 'string')
  return { field: clause.field, op: clause.op as FieldClause['op'], ...(Array.isArray(clause.values) ? { values } : {}) }
}

function asFilterItem(value: unknown): FilterItem | null {
  const raw = asRecord(value)
  if (Array.isArray(raw.and)) return { and: raw.and.flatMap((item) => asFilterItem(item) ?? []) }
  if (Array.isArray(raw.or)) return { or: raw.or.flatMap((item) => asFilterItem(item) ?? []) }
  return asClause(value)
}

const isGroup = (item: FilterItem): item is FilterGroup => 'and' in item || 'or' in item

/**
 * A stored filter (a list of clauses, or a tree of "and" and "or" groups) as
 * the clauses that all hold and the groups beside them.
 */
export function splitFilter(value: unknown): Pick<ViewConfig, 'filters' | 'filterGroups'> {
  const top = Array.isArray(value) ? { and: value } : asRecord(value)
  const items = Array.isArray(top.and)
    ? top.and.flatMap((item) => asFilterItem(item) ?? [])
    : Array.isArray(top.or)
      ? [{ or: top.or.flatMap((item) => asFilterItem(item) ?? []) }]
      : []
  const groups = items.filter(isGroup)
  return { filters: items.filter((item): item is FieldClause => !isGroup(item)), ...(groups.length ? { filterGroups: groups } : {}) }
}

/** A view's filter as the server takes it: a plain list when it's only clauses that all hold. */
export function filterJson(config: Pick<ViewConfig, 'filters' | 'filterGroups'>): FieldClause[] | FilterGroup {
  const groups = config.filterGroups ?? []
  if (groups.length === 0) return config.filters
  if (config.filters.length === 0 && groups.length === 1) return groups[0]
  return { and: [...config.filters, ...groups] }
}

/** Whether a view's filter narrows anything. */
export const hasFilter = (config: Pick<ViewConfig, 'filters' | 'filterGroups'>) => config.filters.length > 0 || Boolean(config.filterGroups?.length)

export function normalizeConfig(value: unknown): ViewConfig {
  const raw = asRecord(value)
  return {
    ...splitFilter(raw.filters),
    sort: asArray(raw.sort).flatMap((entry) => {
      const rule = asRecord(entry)
      return typeof rule.field === 'string' ? [{ field: rule.field, direction: rule.direction === 'desc' ? ('desc' as const) : ('asc' as const) }] : []
    }),
    groupBy: typeof raw.groupBy === 'string' && raw.groupBy ? raw.groupBy : null,
    groupBucket: raw.groupBucket === 'day' || raw.groupBucket === 'week' || raw.groupBucket === 'month' ? raw.groupBucket : null,
    columns: asArray(raw.columns).flatMap((entry) => {
      const column = asRecord(entry)
      if (typeof column.field !== 'string') return []
      return [{ field: column.field, ...(column.hidden === true ? { hidden: true } : {}), ...(typeof column.width === 'number' ? { width: column.width } : {}) }]
    }),
    titleWidth: typeof raw.titleWidth === 'number' ? raw.titleWidth : null,
    dateField: typeof raw.dateField === 'string' && raw.dateField ? raw.dateField : null,
  }
}

function normalizeView(value: unknown): CollectionView {
  const raw = asRecord(value)
  return {
    id: asString(raw.id),
    name: asString(raw.name),
    type: asViewType(raw.type),
    // The view's settings sit beside its name, as the server flattens them.
    config: normalizeConfig(raw),
    position: asNumber(raw.position),
    version: asNumber(raw.version, 1),
    shared: raw.shared !== false,
  }
}

function normalizeBody(value: unknown): CollectionBody {
  const raw = asRecord(value)
  return {
    collection: normalizeCollection(raw.collection),
    fields: asArray(raw.fields).map(normalizeField),
    views: asArray(raw.views).map(normalizeView),
    archivedRecords: asNumber(raw.archivedRecords),
    canEditRecords: raw.canEditRecords === true,
    canBuild: raw.canBuild === true,
    canChangeFields: raw.canChangeFields === true,
  }
}

const person = (value: unknown): PersonRef => {
  const raw = asRecord(value)
  return { id: asString(raw.id), name: asString(raw.name) }
}

export function normalizeRecord(value: unknown): CollectionRecord {
  const raw = asRecord(value)
  const group = asRecord(raw.group)
  return {
    id: asString(raw.id),
    number: asNumber(raw.number),
    title: asString(raw.title),
    values: normalizeValues(raw.values),
    createdBy: person(raw.createdBy),
    updatedBy: person(raw.updatedBy),
    archived: raw.archived === true,
    version: asNumber(raw.version, 1),
    createdAt: asString(raw.createdAt),
    updatedAt: asString(raw.updatedAt),
    group: typeof group.key === 'string' ? { key: group.key, label: asString(group.label) } : null,
  }
}

function normalizePage(value: unknown): RecordPage {
  const raw = asRecord(value)
  return {
    records: asArray(raw.records).map(normalizeRecord),
    references: normalizeReferences(raw.references),
    total: asNumber(raw.total),
    groups: asArray(raw.groups).map((entry) => {
      const group = asRecord(entry)
      return { key: asString(group.key), label: asString(group.label), count: asNumber(group.count), color: typeof group.color === 'string' ? group.color : null }
    }),
    nextCursor: typeof raw.nextCursor === 'string' && raw.nextCursor ? raw.nextCursor : null,
  }
}

function normalizeRecordBody(value: unknown): RecordBody {
  const raw = asRecord(value)
  return {
    record: normalizeRecord(raw.record),
    references: normalizeReferences(raw.references),
    history: asArray(raw.history).map((entry) => {
      const item = asRecord(entry)
      return {
        id: asString(item.id),
        action: asString(item.action),
        actor: asString(item.actor),
        via: typeof item.via === 'string' && item.via ? item.via : null,
        changes: asArray(item.changes).map((change) => {
          const parts = asRecord(change)
          return { field: asString(parts.field), name: asString(parts.name), before: parts.before ?? null, after: parts.after ?? null }
        }),
        version: asNumber(item.version),
        at: asString(item.at),
      }
    }),
  }
}

// ---------------------------------------------------------------------------
// Requests

const projectPath = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/collections`
export const collectionPath = (projectId: string, collectionId: string) => `${projectPath(projectId)}/${encodeURIComponent(collectionId)}`

/** Where a collection's fields are added and changed, for the fields kit's dialogs. */
export const fieldScope = (projectId: string, collectionId: string) => ({ projectId, path: `${collectionPath(projectId, collectionId)}/fields` })

export { collectionKeys } from './keys'

export async function listCollections(projectId: string, includeArchived = false) {
  const payload = await requestJson<{ collections?: unknown[]; canBuild?: boolean }>(`${projectPath(projectId)}${includeArchived ? '?includeArchived=true' : ''}`)
  return { collections: asArray(payload.collections).map(normalizeCollection), canBuild: payload.canBuild === true }
}

export async function createCollection(projectId: string, input: { name: string; description: string; titleName: string; idempotencyKey: string }) {
  const { idempotencyKey, ...json } = input
  return normalizeBody(await requestJson<unknown>(projectPath(projectId), { method: 'POST', idempotencyKey, json }))
}

export async function getCollection(projectId: string, collectionId: string) {
  return normalizeBody(await requestJson<unknown>(collectionPath(projectId, collectionId)))
}

export async function updateCollection(
  projectId: string,
  collectionId: string,
  input: { expectedVersion: number; name?: string; description?: string; titleName?: string; archived?: boolean; actsLikeTask?: boolean },
) {
  return normalizeBody(await requestJson<unknown>(collectionPath(projectId, collectionId), { method: 'PATCH', idempotencyKey: crypto.randomUUID(), json: input }))
}

/** A view's settings as the server stores them: no empty parts. */
export function configJson(config: ViewConfig) {
  return {
    ...(hasFilter(config) ? { filters: filterJson(config) } : {}),
    ...(config.sort.length ? { sort: config.sort } : {}),
    ...(config.groupBy ? { groupBy: config.groupBy } : {}),
    ...(config.groupBy && config.groupBucket ? { groupBucket: config.groupBucket } : {}),
    ...(config.columns.length ? { columns: config.columns } : {}),
    ...(config.titleWidth ? { titleWidth: config.titleWidth } : {}),
    ...(config.dateField ? { dateField: config.dateField } : {}),
  }
}

function viewFromEntry(entry: OpOutput<'view_create'>): CollectionView {
  return {
    id: entry.id,
    name: entry.name,
    type: asViewType(entry.type),
    config: normalizeConfig(entry.config),
    position: entry.position,
    version: entry.version,
    shared: entry.shared,
  }
}

/** Adds a view: shared with everyone who sees the collection, or the person's own. */
export async function createView(collectionId: string, input: { name: string; type?: ViewType; config: ViewConfig; shared: boolean }) {
  return viewFromEntry(
    await callOp('view_create', {
      collection: collectionId,
      name: input.name,
      type: input.type ?? 'table',
      config: configJson(input.config) as OpInput<'view_create'>['config'],
      shared: input.shared,
    }),
  )
}

export async function updateView(view: CollectionView, change: { name?: string; config?: ViewConfig }) {
  return viewFromEntry(
    await callOp(
      'view_update',
      {
        view: view.id,
        ...(change.name ? { name: change.name } : {}),
        ...(change.config ? { config: configJson(change.config) as OpInput<'view_update'>['config'] } : {}),
      },
      { expectedVersion: view.version },
    ),
  )
}

export async function deleteView(viewId: string) {
  await callOp('view_delete', { view: viewId })
}

/** What a list of records asks for: a saved view, and whatever the person changed on top of it. */
export type RecordsQuery = {
  view: string | null
  filters?: FieldClause[] | FilterGroup
  sort?: SortRule[]
  groupBy?: string | null
  groupBucket?: DateBucket | null
  search?: string
  archived?: 'only'
  /** One group of the grouping: a board's column. */
  group?: string
  /** Records whose date field is from `from` up to `to` (YYYY-MM-DD, `to` left out): a calendar's weeks. */
  dates?: { field: string; from: string; to: string }
}

/** The records list's parameters, in a fixed order, so one query has one cache entry. */
export function recordsParams(query: RecordsQuery) {
  const params = new URLSearchParams()
  if (query.view) params.set('view', query.view)
  if (query.filters) params.set('filters', JSON.stringify(query.filters))
  if (query.sort) params.set('sort', JSON.stringify(query.sort))
  if (query.groupBy !== undefined) params.set('groupBy', query.groupBy ?? '')
  if (query.groupBy && query.groupBucket) params.set('groupBucket', query.groupBucket)
  if (query.search?.trim()) params.set('search', query.search.trim())
  if (query.archived) params.set('archived', query.archived)
  if (query.group !== undefined) params.set('group', query.group)
  if (query.dates) {
    params.set('dateField', query.dates.field)
    params.set('from', query.dates.from)
    params.set('to', query.dates.to)
  }
  return params.toString()
}

export const RECORDS_PAGE = 100

export async function listRecords(projectId: string, collectionId: string, params: string, cursor: string | null, limit = RECORDS_PAGE) {
  const query = new URLSearchParams(params)
  query.set('limit', String(limit))
  if (cursor) query.set('cursor', cursor)
  return normalizePage(await requestJson<unknown>(`${collectionPath(projectId, collectionId)}/records?${query.toString()}`))
}

export async function getRecord(projectId: string, collectionId: string, recordId: string) {
  return normalizeRecordBody(await requestJson<unknown>(`${collectionPath(projectId, collectionId)}/records/${encodeURIComponent(recordId)}`))
}

export async function createRecord(projectId: string, collectionId: string, input: { title: string; values?: Record<string, FieldValue> }) {
  return normalizeRecordBody(
    await requestJson<unknown>(`${collectionPath(projectId, collectionId)}/records`, {
      method: 'POST',
      idempotencyKey: crypto.randomUUID(),
      json: { title: input.title, values: input.values ?? {} },
    }),
  )
}

/** A change to one record: its title, some values (null clears one), or archiving it. */
export type RecordChange = { title?: string; values?: Record<string, FieldValue | null>; archived?: boolean }

export async function updateRecord(projectId: string, collectionId: string, recordId: string, expectedVersion: number, change: RecordChange) {
  return normalizeRecordBody(
    await requestJson<unknown>(`${collectionPath(projectId, collectionId)}/records/${encodeURIComponent(recordId)}`, {
      method: 'PATCH',
      idempotencyKey: crypto.randomUUID(),
      json: { expectedVersion, ...change },
    }),
  )
}

// ---------------------------------------------------------------------------
// Hooks

export function useCollections(projectId: string, includeArchived = false) {
  return useQuery({ queryKey: collectionKeys.list(projectId, includeArchived), queryFn: () => listCollections(projectId, includeArchived), enabled: Boolean(projectId) })
}

export function useCollection(projectId: string, collectionId: string) {
  return useQuery({ queryKey: collectionKeys.one(projectId, collectionId), queryFn: () => getCollection(projectId, collectionId), enabled: Boolean(collectionId) })
}

/**
 * A list of records, a page at a time as it scrolls. While a changed filter,
 * sort or search loads, the list it replaces stays (one collection's: the
 * page that shows it remounts for another).
 */
export function useRecords(projectId: string, collectionId: string, params: string, enabled = true, limit = RECORDS_PAGE) {
  return useInfiniteQuery({
    queryKey: collectionKeys.records(projectId, collectionId, limit === RECORDS_PAGE ? params : `${params}&limit=${limit}`),
    queryFn: ({ pageParam }) => listRecords(projectId, collectionId, params, pageParam, limit),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    placeholderData: keepPreviousData,
    enabled: enabled && Boolean(collectionId),
  })
}

export function useRecord(projectId: string, collectionId: string, recordId: string | null) {
  return useQuery({
    queryKey: collectionKeys.record(projectId, collectionId, recordId ?? ''),
    queryFn: () => getRecord(projectId, collectionId, recordId ?? ''),
    enabled: Boolean(recordId),
  })
}
