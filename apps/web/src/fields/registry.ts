import type { ComponentType } from 'react'

import { CheckboxFieldEditor, ChoiceFieldEditor, DateFieldEditor, LinkFieldEditor, PersonFieldEditor, RelationFieldEditor, TypedFieldEditor, type EditorProps } from './editors'
import { FIELD_TYPE_INFO, FIELD_TYPE_LIST, type FieldType, type FieldTypeInfo } from './fieldTypes'
import { CheckRenderer, ChipsRenderer, EmailRenderer, FormulaRenderer, LinkRenderer, LongTextRenderer, PhoneRenderer, RelationRenderer, TextRenderer, UrlRenderer, type RendererProps } from './renderers'

/**
 * The field registry (T1.13a, moved from W1.16 by R17): one entry per field
 * type with everything the web app does with a field of it. What it is
 * (`fieldTypes.ts`: name, hint, filter operations, sort, grouping, column
 * width) and how a value reads (`Renderer`) and is changed (`Editor`, null
 * when nobody fills it in). Lists, tables, boards, panels and the task page
 * draw fields through it (`FieldValueView`, `FieldEditor`) rather than
 * switching on the type themselves; a new type registers here once.
 */
export type FieldTypeSpec = FieldTypeInfo & {
  type: FieldType
  Renderer: ComponentType<RendererProps>
  Editor: ComponentType<EditorProps> | null
}

export type { EditorProps, RendererProps }

type Drawing = Pick<FieldTypeSpec, 'Renderer' | 'Editor'>

const chips: Drawing = { Renderer: ChipsRenderer, Editor: ChoiceFieldEditor }
const typed: Drawing = { Renderer: TextRenderer, Editor: TypedFieldEditor }
const shown: Drawing = { Renderer: TextRenderer, Editor: null }

/** How each type is drawn. `Record` makes a type without an entry a type error. */
const DRAWINGS: Record<FieldType, Drawing> = {
  text: typed,
  long_text: { Renderer: LongTextRenderer, Editor: TypedFieldEditor },
  number: typed,
  date: { Renderer: TextRenderer, Editor: DateFieldEditor },
  single_choice: chips,
  multiple_choice: chips,
  status: chips,
  person: { Renderer: TextRenderer, Editor: PersonFieldEditor },
  url: { Renderer: UrlRenderer, Editor: TypedFieldEditor },
  email: { Renderer: EmailRenderer, Editor: TypedFieldEditor },
  phone: { Renderer: PhoneRenderer, Editor: TypedFieldEditor },
  checkbox: { Renderer: CheckRenderer, Editor: CheckboxFieldEditor },
  link: { Renderer: LinkRenderer, Editor: LinkFieldEditor },
  relation: { Renderer: RelationRenderer, Editor: RelationFieldEditor },
  // Worked out by the server; nobody fills it in.
  formula: { Renderer: FormulaRenderer, Editor: null },
  rollup: { Renderer: FormulaRenderer, Editor: null },
  // No value to show: collections draw it as a button that runs its steps.
  button: shown,
  created_time: shown,
  edited_time: shown,
  created_by: shown,
  edited_by: shown,
  auto_id: shown,
  // Named by module manifests only: shown as text, never stored.
  people: shown,
  labels: shown,
}

const registry = new Map<FieldType, FieldTypeSpec>(FIELD_TYPE_LIST.map((type) => [type, { type, ...FIELD_TYPE_INFO[type], ...DRAWINGS[type] }]))

/**
 * Adds or replaces a type's entry, such as a module's own way of drawing one.
 * Parts left out keep the current entry's.
 */
export function registerFieldType(type: FieldType, spec: Partial<Omit<FieldTypeSpec, 'type'>>) {
  const current = registry.get(type) ?? { type, ...FIELD_TYPE_INFO.text, ...shown }
  registry.set(type, { ...current, ...spec, type })
}

/** A type's entry. A type this build doesn't know (from a newer server) reads as text and can't be edited. */
export function fieldSpec(type: FieldType | string): FieldTypeSpec {
  return registry.get(type as FieldType) ?? { type: 'text', ...FIELD_TYPE_INFO.text, ...shown }
}

/** Every registered type, in the order people pick them. */
export function registeredTypes(): FieldType[] {
  return [...registry.keys()]
}
