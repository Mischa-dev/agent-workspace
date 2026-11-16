/**
 * Custom fields (AWD-89): the field types every kind of record shares, on
 * the web side. The server's `crate::fields` is the other half. Tasks use
 * them now; collections (AWD-90) and modules' records will reuse the same
 * types, display and filters, so nothing here knows about tasks.
 *
 * A record's values are an object keyed by field ID. A field with no value
 * has no key. Values that name something hold its ID (an option, a person, a
 * task or a page); `FieldReference`s, sent beside the values, say what those
 * people, tasks and pages are called.
 */

/**
 * A field's type: the one list (T1.2, T1.13a), in `fieldTypes.ts`;
 * `modules/contract.ts` re-exports it. The server's
 * `crate::fields::FieldType` is the same list. `FIELD_TYPES` are the ones a
 * field can be made with; `people` and `labels` are named by manifests only,
 * for now. System types (`isSystemType`) show the record's own columns and
 * take no values.
 */
export type { FieldType, FieldOp } from './fieldTypes'
import type { FieldDecl } from '../modules/contract'
import { FIELD_TYPE_INFO, FIELD_TYPE_LIST, isFieldType, typeInfo, type FieldOp, type FieldType } from './fieldTypes'

/** What kind of status a status option is, in workflow order (the server's STATUS_CATEGORIES). */
export const STATUS_CATEGORIES = ['backlog', 'unstarted', 'started', 'review', 'verification', 'completed', 'canceled'] as const
export type StatusCategory = (typeof STATUS_CATEGORIES)[number]

export const CATEGORY_LABELS: Record<StatusCategory, string> = {
  backlog: 'Backlog',
  unstarted: 'Not started',
  started: 'Started',
  review: 'In review',
  verification: 'Verifying',
  completed: 'Done',
  canceled: 'Canceled',
}

export type FieldOption = { id: string; name: string; color: string; archived: boolean; category?: StatusCategory | null }

export type FieldDefinition = {
  id: string
  name: string
  description: string
  type: FieldType
  /** Choice fields: every option in order, archived ones too. */
  options: FieldOption[]
  /** Link fields: what they link to. */
  target: LinkTarget | null
  /** Record links: the collection whose records they hold. */
  collection?: string | null
  /** 1 is first. */
  position: number
  archived: boolean
  version: number
}

export type FieldValue = string | number | boolean | string[]
export type FieldValues = Record<string, FieldValue>

export type LinkTarget = 'task' | 'page' | 'record'

export type FieldReference = { id: string; kind: 'person' | 'task' | 'page' | 'record'; label: string; key: string | null }

/** One field filter, with stored values (option IDs, actor IDs, "true", numbers, dates). */
export type FieldClause = { field: string; op: FieldOp; values?: string[] }

/** The types a field can be made with, from the registry. `tasks: false` ones are for collections only. */
export const FIELD_TYPES: Array<{ type: FieldType; label: string; hint: string; tasks?: false }> = FIELD_TYPE_LIST.filter(
  (type) => FIELD_TYPE_INFO[type].creatable,
).map((type) => {
  const { label, hint, tasks } = FIELD_TYPE_INFO[type]
  return tasks ? { type, label, hint } : { type, label, hint, tasks: false as const }
})

/** Whether a type shows the record's own columns: nobody fills it in. */
export function isSystemType(type: FieldType) {
  return typeInfo(type).system
}

/** The type it filters, sorts and groups like (the server's `behaves_as`). */
export function behavesAs(type: FieldType): FieldType {
  return typeInfo(type).behavesAs
}

export function typeLabel(type: FieldType) {
  return FIELD_TYPE_INFO[type]?.label ?? type
}

// ---------------------------------------------------------------------------
// Reading the server's JSON

type Wire = Record<string, unknown>
const asRecord = (value: unknown): Wire =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Wire) : {}
const asString = (value: unknown) => (typeof value === 'string' ? value : '')

function fieldType(value: unknown): FieldType {
  return isFieldType(value) ? value : 'text'
}

export function normalizeField(value: unknown): FieldDefinition {
  const field = asRecord(value)
  return {
    id: asString(field.id),
    name: asString(field.name),
    description: asString(field.description),
    type: fieldType(field.type),
    options: (Array.isArray(field.options) ? field.options : []).map((entry) => {
      const option = asRecord(entry)
      return {
        id: asString(option.id),
        name: asString(option.name),
        color: asString(option.color) || '#8b8d98',
        archived: option.archived === true,
        category: STATUS_CATEGORIES.includes(option.category as StatusCategory) ? (option.category as StatusCategory) : null,
      }
    }),
    target: field.target === 'page' || field.target === 'task' || field.target === 'record' ? field.target : null,
    collection: typeof field.collection === 'string' ? field.collection : null,
    position: typeof field.position === 'number' ? field.position : 0,
    archived: field.archived === true,
    version: typeof field.version === 'number' ? field.version : 1,
  }
}

/** A record's values: only the shapes a field can hold. */
export function normalizeValues(value: unknown): FieldValues {
  const values: FieldValues = {}
  for (const [key, raw] of Object.entries(asRecord(value))) {
    if (typeof raw === 'string' || typeof raw === 'boolean') values[key] = raw
    else if (typeof raw === 'number' && Number.isFinite(raw)) values[key] = raw
    else if (Array.isArray(raw)) values[key] = raw.filter((item): item is string => typeof item === 'string')
  }
  return values
}

export function normalizeReferences(value: unknown): FieldReference[] {
  return (Array.isArray(value) ? value : []).flatMap((entry) => {
    const reference = asRecord(entry)
    const kind = reference.kind
    if (kind !== 'person' && kind !== 'task' && kind !== 'page' && kind !== 'record') return []
    return [
      {
        id: asString(reference.id),
        kind,
        label: asString(reference.label),
        key: typeof reference.key === 'string' ? reference.key : null,
      },
    ]
  })
}

// ---------------------------------------------------------------------------
// Showing values

export function isEmpty(value: FieldValue | undefined | null) {
  return value === undefined || value === null || value === '' || value === false || (Array.isArray(value) && value.length === 0)
}

/** The options a value names, in the field's order. */
export function chosenOptions(field: FieldDefinition, value: FieldValue | undefined): FieldOption[] {
  const ids = Array.isArray(value) ? value : typeof value === 'string' ? [value] : []
  return field.options.filter((option) => ids.includes(option.id))
}

/**
 * A number as the app shows it. Six decimals would round a small value
 * such as 1e-7 to 0, so below 1 it keeps six significant digits, and below
 * one millionth it reads in scientific notation ("1e-7") rather than a
 * long run of zeros.
 */
function formatNumber(value: number) {
  if (Number.isInteger(value)) return value.toLocaleString()
  const size = Math.abs(value)
  if (size < 1e-6) return Number(value.toPrecision(6)).toExponential()
  if (size < 1) return value.toLocaleString(undefined, { maximumSignificantDigits: 6 })
  return value.toLocaleString(undefined, { maximumFractionDigits: 6 })
}

/** A time (as the server writes one) as "Nov 4, 14:05", with the year when it isn't this year. */
export function formatFieldTime(value: string) {
  const time = new Date(value.replace(' ', 'T').replace(/\+00$/, 'Z'))
  if (Number.isNaN(time.getTime())) return value
  const sameYear = time.getFullYear() === new Date().getFullYear()
  return time.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', ...(sameYear ? {} : { year: 'numeric' }) })
}

/** A date as "Nov 4", with the year when it isn't this year. */
export function formatFieldDate(value: string) {
  const [year, month, day] = value.split('-').map(Number)
  if (!year || !month || !day) return value
  const date = new Date(year, month - 1, day)
  const sameYear = year === new Date().getFullYear()
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) })
}

/** A value as one line of plain text, or '' when it has none. */
export function displayText(
  field: FieldDefinition,
  value: FieldValue | undefined,
  references: readonly FieldReference[] = [],
): string {
  if (isEmpty(value)) return ''
  switch (field.type) {
    case 'number':
      return typeof value === 'number' ? formatNumber(value) : String(value)
    case 'date':
      return typeof value === 'string' ? formatFieldDate(value) : ''
    case 'auto_id':
      return typeof value === 'number' ? `#${value}` : String(value)
    case 'created_time':
    case 'edited_time':
      return typeof value === 'string' ? formatFieldTime(value) : ''
    case 'single_choice':
    case 'multiple_choice':
    case 'status':
      return chosenOptions(field, value)
        .map((option) => option.name)
        .join(', ')
    case 'checkbox':
      return value === true ? 'Yes' : ''
    case 'person':
    case 'created_by':
    case 'edited_by':
    case 'link': {
      const reference = references.find((entry) => entry.id === value)
      if (!reference) return field.type === 'link' ? 'Not found' : 'Someone'
      return reference.kind === 'task' && reference.key ? `${reference.key} ${reference.label}` : reference.label
    }
    default:
      return String(value)
  }
}

// ---------------------------------------------------------------------------
// Filters, sorts and groups

const OP_LABELS: Record<FieldOp, string> = {
  is: 'is',
  isNot: 'is not',
  empty: 'is empty',
  notEmpty: 'is set',
  contains: 'contains',
  gt: 'is after / above',
  gte: 'is on or after / at least',
  lt: 'is before / below',
  lte: 'is on or before / at most',
  within: 'is within',
  categoryIs: 'is in category',
  categoryIsNot: 'is not in category',
}

export function opLabel(op: FieldOp, type?: FieldType) {
  type = type ? behavesAs(type) : type
  if (type === 'date') return { gt: 'is after', gte: 'is on or after', lt: 'is before', lte: 'is on or before' }[op as 'gt'] ?? OP_LABELS[op]
  if (type === 'number') return { gt: 'is above', gte: 'is at least', lt: 'is below', lte: 'is at most' }[op as 'gt'] ?? OP_LABELS[op]
  if (type === 'checkbox' && op === 'is') return 'is checked'
  if (type === 'checkbox' && op === 'empty') return 'is not checked'
  return OP_LABELS[op]
}

/** The filter operations a type offers, as the server accepts them (the registry's). */
export function opsFor(type: FieldType): FieldOp[] {
  return [...typeInfo(type).ops]
}

export function opTakesValues(op: FieldOp) {
  return op !== 'empty' && op !== 'notEmpty'
}

/** Whether a task list groups by a field of the type: one group per task. */
export function canGroup(type: FieldType) {
  return typeInfo(type).grouping === 'one'
}

/**
 * Whether a collection's views group by a field of the type: also several
 * choices (a record in each of its groups) and dates (by day, week or month).
 */
export function canGroupRecords(type: FieldType) {
  return typeInfo(type).grouping !== null
}

/** How a view grouped by a date puts records together. */
export type DateBucket = 'day' | 'week' | 'month'
export const DATE_BUCKETS: Array<{ bucket: DateBucket; label: string }> = [
  { bucket: 'day', label: 'Day' },
  { bucket: 'week', label: 'Week' },
  { bucket: 'month', label: 'Month' },
]

/** Field filters as a list request carries them, or [] for anything else. */
export function parseClauses(param: string | undefined | null): FieldClause[] {
  if (!param) return []
  try {
    const parsed: unknown = JSON.parse(param)
    if (!Array.isArray(parsed)) return []
    return parsed.flatMap((entry) => {
      const clause = asRecord(entry)
      const op = clause.op as FieldOp
      if (typeof clause.field !== 'string' || !(op in OP_LABELS)) return []
      const values = Array.isArray(clause.values) ? clause.values.filter((value): value is string => typeof value === 'string') : []
      return [opTakesValues(op) ? { field: clause.field, op, values } : { field: clause.field, op }]
    })
  } catch {
    return []
  }
}

/**
 * The longest a list's field filters may be as their URL writes them: what
 * a saved view keeps of one setting (the server's check_view_query), so
 * any filter the list applies can be read back and saved.
 */
export const MAX_FIELD_FILTERS_CHARACTERS = 1000
/** The server's limits on one list request's field filters (fields.rs: MAX_CLAUSES, MAX_CLAUSE_VALUES, MAX_CLAUSE_VALUE_CHARACTERS). */
export const MAX_FIELD_CLAUSES = 8
export const MAX_CLAUSE_VALUES = 20
export const MAX_CLAUSE_VALUE_CHARACTERS = 200

/**
 * Whether field filters are ones the list can keep and the server takes:
 * at most MAX_FIELD_CLAUSES of them, each with at most MAX_CLAUSE_VALUES
 * values of at most MAX_CLAUSE_VALUE_CHARACTERS characters, and
 * MAX_FIELD_FILTERS_CHARACTERS as the URL writes them.
 */
export function clausesWithinLimits(clauses: readonly FieldClause[]): boolean {
  return (
    clauses.length <= MAX_FIELD_CLAUSES &&
    clauses.every(
      (clause) =>
        (clause.values?.length ?? 0) <= MAX_CLAUSE_VALUES &&
        (clause.values ?? []).every((value) => [...value].length <= MAX_CLAUSE_VALUE_CHARACTERS),
    ) &&
    (clausesParam(clauses)?.length ?? 0) <= MAX_FIELD_FILTERS_CHARACTERS
  )
}

export function clausesParam(clauses: readonly FieldClause[]): string | undefined {
  const kept = clauses.filter((clause) => !opTakesValues(clause.op) || (clause.values?.length ?? 0) > 0)
  return kept.length ? JSON.stringify(kept) : undefined
}

/** How a list column, sort or group names a field: `field:<id>`. */
export const FIELD_KEY_PREFIX = 'field:'

export function fieldKey(fieldId: string) {
  return `${FIELD_KEY_PREFIX}${fieldId}`
}

/** The field ID in a `field:<id>` column, sort or group, or null. A sort may end in `:desc`. */
export function fieldIdOf(key: string | undefined | null): string | null {
  if (!key?.startsWith(FIELD_KEY_PREFIX)) return null
  return key.slice(FIELD_KEY_PREFIX.length).replace(/:(desc|asc)$/, '') || null
}

/** A clause in words: "Risk is High, Low". */
export function clauseText(clause: FieldClause, field: FieldDefinition | undefined, references: readonly FieldReference[] = []) {
  if (!field) return 'A removed field'
  const values = (clause.values ?? []).map((value) => {
    if (clause.op === 'categoryIs' || clause.op === 'categoryIsNot') return CATEGORY_LABELS[value as StatusCategory] ?? value
    if (field.type === 'single_choice' || field.type === 'multiple_choice' || field.type === 'status') {
      return field.options.find((option) => option.id === value)?.name ?? 'an old option'
    }
    const like = behavesAs(field.type)
    if (like === 'person' || like === 'link') return references.find((entry) => entry.id === value)?.label ?? 'someone'
    if (like === 'date') return formatFieldDate(value)
    if (field.type === 'checkbox') return ''
    return value
  })
  const words = [field.name, opLabel(clause.op, field.type), values.filter(Boolean).join(', ')].filter(Boolean)
  return words.join(' ')
}

// ---------------------------------------------------------------------------
// Fields module manifests declare

/**
 * A field a module manifest declares (`FieldDecl`), as a field definition
 * (T1.13a): manifests declare a subset of a field (an id, a label, a type and
 * option names), so the registry draws and edits their values the way it does
 * a project's own fields. An option's id is its name, as manifests' values hold it.
 */
export function definitionOfDecl(decl: FieldDecl, position = 1): FieldDefinition {
  return {
    id: decl.id,
    name: decl.label,
    description: '',
    type: decl.type,
    options: (decl.options ?? []).map((name) => ({ id: name, name, color: '#8b8d98', archived: false, category: null })),
    target: null,
    position,
    archived: false,
    version: 1,
  }
}
