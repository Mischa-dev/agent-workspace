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

export type FieldType =
  | 'text'
  | 'number'
  | 'date'
  | 'single_choice'
  | 'multiple_choice'
  | 'person'
  | 'url'
  | 'checkbox'
  | 'link'

export type FieldOption = { id: string; name: string; color: string; archived: boolean }

export type FieldDefinition = {
  id: string
  name: string
  description: string
  type: FieldType
  /** Choice fields: every option in order, archived ones too. */
  options: FieldOption[]
  /** Link fields: what they link to. */
  target: 'task' | 'page' | null
  /** 1 is first. */
  position: number
  archived: boolean
  version: number
}

export type FieldValue = string | number | boolean | string[]
export type FieldValues = Record<string, FieldValue>

export type FieldReference = { id: string; kind: 'person' | 'task' | 'page'; label: string; key: string | null }

export type FieldOp = 'is' | 'isNot' | 'empty' | 'notEmpty' | 'contains' | 'gt' | 'gte' | 'lt' | 'lte'

/** One field filter, with stored values (option IDs, actor IDs, "true", numbers, dates). */
export type FieldClause = { field: string; op: FieldOp; values?: string[] }

export const FIELD_TYPES: Array<{ type: FieldType; label: string; hint: string }> = [
  { type: 'text', label: 'Text', hint: 'A line or a few lines of text' },
  { type: 'number', label: 'Number', hint: 'Such as 3 or 2.5' },
  { type: 'date', label: 'Date', hint: 'A day' },
  { type: 'single_choice', label: 'Choice', hint: 'One of a set of options' },
  { type: 'multiple_choice', label: 'Choices', hint: 'Any of a set of options' },
  { type: 'person', label: 'Person', hint: 'Someone in the project' },
  { type: 'url', label: 'Link (URL)', hint: 'A web address' },
  { type: 'checkbox', label: 'Checkbox', hint: 'Yes or no' },
  { type: 'link', label: 'Task or page', hint: 'Another task or a page in the project' },
]

export function typeLabel(type: FieldType) {
  return FIELD_TYPES.find((entry) => entry.type === type)?.label ?? type
}

// ---------------------------------------------------------------------------
// Reading the server's JSON

type Wire = Record<string, unknown>
const asRecord = (value: unknown): Wire =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Wire) : {}
const asString = (value: unknown) => (typeof value === 'string' ? value : '')

function fieldType(value: unknown): FieldType {
  return FIELD_TYPES.some((entry) => entry.type === value) ? (value as FieldType) : 'text'
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
      }
    }),
    target: field.target === 'page' ? 'page' : field.target === 'task' ? 'task' : null,
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
    if (kind !== 'person' && kind !== 'task' && kind !== 'page') return []
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

function formatNumber(value: number) {
  return Number.isInteger(value) ? value.toLocaleString() : value.toLocaleString(undefined, { maximumFractionDigits: 6 })
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
    case 'single_choice':
    case 'multiple_choice':
      return chosenOptions(field, value)
        .map((option) => option.name)
        .join(', ')
    case 'checkbox':
      return value === true ? 'Yes' : ''
    case 'person':
    case 'link': {
      const reference = references.find((entry) => entry.id === value)
      if (!reference) return field.type === 'person' ? 'Someone' : 'Not found'
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
}

export function opLabel(op: FieldOp, type?: FieldType) {
  if (type === 'date') return { gt: 'is after', gte: 'is on or after', lt: 'is before', lte: 'is on or before' }[op as 'gt'] ?? OP_LABELS[op]
  if (type === 'number') return { gt: 'is above', gte: 'is at least', lt: 'is below', lte: 'is at most' }[op as 'gt'] ?? OP_LABELS[op]
  if (type === 'checkbox' && op === 'is') return 'is checked'
  if (type === 'checkbox' && op === 'empty') return 'is not checked'
  return OP_LABELS[op]
}

/** The filter operations a type offers, as the server accepts them. */
export function opsFor(type: FieldType): FieldOp[] {
  switch (type) {
    case 'text':
    case 'url':
      return ['contains', 'is', 'isNot', 'empty', 'notEmpty']
    case 'number':
    case 'date':
      return ['is', 'gt', 'gte', 'lt', 'lte', 'empty', 'notEmpty']
    case 'checkbox':
      return ['is', 'empty']
    default:
      return ['is', 'isNot', 'empty', 'notEmpty']
  }
}

export function opTakesValues(op: FieldOp) {
  return op !== 'empty' && op !== 'notEmpty'
}

export function canGroup(type: FieldType) {
  return type === 'single_choice' || type === 'person' || type === 'checkbox'
}

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
    if (field.type === 'single_choice' || field.type === 'multiple_choice') {
      return field.options.find((option) => option.id === value)?.name ?? 'an old option'
    }
    if (field.type === 'person' || field.type === 'link') return references.find((entry) => entry.id === value)?.label ?? 'someone'
    if (field.type === 'date') return formatFieldDate(value)
    if (field.type === 'checkbox') return ''
    return value
  })
  const words = [field.name, opLabel(clause.op, field.type), values.filter(Boolean).join(', ')].filter(Boolean)
  return words.join(' ')
}
