import { canGroup, isEmpty, type FieldDefinition, type FieldReference, type FieldType, type FieldValue, type FieldValues } from '../fields/fieldModel'
import { formatDateLabel } from '../ui/dateModel'
import type { CollectionRecord, GroupCount, SortRule, ViewConfig } from './api'

/** How a record is named where its title is empty. */
export const recordName = (record: Pick<CollectionRecord, 'title' | 'number'>) => record.title || `#${record.number}`

/** The title column's width when a view doesn't set one, in pixels. */
export const TITLE_WIDTH = 280

const WIDTHS: Record<FieldType, number> = {
  text: 240,
  number: 120,
  date: 140,
  single_choice: 160,
  multiple_choice: 220,
  person: 180,
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
    JSON.stringify([config.filters.map((clause) => [clause.field, clause.op, clause.values ?? []]), config.sort, config.groupBy, config.columns, config.titleWidth])
  return comparable(left) === comparable(right)
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
  field.type === 'person' ? 'person' : field.target === 'page' ? 'page' : 'task'

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
