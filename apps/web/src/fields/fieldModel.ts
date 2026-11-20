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
export const STATUS_CATEGORIES = ['triage', 'backlog', 'unstarted', 'started', 'review', 'verification', 'completed', 'canceled'] as const
export type StatusCategory = (typeof STATUS_CATEGORIES)[number]

export const CATEGORY_LABELS: Record<StatusCategory, string> = {
  triage: 'Triage',
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
  /** Formula fields: the formula as stored (fields as `{#<id>}`, the title as `{#title}`) and what it gives. */
  formula?: FormulaSettings | null
  /** Button fields: what it says and the operations pressing it runs (placeholders as stored). */
  button?: ButtonSettings | null
  /** Two-way relations: the paired field on the other collection. */
  inverse?: string | null
  /** Rollup fields: the relation they read, the related records' field, and how. */
  rollup?: RollupSettings | null
  /** Collection fields: what their values are held to. */
  rules?: FieldRules | null
  /** 1 is first. */
  position: number
  archived: boolean
  version: number
}

/** One operation a button runs: its name, its title, and its arguments with `{{...}}` placeholders (fields as `{{#<id>}}`). */
export type ButtonStep = { operation: string; title: string; arguments: Record<string, unknown> }
export type ButtonSettings = { label: string; steps: ButtonStep[] }

/**
 * What a field's values are held to (T1.19): every record has one, no two
 * share one, a new record's default ("me" and "today" worked out when it's
 * added), a number's range, a text pattern, and whether only people who
 * build the collection change it.
 */
export type FieldRules = {
  required: boolean
  unique: boolean
  default: unknown
  min: number | null
  max: number | null
  pattern: string | null
  buildersOnly: boolean
}

/** What a formula gives, which decides how it filters, sorts and groups. */
export type FormulaResult = 'number' | 'text' | 'boolean' | 'date' | 'list'
export type FormulaSettings = { source: string; result: FormulaResult }

/** How a rollup puts the related records' values together. */
export const ROLLUP_AGGREGATES = [
  { aggregate: 'count', label: 'Count records', needs: null },
  { aggregate: 'count_values', label: 'Count filled in', needs: 'any' },
  { aggregate: 'count_unique', label: 'Count different values', needs: 'any' },
  { aggregate: 'sum', label: 'Sum', needs: 'number' },
  { aggregate: 'average', label: 'Average', needs: 'number' },
  { aggregate: 'min', label: 'Lowest', needs: 'number' },
  { aggregate: 'max', label: 'Highest', needs: 'number' },
  { aggregate: 'earliest', label: 'Earliest date', needs: 'date' },
  { aggregate: 'latest', label: 'Latest date', needs: 'date' },
  { aggregate: 'percent_checked', label: 'Percent checked', needs: 'checkbox' },
  { aggregate: 'percent_empty', label: 'Percent empty', needs: 'any' },
  { aggregate: 'list', label: 'Show the values', needs: 'any' },
] as const
export type RollupAggregate = (typeof ROLLUP_AGGREGATES)[number]['aggregate']
export type RollupSettings = { relation: string; target: string | null; aggregate: RollupAggregate; result: FormulaResult }

/** A formula's error for one record ("Divide by zero"): shown in the cell, filtered as empty. */
export type FormulaError = { error: string }

export type FieldValue = string | number | boolean | string[] | FormulaError
export type FieldValues = Record<string, FieldValue>

export function isFormulaError(value: unknown): value is FormulaError {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value) && typeof (value as FormulaError).error === 'string'
}

export type LinkTarget = 'task' | 'page' | 'record'

export type FieldReference = { id: string; kind: 'person' | 'task' | 'page' | 'record' | 'file'; label: string; key: string | null }

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

/** Whether nobody fills in a field of the type: a system field, a formula the server works out, or a button. */
export function isReadOnlyType(type: FieldType) {
  return typeInfo(type).system || type === 'formula' || type === 'rollup' || type === 'button'
}

/** A field, or only its type. */
export type TypeOrField = FieldType | (Pick<FieldDefinition, 'type' | 'formula'> & Partial<Pick<FieldDefinition, 'rollup'>>)

const RESULT_LIKE: Record<FormulaResult, FieldType> = { number: 'number', date: 'date', boolean: 'checkbox', text: 'text', list: 'text' }

/** The type a field reads like: a formula's result's, any other field's own. */
export function likeOf(of: TypeOrField): FieldType {
  if (typeof of === 'string') return of
  if (of.type === 'formula' && of.formula) return RESULT_LIKE[of.formula.result]
  if (of.type === 'rollup' && of.rollup) return RESULT_LIKE[of.rollup.result]
  return of.type
}

/** A formula's or rollup's result type, or null for any other field. */
export function resultOf(field: FieldDefinition): FormulaResult | null {
  return field.formula?.result ?? field.rollup?.result ?? null
}

/** The type it filters, sorts and groups like (the server's `behaves_as`). */
export function behavesAs(of: TypeOrField): FieldType {
  return typeInfo(likeOf(of)).behavesAs
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
    formula: formulaOf(field.formula),
    button: buttonOf(field.button),
    inverse: typeof field.inverse === 'string' ? field.inverse : null,
    rollup: rollupOf(field.rollup),
    rules: rulesOf(field.rules),
    position: typeof field.position === 'number' ? field.position : 0,
    archived: field.archived === true,
    version: typeof field.version === 'number' ? field.version : 1,
  }
}

/** A stored formula as people write it: `{#<id>}` by today's field name, `{#title}` by the title's. */
export function formulaShown(source: string, fields: readonly FieldDefinition[], titleName: string) {
  return source.replace(/\{#([^}]*)\}/g, (whole, id: string) => {
    if (id === 'title') return `{${titleName}}`
    const field = fields.find((entry) => entry.id === id)
    return field && !/[{}]/.test(field.name) ? `{${field.name}}` : whole
  })
}

/** A button's stored arguments as people write them: `{{#<id>}}` by today's field name. */
export function buttonArgumentsShown(value: unknown, fields: readonly FieldDefinition[]): unknown {
  if (typeof value === 'string') {
    return value.replace(/\{\{\s*#([^}]*?)\s*\}\}/g, (whole, id: string) => {
      const field = fields.find((entry) => entry.id === id)
      return field && !field.name.includes('}}') ? `{{${field.name}}}` : whole
    })
  }
  if (Array.isArray(value)) return value.map((item) => buttonArgumentsShown(item, fields))
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, buttonArgumentsShown(item, fields)]))
  }
  return value
}

function buttonOf(value: unknown): ButtonSettings | null {
  const button = asRecord(value)
  if (!Array.isArray(button.steps)) return null
  return {
    label: asString(button.label),
    steps: button.steps.map((entry) => {
      const step = asRecord(entry)
      return { operation: asString(step.operation), title: asString(step.title) || asString(step.operation), arguments: asRecord(step.arguments) }
    }),
  }
}

function rulesOf(value: unknown): FieldRules | null {
  if (!value || typeof value !== 'object') return null
  const rules = asRecord(value)
  const number = (entry: unknown) => (typeof entry === 'number' && Number.isFinite(entry) ? entry : null)
  return {
    required: rules.required === true,
    unique: rules.unique === true,
    default: rules.default ?? null,
    min: number(rules.min),
    max: number(rules.max),
    pattern: typeof rules.pattern === 'string' && rules.pattern ? rules.pattern : null,
    buildersOnly: rules.buildersOnly === true,
  }
}

const FORMULA_RESULTS: readonly FormulaResult[] = ['number', 'text', 'boolean', 'date', 'list']

function rollupOf(value: unknown): RollupSettings | null {
  const rollup = asRecord(value)
  if (typeof rollup.relation !== 'string') return null
  const aggregate = ROLLUP_AGGREGATES.find((entry) => entry.aggregate === rollup.aggregate)?.aggregate ?? 'count'
  const result = FORMULA_RESULTS.includes(rollup.result as FormulaResult) ? (rollup.result as FormulaResult) : 'text'
  return { relation: rollup.relation, target: typeof rollup.target === 'string' ? rollup.target : null, aggregate, result }
}

function formulaOf(value: unknown): FormulaSettings | null {
  const formula = asRecord(value)
  if (typeof formula.source !== 'string') return null
  const result = FORMULA_RESULTS.includes(formula.result as FormulaResult) ? (formula.result as FormulaResult) : 'text'
  return { source: formula.source, result }
}

/** A record's values: only the shapes a field can hold (a formula's error too). */
export function normalizeValues(value: unknown): FieldValues {
  const values: FieldValues = {}
  for (const [key, raw] of Object.entries(asRecord(value))) {
    if (typeof raw === 'string' || typeof raw === 'boolean') values[key] = raw
    else if (typeof raw === 'number' && Number.isFinite(raw)) values[key] = raw
    else if (Array.isArray(raw)) values[key] = raw.filter((item): item is string => typeof item === 'string')
    else if (isFormulaError(raw)) values[key] = { error: raw.error }
  }
  return values
}

export function normalizeReferences(value: unknown): FieldReference[] {
  return (Array.isArray(value) ? value : []).flatMap((entry) => {
    const reference = asRecord(entry)
    const kind = reference.kind
    if (kind !== 'person' && kind !== 'task' && kind !== 'page' && kind !== 'record' && kind !== 'file') return []
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

/** A formula's value as it reads: by what it gives. */
function formulaText(field: FieldDefinition, value: FieldValue): string {
  if (isFormulaError(value)) return `Error: ${value.error}`
  if (Array.isArray(value)) return value.join(', ')
  switch (likeOf(field)) {
    case 'number':
      return typeof value === 'number' ? formatNumber(value) : String(value)
    case 'date':
      return typeof value === 'string' ? formatFieldDate(value) : String(value)
    case 'checkbox':
      return value === true ? 'Yes' : ''
    default:
      return String(value)
  }
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
  if (field.type === 'formula' || field.type === 'rollup') return formulaText(field, value!)
  if (isFormulaError(value)) return ''
  switch (field.type) {
    case 'relation': {
      const ids = Array.isArray(value) ? value : typeof value === 'string' ? [value] : []
      return ids.map((id) => references.find((entry) => entry.id === id)?.label ?? 'An archived record').join(', ')
    }
    case 'file': {
      // A removed file drops out.
      const ids = Array.isArray(value) ? value : []
      return ids.flatMap((id) => references.find((entry) => entry.id === id && entry.kind === 'file')?.label ?? []).join(', ')
    }
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

export function opLabel(op: FieldOp, of?: TypeOrField) {
  const type = of ? behavesAs(of) : undefined
  if (type === 'date') return { gt: 'is after', gte: 'is on or after', lt: 'is before', lte: 'is on or before' }[op as 'gt'] ?? OP_LABELS[op]
  if (type === 'number') return { gt: 'is above', gte: 'is at least', lt: 'is below', lte: 'is at most' }[op as 'gt'] ?? OP_LABELS[op]
  if (type === 'checkbox' && op === 'is') return 'is checked'
  if (type === 'checkbox' && op === 'empty') return 'is not checked'
  return OP_LABELS[op]
}

/** The filter operations a type offers, as the server accepts them (the registry's). */
export function opsFor(of: TypeOrField): FieldOp[] {
  return [...typeInfo(likeOf(of)).ops]
}

export function opTakesValues(op: FieldOp) {
  return op !== 'empty' && op !== 'notEmpty'
}

/** Whether a task list groups by a field of the type: one group per task. */
export function canGroup(of: TypeOrField) {
  return typeInfo(likeOf(of)).grouping === 'one'
}

/**
 * Whether a collection's views group by a field of the type: also several
 * choices (a record in each of its groups) and dates (by day, week or month).
 */
export function canGroupRecords(of: TypeOrField) {
  return typeInfo(likeOf(of)).grouping !== null
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
    const like = behavesAs(field)
    if (like === 'person' || like === 'link') return references.find((entry) => entry.id === value)?.label ?? 'someone'
    if (like === 'date') return formatFieldDate(value)
    if (field.type === 'checkbox') return ''
    return value
  })
  const words = [field.name, opLabel(clause.op, field), values.filter(Boolean).join(', ')].filter(Boolean)
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
