import { MAX_FIELD_FILTERS_CHARACTERS, parseClauses } from '../fields/fieldModel'
import { workItemPriorities } from './constants'
import type { BoardColumnsBy, WorkItemFilters, WorkItemGroupBy, WorkItemSort } from './types'

/**
 * The task list's state lives in the URL: filters, sort, grouping, the view,
 * the board's columns and lanes, and the task open in the side panel. This
 * module is the one place that reads and writes it, so a link always reopens
 * the same view.
 */

export type ViewMode = 'list' | 'board'
export type LaneBy = 'none' | 'assignee' | 'priority' | 'milestone' | 'type'

/** Filters that go to the server unchanged, keyed by their query parameter. */
export type FilterValues = Omit<WorkItemFilters, 'cursor' | 'limit' | 'sort' | 'groupBy' | 'unassigned'>

export type ListState = {
  view: ViewMode
  filters: FilterValues
  sort: WorkItemSort
  groupBy: WorkItemGroupBy | null
  columnsBy: BoardColumnsBy
  lanes: LaneBy
  /** The task open in the side panel: its id or key. */
  task: string | null
  /** The saved view this started from, kept while you change it so it can be saved back. */
  savedView: string | null
  /** List columns to show; every column when null. */
  fields: string[] | null
  /** Done and canceled work is open below the rest (list) or as columns (board), not tucked behind its toggle. */
  showDone: boolean
}

export const SORTS: Array<{ value: WorkItemSort; label: string }> = [
  { value: 'newest', label: 'Newest' },
  { value: 'oldest', label: 'Oldest' },
  { value: 'updated', label: 'Recently updated' },
  { value: 'priority', label: 'Priority' },
  { value: 'due', label: 'Due date' },
  { value: 'title', label: 'Title' },
  { value: 'manual', label: 'Manual (board order)' },
]

export const GROUPINGS: Array<{ value: WorkItemGroupBy; label: string }> = [
  { value: 'status', label: 'Status' },
  { value: 'assignee', label: 'Assignee' },
  { value: 'priority', label: 'Priority' },
  { value: 'label', label: 'Label' },
  { value: 'milestone', label: 'Milestone' },
  { value: 'parent', label: 'Parent' },
  { value: 'type', label: 'Type' },
]

export const COLUMN_FIELDS: Array<{ value: BoardColumnsBy; label: string }> = [
  { value: 'status', label: 'Status' },
  { value: 'assignee', label: 'Assignee' },
  { value: 'priority', label: 'Priority' },
  { value: 'milestone', label: 'Milestone' },
  { value: 'type', label: 'Type' },
  { value: 'label', label: 'Label' },
]

export const LANE_FIELDS: Array<{ value: LaneBy; label: string }> = [
  { value: 'none', label: 'No lanes' },
  { value: 'assignee', label: 'Assignee' },
  { value: 'priority', label: 'Priority' },
  { value: 'milestone', label: 'Milestone' },
  { value: 'type', label: 'Type' },
]

/** List columns a view can hide; select and title always show. */
export const VIEW_FIELDS = ['priority', 'key', 'status', 'details', 'due', 'assignee']

export const SEMANTIC_CATEGORIES = ['backlog', 'unstarted', 'started', 'review', 'verification', 'completed', 'canceled']

const ID = /^[A-Za-z0-9_.:-]{1,100}$/
const DATE = /^\d{4}-\d{2}-\d{2}$/
/** The most values one filter names, as the server allows. */
export const MAX_FILTER_VALUES = 20

type Rule = (value: string) => boolean

const id: Rule = (value) => value !== 'none' && ID.test(value)
const idOrNone: Rule = (value) => value === 'none' || ID.test(value)
const isCategory: Rule = (value) => SEMANTIC_CATEGORIES.includes(value)
const isPriority: Rule = (value) => workItemPriorities.some((priority) => priority.value === value)
const isTrue: Rule = (value) => value === 'true'
const isBoolean: Rule = (value) => value === 'true' || value === 'false'
const isDate: Rule = (value) => DATE.test(value)
/** Up to 20 values, comma separated, each passing `part`. */
const several =
  (part: Rule): Rule =>
  (value) => {
    const parts = value.split(',')
    return parts.length <= MAX_FILTER_VALUES && parts.every(part)
  }

/** Every filter the URL can carry, in the order chips show them. */
export const FILTER_RULES: Record<keyof FilterValues, Rule> = {
  q: (value) => value.trim().length > 0 && value.length <= 120,
  statusId: several(id),
  notStatusId: several(id),
  semanticCategory: several(isCategory),
  notSemanticCategory: several(isCategory),
  priority: several(isPriority),
  notPriority: several(isPriority),
  assigneeActorId: several(idOrNone),
  notAssigneeActorId: several(idOrNone),
  // All of these labels; "none" only on its own.
  labelIds: (value) => value === 'none' || several(id)(value),
  anyLabelIds: several(idOrNone),
  notLabelIds: several(idOrNone),
  milestoneId: several(idOrNone),
  notMilestoneId: several(idOrNone),
  parentId: idOrNone,
  typeId: several(id),
  notTypeId: several(id),
  blocked: isBoolean,
  ready: isTrue,
  claimed: isBoolean,
  dueAfter: isDate,
  dueBefore: isDate,
  overdue: isTrue,
  includeDone: (value) => value === 'false',
  archived: isTrue,
  fieldFilters: (value) => value.length <= MAX_FIELD_FILTERS_CHARACTERS && parseClauses(value).length > 0,
}

export const FILTER_KEYS = Object.keys(FILTER_RULES) as Array<keyof FilterValues>

/** Filters that name several values; kept sorted so one filter has one URL. */
const LIST_KEYS = new Set<keyof FilterValues>([
  'statusId',
  'notStatusId',
  'semanticCategory',
  'notSemanticCategory',
  'priority',
  'notPriority',
  'assigneeActorId',
  'notAssigneeActorId',
  'labelIds',
  'anyLabelIds',
  'notLabelIds',
  'milestoneId',
  'notMilestoneId',
  'typeId',
  'notTypeId',
])

/** Each value once, sorted, "none" last: one URL for one filter. */
export function canonicalValues(values: readonly string[]): string[] {
  return [...new Set(values)].sort((a, b) => (a === 'none' ? 1 : b === 'none' ? -1 : a < b ? -1 : a > b ? 1 : 0))
}

function canonicalList(value: string) {
  return canonicalValues(value.split(',')).join(',')
}

/** A custom field's sort (`field:<id>`, or `…:desc`), group or column (`field:<id>`). */
const FIELD_SORT = /^field:[A-Za-z0-9-]{1,64}(:desc)?$/
const FIELD_KEY = /^field:[A-Za-z0-9-]{1,64}$/

function oneOf<T extends string>(value: string | null, options: Array<{ value: T }>, fallback: T): T {
  return options.some((option) => option.value === value) ? (value as T) : fallback
}

export function readListState(parameters: URLSearchParams): ListState {
  const filters: Record<string, unknown> = {}
  for (const key of FILTER_KEYS) {
    const value = parameters.get(key)
    if (value === null || !FILTER_RULES[key](value)) continue
    filters[key] = LIST_KEYS.has(key) ? canonicalList(value) : value === 'true' && key !== 'blocked' && key !== 'claimed' ? true : value
  }
  // Older links: "unassigned" is the assignee "none", as the server reads it,
  // and "no labels" is the one value of "any of these labels".
  if (parameters.get('unassigned') === 'true') {
    const assignees = typeof filters.assigneeActorId === 'string' ? `${filters.assigneeActorId},none` : 'none'
    if (FILTER_RULES.assigneeActorId(assignees)) filters.assigneeActorId = canonicalList(assignees)
  }
  if (filters.labelIds === 'none') {
    delete filters.labelIds
    filters.anyLabelIds = typeof filters.anyLabelIds === 'string' ? canonicalList(`${filters.anyLabelIds},none`) : 'none'
  }
  const groupBy = parameters.get('groupBy')
  const task = parameters.get('task')
  const savedView = parameters.get('savedView')
  const fields = parameters.get('fields')
  const columnsBy = oneOf(parameters.get('columnsBy'), COLUMN_FIELDS, 'status')
  const lanes = oneOf(parameters.get('lanes'), LANE_FIELDS, 'none')
  return {
    view: parameters.get('view') === 'board' ? 'board' : 'list',
    filters: filters as FilterValues,
    sort: FIELD_SORT.test(parameters.get('sort') ?? '') ? (parameters.get('sort') as WorkItemSort) : oneOf(parameters.get('sort'), SORTS, 'newest'),
    groupBy: GROUPINGS.some((option) => option.value === groupBy) || FIELD_KEY.test(groupBy ?? '') ? (groupBy as WorkItemGroupBy) : null,
    columnsBy,
    // Lanes by the column field would leave only the diagonal cells.
    lanes: lanes === columnsBy ? 'none' : lanes,
    task: task && task.length <= 100 ? task : null,
    savedView: savedView && ID.test(savedView) ? savedView : null,
    fields: fields === null ? null : fields.split(',').filter((field) => VIEW_FIELDS.includes(field) || FIELD_KEY.test(field)),
    showDone: parameters.get('showDone') === 'true',
  }
}

/** The URL for a state. Defaults are left out, so the plain page has no query at all. */
export function writeListState(state: ListState): URLSearchParams {
  const parameters = new URLSearchParams()
  if (state.view !== 'list') parameters.set('view', state.view)
  for (const key of FILTER_KEYS) {
    const value = state.filters[key]
    if (value === undefined || value === '' || value === false) continue
    parameters.set(key, String(value))
  }
  if (state.sort !== 'newest') parameters.set('sort', state.sort)
  if (state.groupBy) parameters.set('groupBy', state.groupBy)
  if (state.columnsBy !== 'status') parameters.set('columnsBy', state.columnsBy)
  if (state.lanes !== 'none') parameters.set('lanes', state.lanes)
  if (state.task) parameters.set('task', state.task)
  if (state.fields) parameters.set('fields', state.fields.join(','))
  if (state.showDone) parameters.set('showDone', 'true')
  if (state.savedView) parameters.set('savedView', state.savedView)
  return parameters
}

export function withFilter<K extends keyof FilterValues>(state: ListState, key: K, value: FilterValues[K] | undefined): ListState {
  const filters = { ...state.filters }
  if (value === undefined || value === '' || value === false) delete filters[key]
  else filters[key] = value
  return { ...state, filters }
}

export function clearFilters(state: ListState): ListState {
  return { ...state, filters: {} }
}

export function activeFilterKeys(state: ListState) {
  return FILTER_KEYS.filter((key) => key !== 'q' && state.filters[key] !== undefined)
}

/** What GET /work-items gets for the list view. */
export function listQuery(state: ListState): WorkItemFilters {
  return {
    ...state.filters,
    sort: state.sort,
    ...(state.groupBy ? { groupBy: state.groupBy } : {}),
  }
}

/** What GET /work-items/board gets. */
export function boardQuery(state: ListState): WorkItemFilters & { columnsBy: BoardColumnsBy } {
  return { ...state.filters, columnsBy: state.columnsBy }
}

/** What the Project remembers so the page reopens the same way: everything but the open task. */
export function rememberedState(state: ListState): Record<string, string> {
  const parameters = writeListState({ ...state, task: null, savedView: null })
  return Object.fromEntries(parameters.entries())
}

/** Saved navigation filters back into URL parameters, dropping anything no longer valid. */
export function parametersFromRemembered(saved: Record<string, unknown>): URLSearchParams {
  const raw = new URLSearchParams()
  for (const [key, value] of Object.entries(saved)) {
    if (typeof value === 'string') raw.set(key, value)
    else if (value === true) raw.set(key, 'true')
  }
  return writeListState({ ...readListState(raw), task: null, savedView: null })
}
