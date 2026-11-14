import { canGroup, isEmpty, type FieldDefinition, type FieldReference, type FieldType, type FieldValue, type FieldValues } from '../fields/fieldModel'
import { formatDateLabel } from '../ui/dateModel'
import type { CollectionRecord, GroupCount, SortRule, ViewConfig } from './api'

/** How a record is named where its title is empty. */
export const recordName = (record: Pick<CollectionRecord, 'title' | 'number'>) => record.title || `#${record.number}`

/** The title column's width when a view doesn't set one, in pixels. */
export const TITLE_WIDTH = 280

const WIDTHS: Record<FieldType, number> = {
  text: 240,
  long_text: 320,
  number: 120,
  date: 140,
  single_choice: 160,
  multiple_choice: 220,
  status: 160,
  person: 180,
  people: 220,
  labels: 220,
  url: 200,
  checkbox: 104,
  link: 220,
}

export type ShownColumn = { field: FieldDefinition; width: number }

/**
 * The fields a view shows, in its order: the ones it lists first, then any it
 * doesn't (a field added since) in the collection's order. Hidden and
 * archived fields stay out.
 */
export function viewColumns(fields: readonly FieldDefinition[], config: ViewConfig): ShownColumn[] {
  return columnOrder(fields, config).flatMap(({ field, hidden, width }) => (hidden ? [] : [{ field, width }]))
}

/** Every active field in the view's column order, with whether it's hidden there, for the Fields menu. */
export function columnOrder(fields: readonly FieldDefinition[], config: ViewConfig) {
  const active = fields.filter((field) => !field.archived)
  const listed = config.columns.flatMap((column) => {
    const field = active.find((entry) => entry.id === column.field)
    return field ? [{ field, hidden: column.hidden === true, width: column.width ?? WIDTHS[field.type] }] : []
  })
  const rest = active
    .filter((field) => !config.columns.some((column) => column.field === field.id))
    .sort((left, right) => left.position - right.position)
    .map((field) => ({ field, hidden: false, width: WIDTHS[field.type] }))
  return [...listed, ...rest]
}

/** The view with a column shown or hidden. Its columns list every active field from then on, so the order stays put. */
export function withColumnHidden(fields: readonly FieldDefinition[], config: ViewConfig, fieldId: string, hidden: boolean): ViewConfig {
  return {
    ...config,
    columns: columnOrder(fields, config).map(({ field, hidden: wasHidden, width }) => ({
      field: field.id,
      ...((field.id === fieldId ? hidden : wasHidden) ? { hidden: true } : {}),
      ...(width !== WIDTHS[field.type] ? { width } : {}),
    })),
  }
}

/** Whether the person changed what a view finds or shows. */
export function sameConfig(left: ViewConfig, right: ViewConfig) {
  const comparable = (config: ViewConfig) =>
    JSON.stringify([config.filters.map((clause) => [clause.field, clause.op, clause.values ?? []]), config.sort, config.groupBy, config.columns, config.titleWidth, config.dateField])
  return comparable(left) === comparable(right)
}

const BUILT_IN_SORTS = new Set(['title', 'number', 'updated'])

/**
 * A view as it reads now, as the server reads a saved one: filters, sorts and
 * a grouping on a field that's archived (or gone) are left out until it comes
 * back. The server refuses them in a query or a save, so a view changed
 * before its field was archived still loads and saves.
 */
export function activeConfig(config: ViewConfig, fields: readonly FieldDefinition[]): ViewConfig {
  const active = new Set(fields.filter((field) => !field.archived).map((field) => field.id))
  const filters = config.filters.filter((clause) => active.has(clause.field))
  const sort = config.sort.filter((rule) => BUILT_IN_SORTS.has(rule.field) || active.has(rule.field))
  const groupBy = config.groupBy !== null && active.has(config.groupBy) ? config.groupBy : null
  const dateField = config.dateField !== null && active.has(config.dateField) ? config.dateField : null
  if (filters.length === config.filters.length && sort.length === config.sort.length && groupBy === config.groupBy && dateField === config.dateField) return config
  return { ...config, filters, sort, groupBy, dateField }
}

/** What a sort key sorts by, in words: the title column, the record's number or a field. */
export function sortName(rule: SortRule, fields: readonly FieldDefinition[], titleName: string) {
  if (rule.field === 'title') return titleName
  if (rule.field === 'number') return 'Number'
  if (rule.field === 'updated') return 'Last changed'
  return fields.find((field) => field.id === rule.field)?.name ?? 'A removed field'
}

/** "Severity, Z to A": a sort key's direction as people read it for its type. */
export function sortText(rule: SortRule, fields: readonly FieldDefinition[], titleName: string) {
  const field = fields.find((entry) => entry.id === rule.field)
  const type = rule.field === 'number' ? 'number' : rule.field === 'updated' ? 'date' : (field?.type ?? 'text')
  const words =
    type === 'number'
      ? rule.direction === 'asc'
        ? '1 to 9'
        : '9 to 1'
      : type === 'date'
        ? rule.direction === 'asc'
          ? 'oldest first'
          : 'newest first'
        : type === 'checkbox'
          ? rule.direction === 'asc'
            ? 'checked first'
            : 'unchecked first'
          : type === 'single_choice' || type === 'multiple_choice'
            ? rule.direction === 'asc'
              ? 'in option order'
              : 'in reverse option order'
            : rule.direction === 'asc'
              ? 'A to Z'
              : 'Z to A'
  return `${sortName(rule, fields, titleName)}, ${words}`
}

/** Fields a view can group by: one choice, a person or a checkbox. */
export function groupableFields(fields: readonly FieldDefinition[]) {
  return fields.filter((field) => !field.archived && canGroup(field.type))
}

export type TableItem =
  | { kind: 'group'; key: string; label: string; count: number; color: string | null; collapsed: boolean }
  | { kind: 'record'; record: CollectionRecord }
  | { kind: 'loading' }

/**
 * The table's rows in the server's order: a header before each group when
 * grouped (with every group's count, loaded or not), the records of the open
 * groups, and a row that loads the next page when there is one.
 */
export function tableItems(
  records: readonly CollectionRecord[],
  groups: readonly GroupCount[],
  grouped: boolean,
  collapsed: ReadonlySet<string>,
  hasMore: boolean,
): TableItem[] {
  const items: TableItem[] = []
  let current: string | null = null
  for (const record of records) {
    if (grouped) {
      const key = record.group?.key ?? ''
      if (key !== current) {
        current = key
        const count = groups.find((group) => group.key === key)
        items.push({
          kind: 'group',
          key,
          label: record.group?.label || count?.label || '',
          count: count?.count ?? 0,
          color: count?.color ?? null,
          collapsed: collapsed.has(key),
        })
      }
      if (collapsed.has(key)) continue
    }
    items.push({ kind: 'record', record })
  }
  if (hasMore) items.push({ kind: 'loading' })
  return items
}

/** What a person, task or page just picked in a field's editor is, so the cell names it before the save lands. */
export const referenceKind = (field: FieldDefinition): FieldReference['kind'] =>
  field.type === 'person' ? 'person' : field.target === 'page' || field.target === 'record' ? field.target : 'task'

/** A record's values with a change applied: null clears a value. */
export function withValues(values: FieldValues, change: Record<string, FieldValue | null> | undefined): FieldValues {
  if (!change) return values
  const next = { ...values }
  for (const [field, value] of Object.entries(change)) {
    if (value === null || isEmpty(value)) delete next[field]
    else next[field] = value
  }
  return next
}

/** A value in a record's history, as it read then. The field's type, when known, reads a checkbox as Yes or No and a date as a date. */
export function historyValue(value: unknown, type?: FieldType): string {
  if (type === 'checkbox') return value === true ? 'Yes' : 'No'
  if (value === null || value === undefined || value === '' || value === false) return 'empty'
  if (value === true) return 'Yes'
  if (type === 'date' && typeof value === 'string') return formatDateLabel(value) || value
  if (Array.isArray(value)) return value.length ? value.map((item) => historyValue(item)).join(', ') : 'empty'
  if (typeof value === 'object') {
    const named = value as { label?: unknown; name?: unknown }
    if (typeof named.label === 'string') return named.label
    if (typeof named.name === 'string') return named.name
    return 'a value'
  }
  return String(value)
}

/** The first few values worth showing on a record's card on a phone, in column order. */
export function cardValues(record: CollectionRecord, columns: readonly ShownColumn[], limit = 4) {
  return columns.filter(({ field }) => !isEmpty(record.values[field.id])).slice(0, limit)
}

// ---------------------------------------------------------------------------
// Boards (AWD-110): a column for each value of the field the view groups by.

/** "none" is the server's key for records with no value (and a checkbox left clear). */
export const NO_GROUP = 'none'

export type BoardColumn = { key: string; label: string; color: string | null; count: number }

/**
 * A board's columns: a choice field's options in order (archived ones only
 * while records still have them), a checkbox's two states, or the people the
 * server found; then "No value" for the rest. Counts come from the server's
 * groups, so an empty column still shows, ready to drop a card on.
 */
export function boardColumns(field: FieldDefinition, groups: readonly GroupCount[]): BoardColumn[] {
  const count = (key: string) => groups.find((group) => group.key === key)?.count ?? 0
  const none: BoardColumn = {
    key: NO_GROUP,
    label: field.type === 'person' ? 'No one' : field.type === 'checkbox' ? 'Not checked' : 'No value',
    color: null,
    count: count(NO_GROUP),
  }
  if (field.type === 'checkbox') return [none, { key: 'true', label: 'Checked', color: null, count: count('true') }]
  const columns: BoardColumn[] =
    field.type === 'single_choice'
      ? field.options
          .filter((option) => !option.archived || count(option.id) > 0)
          .map((option) => ({ key: option.id, label: option.name, color: option.color, count: count(option.id) }))
      : groups.filter((group) => group.key !== NO_GROUP).map((group) => ({ key: group.key, label: group.label, color: group.color, count: group.count }))
  return [...columns, none]
}

/** The column a record belongs in by its own value, as it reads now (a move shows before it saves). */
export function boardKey(record: Pick<CollectionRecord, 'values'>, field: FieldDefinition, columns: readonly BoardColumn[]): string {
  const value = record.values[field.id]
  if (field.type === 'checkbox') return value === true ? 'true' : NO_GROUP
  if (typeof value !== 'string' || !value) return NO_GROUP
  return columns.some((column) => column.key === value) ? value : NO_GROUP
}

/** The value a card takes when it's moved to a column: null clears it. */
export function boardValue(field: FieldDefinition, key: string): FieldValue | null {
  if (key === NO_GROUP) return field.type === 'checkbox' ? false : null
  return field.type === 'checkbox' ? true : key
}

// ---------------------------------------------------------------------------
// Calendars (AWD-110): a month, each record on its date field's day.

/** The month a calendar shows from a `YYYY-MM` in the address, or this month. */
export function calendarMonth(value: string | null, today = new Date()): Date {
  const match = value ? /^(\d{4})-(\d{2})$/.exec(value) : null
  if (match) {
    const month = Number(match[2])
    if (month >= 1 && month <= 12) return new Date(Number(match[1]), month - 1, 1)
  }
  return new Date(today.getFullYear(), today.getMonth(), 1)
}

export const monthKey = (month: Date) => `${month.getFullYear()}-${String(month.getMonth() + 1).padStart(2, '0')}`

/** A calendar's records by day (YYYY-MM-DD), in the order they loaded. */
export function recordsByDay(records: readonly CollectionRecord[], dateField: string): Map<string, CollectionRecord[]> {
  const days = new Map<string, CollectionRecord[]>()
  for (const record of records) {
    const value = record.values[dateField]
    if (typeof value !== 'string' || !value) continue
    const day = value.slice(0, 10)
    days.set(day, [...(days.get(day) ?? []), record])
  }
  return days
}

/** Fields a calendar can show records by. */
export function dateFields(fields: readonly FieldDefinition[]) {
  return fields.filter((field) => !field.archived && field.type === 'date').sort((left, right) => left.position - right.position)
}
