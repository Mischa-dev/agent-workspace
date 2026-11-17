/**
 * The field types (T1.13a): the one list, and what the app knows about each
 * type that isn't drawing it: its name and hint, whether fields can be made
 * with it, what it filters, sorts and groups like, and how wide its column
 * starts. `registry.tsx` adds how a value of each type is drawn and edited.
 *
 * The server's `crate::fields::FieldType` is the same list;
 * `fieldTypes.generated.json` is written from it, and `registry.test.tsx`
 * fails when a type is in one and not the other, or has no entry here.
 */

/** Every field type, in the order people pick them. */
export const FIELD_TYPE_LIST = [
  'text',
  'long_text',
  'number',
  'date',
  'single_choice',
  'multiple_choice',
  'status',
  'person',
  'url',
  'email',
  'phone',
  'checkbox',
  'link',
  'formula',
  'button',
  'created_time',
  'edited_time',
  'created_by',
  'edited_by',
  'auto_id',
  'people',
  'labels',
] as const

export type FieldType = (typeof FIELD_TYPE_LIST)[number]

export type FieldOp = 'is' | 'isNot' | 'empty' | 'notEmpty' | 'contains' | 'gt' | 'gte' | 'lt' | 'lte' | 'within' | 'categoryIs' | 'categoryIsNot'

/** Whether a list can be grouped by a field of the type: `one` gives each record one group (task lists too); `records`, also several or by date (collections' views). */
export type Grouping = 'one' | 'records' | null

export type FieldTypeInfo = {
  label: string
  hint: string
  /** Fields can be made with it; the rest are named by module manifests only. */
  creatable: boolean
  /** A task's field can be of it (tasks have their own status and record columns). */
  tasks: boolean
  /** It shows the record's own columns: nobody fills it in. */
  system: boolean
  /** The type it filters, sorts and groups like. */
  behavesAs: Exclude<FieldType, 'created_time' | 'edited_time' | 'created_by' | 'edited_by' | 'auto_id' | 'formula' | 'button'>
  /** The filter operations the web offers for it, in the order the menu lists them. */
  ops: readonly FieldOp[]
  /** A sort's directions in words, or null when it doesn't sort. */
  sort: { asc: string; desc: string } | null
  grouping: Grouping
  /** A table column's width when a view doesn't set one, in pixels. */
  width: number
}

const TEXT_OPS = ['contains', 'is', 'isNot', 'empty', 'notEmpty'] as const
const RANGE_OPS = ['is', 'gt', 'gte', 'lt', 'lte', 'empty', 'notEmpty'] as const
const ID_OPS = ['is', 'isNot', 'empty', 'notEmpty'] as const
const A_TO_Z = { asc: 'A to Z', desc: 'Z to A' }
const OLDEST = { asc: 'oldest first', desc: 'newest first' }
const ONE_TO_NINE = { asc: '1 to 9', desc: '9 to 1' }
const OPTION_ORDER = { asc: 'in option order', desc: 'in reverse option order' }

const info = (entry: Omit<FieldTypeInfo, 'creatable' | 'tasks' | 'system'> & Partial<Pick<FieldTypeInfo, 'creatable' | 'tasks' | 'system'>>): FieldTypeInfo => ({
  creatable: true,
  tasks: true,
  system: false,
  ...entry,
})

const system = (entry: Omit<FieldTypeInfo, 'creatable' | 'tasks' | 'system'>): FieldTypeInfo => info({ ...entry, tasks: false, system: true })

/** What the app knows about each type. `Record` makes a missing type a type error. */
export const FIELD_TYPE_INFO: Record<FieldType, FieldTypeInfo> = {
  text: info({ label: 'Text', hint: 'A line or a few lines of text', behavesAs: 'text', ops: TEXT_OPS, sort: A_TO_Z, grouping: null, width: 240 }),
  long_text: info({ label: 'Long text', hint: 'Paragraphs, such as notes', behavesAs: 'long_text', ops: ['contains', 'empty', 'notEmpty'], sort: A_TO_Z, grouping: null, width: 320 }),
  number: info({ label: 'Number', hint: 'Such as 3 or 2.5', behavesAs: 'number', ops: RANGE_OPS, sort: ONE_TO_NINE, grouping: null, width: 120 }),
  date: info({ label: 'Date', hint: 'A day', behavesAs: 'date', ops: RANGE_OPS, sort: OLDEST, grouping: 'records', width: 140 }),
  single_choice: info({ label: 'Choice', hint: 'One of a set of options', behavesAs: 'single_choice', ops: ID_OPS, sort: OPTION_ORDER, grouping: 'one', width: 160 }),
  multiple_choice: info({ label: 'Choices', hint: 'Any of a set of options', behavesAs: 'multiple_choice', ops: ID_OPS, sort: OPTION_ORDER, grouping: 'records', width: 220 }),
  status: info({
    label: 'Status',
    hint: 'Where it is: options in categories such as started or done',
    tasks: false,
    behavesAs: 'status',
    ops: ['is', 'isNot', 'categoryIs', 'categoryIsNot', 'empty', 'notEmpty'],
    sort: { asc: 'in workflow order', desc: 'in reverse workflow order' },
    grouping: 'one',
    width: 160,
  }),
  person: info({ label: 'Person', hint: 'Someone in the project', behavesAs: 'person', ops: ID_OPS, sort: A_TO_Z, grouping: 'one', width: 180 }),
  url: info({ label: 'Link (URL)', hint: 'A web address', behavesAs: 'url', ops: TEXT_OPS, sort: A_TO_Z, grouping: null, width: 200 }),
  email: info({ label: 'Email', hint: 'An email address', behavesAs: 'email', ops: TEXT_OPS, sort: A_TO_Z, grouping: null, width: 220 }),
  phone: info({ label: 'Phone', hint: 'A phone number', behavesAs: 'phone', ops: TEXT_OPS, sort: A_TO_Z, grouping: null, width: 160 }),
  checkbox: info({ label: 'Checkbox', hint: 'Yes or no', behavesAs: 'checkbox', ops: ['is', 'empty'], sort: { asc: 'checked first', desc: 'unchecked first' }, grouping: 'one', width: 104 }),
  link: info({ label: 'Task, page or record', hint: 'A task, a page or a collection’s record in the project', behavesAs: 'link', ops: ['notEmpty', 'empty'], sort: A_TO_Z, grouping: null, width: 220 }),
  // What a formula filters, sorts and groups like comes from its result (`fieldModel`'s `likeOf`).
  formula: info({ label: 'Formula', hint: 'Worked out from the record’s other fields', tasks: false, behavesAs: 'text', ops: ['empty', 'notEmpty'], sort: A_TO_Z, grouping: null, width: 160 }),
  // A button has no value: nothing to filter, sort or group by.
  button: info({ label: 'Button', hint: 'Runs actions on the record when someone presses it', tasks: false, behavesAs: 'text', ops: [], sort: null, grouping: null, width: 150 }),
  created_time: system({ label: 'Created time', hint: 'When the record was made', behavesAs: 'date', ops: RANGE_OPS, sort: OLDEST, grouping: 'records', width: 170 }),
  edited_time: system({ label: 'Edited time', hint: 'When the record last changed', behavesAs: 'date', ops: RANGE_OPS, sort: OLDEST, grouping: 'records', width: 170 }),
  created_by: system({ label: 'Created by', hint: 'Who made the record', behavesAs: 'person', ops: ID_OPS, sort: A_TO_Z, grouping: 'one', width: 180 }),
  edited_by: system({ label: 'Edited by', hint: 'Who last changed the record', behavesAs: 'person', ops: ID_OPS, sort: A_TO_Z, grouping: 'one', width: 180 }),
  auto_id: system({ label: 'ID', hint: 'The record’s number', behavesAs: 'number', ops: RANGE_OPS, sort: ONE_TO_NINE, grouping: null, width: 90 }),
  // Named by module manifests only, for now.
  people: info({ label: 'People', hint: 'Several people', creatable: false, tasks: false, behavesAs: 'people', ops: ID_OPS, sort: A_TO_Z, grouping: null, width: 220 }),
  labels: info({ label: 'Labels', hint: 'The project’s labels', creatable: false, tasks: false, behavesAs: 'labels', ops: ID_OPS, sort: A_TO_Z, grouping: null, width: 220 }),
}

/** A type's entry; an unknown type (from a newer server) reads as text. */
export function typeInfo(type: FieldType | string): FieldTypeInfo {
  return FIELD_TYPE_INFO[type as FieldType] ?? FIELD_TYPE_INFO.text
}

export function isFieldType(value: unknown): value is FieldType {
  return typeof value === 'string' && (FIELD_TYPE_LIST as readonly string[]).includes(value)
}
