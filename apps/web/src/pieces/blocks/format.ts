import type { FieldType } from '../../fields/fieldModel'
import type { FormField, FormValues } from '../formFields'

// The panel block format, `aw-panel/1` (M1's plan, "Panels: the block
// format"): a small document of blocks a connected or built module sends,
// which the app draws with its own pieces (W1.25). Text is never HTML.
// M1.14's `PanelHost` fetches, signs and times out; the server's validator
// refuses what's over the limits or asks for a secret. Here the blocks are
// only read and drawn.

export const PANEL_FORMAT = 'aw-panel/1'

export type PanelTone = 'muted' | 'agent' | 'warning' | 'success' | 'danger'

/** What a button, a row or a form asks the module to do. `values` holds a form's values or the row it came from. */
export type PanelAction = { id: string; values?: Record<string, unknown> }

export type PanelActionRef = { id: string }

export type TextBlock = { type: 'text'; text: string; style?: 'body' | 'heading' | 'muted' | 'callout'; tone?: PanelTone }

export type FieldKind = 'text' | 'time' | 'date' | 'number' | 'badge' | 'link' | 'thing' | 'person'
export type FieldsBlock = { type: 'fields'; items: { label: string; value: unknown; kind?: FieldKind }[] }

/** The subset of JSON Schema 2020-12 a panel's form may use. */
export type PanelSchemaProperty = {
  type?: 'string' | 'number' | 'integer' | 'boolean' | 'array'
  title?: string
  description?: string
  enum?: string[]
  items?: { enum?: string[] }
  default?: unknown
  minimum?: number
  maxLength?: number
  pattern?: string
}
export type PanelSchema = { type?: 'object'; properties: Record<string, PanelSchemaProperty>; required?: string[] }
export type FormWidget = 'text' | 'textarea' | 'select' | 'radio' | 'checkbox' | 'multiselect'
export type FormBlock = {
  type: 'form'
  id: string
  schema: PanelSchema
  ui?: { order?: string[]; widgets?: Record<string, FormWidget> }
  values?: Record<string, unknown>
  submit: { label: string; action: PanelActionRef }
  cancel?: { label: string; action: PanelActionRef }
}

export type PanelStatus = string | { text: string; tone?: PanelTone }
export type ListBlock = {
  type: 'list'
  items: { id: string; title: string; subtitle?: string; status?: PanelStatus; meta?: string; action?: PanelActionRef }[]
  empty?: string
  more?: { action: PanelActionRef; label?: string }
}

export type TableBlock = {
  type: 'table'
  columns: { id: string; label: string; kind?: FieldKind }[]
  rows: { id: string; cells: Record<string, unknown> }[]
  rowAction?: PanelActionRef
}

export type ButtonsBlock = {
  type: 'buttons'
  items: { label: string; action: PanelActionRef; style?: 'primary' | 'secondary' | 'danger'; confirm?: { title: string; body?: string } }[]
}

export type StatusBlock = { type: 'status'; state: 'ok' | 'working' | 'waiting' | 'error' | 'unknown'; label: string; detail?: string; progress?: number }

export type ChartBlock = {
  type: 'chart'
  kind: 'bar' | 'line' | 'number'
  x?: string[]
  series?: { name: string; values: number[] }[]
  value?: number
  unit?: string
}

export type SectionBlock = { type: 'section'; title?: string; blocks: PanelBlock[] }

export type KnownBlock = TextBlock | FieldsBlock | FormBlock | ListBlock | TableBlock | ButtonsBlock | StatusBlock | ChartBlock | SectionBlock
/** A block this app doesn't know, from a newer format: drawn as a note, never a failure. */
export type UnknownBlock = { type: string }
export type PanelBlock = KnownBlock | UnknownBlock

export type PanelDocument = {
  format: typeof PANEL_FORMAT
  title?: string
  blocks: PanelBlock[]
  state?: string
  refresh?: { afterSeconds: number }
}

const KNOWN = new Set(['text', 'fields', 'form', 'list', 'table', 'buttons', 'status', 'chart', 'section'])

export function isKnownBlock(block: PanelBlock): block is KnownBlock {
  return KNOWN.has(block.type)
}

// ---------------------------------------------------------------------------
// Text: plain words with **bold**, *italic*, `code` and links to https: or
// the app's own paths. Anything else stays as written.

export type TextRun =
  | { kind: 'text'; text: string }
  | { kind: 'bold' | 'italic' | 'code'; text: string }
  | { kind: 'link'; text: string; href: string }

const INLINE = /\*\*([^*]+)\*\*|\*([^*]+)\*|`([^`]+)`|\[([^\]]+)\]\(([^)\s]+)\)/g

/** Whether a panel may link here: an https address, or a path in this app (never `//host`). */
export function safeHref(href: string): boolean {
  return /^https:\/\/[^\s]+$/i.test(href) || (href.startsWith('/') && !href.startsWith('//'))
}

export function textRuns(text: string): TextRun[] {
  const runs: TextRun[] = []
  let last = 0
  for (const match of text.matchAll(INLINE)) {
    const at = match.index ?? 0
    if (at > last) runs.push({ kind: 'text', text: text.slice(last, at) })
    const [whole, bold, italic, code, label, href] = match
    if (bold !== undefined) runs.push({ kind: 'bold', text: bold })
    else if (italic !== undefined) runs.push({ kind: 'italic', text: italic })
    else if (code !== undefined) runs.push({ kind: 'code', text: code })
    else if (label !== undefined && href !== undefined && safeHref(href)) runs.push({ kind: 'link', text: label, href })
    else runs.push({ kind: 'text', text: whole })
    last = at + whole.length
  }
  if (last < text.length) runs.push({ kind: 'text', text: text.slice(last) })
  return runs
}

// ---------------------------------------------------------------------------
// Forms: the schema as the form piece's fields, in the order the panel asks.

function fieldType(property: PanelSchemaProperty, widget: FormWidget | undefined): FieldType {
  if (property.type === 'boolean' || widget === 'checkbox') return 'checkbox'
  if (property.type === 'array' || widget === 'multiselect') return 'multiple_choice'
  if (property.enum || widget === 'select' || widget === 'radio') return 'single_choice'
  if (property.type === 'number' || property.type === 'integer') return 'number'
  if (widget === 'textarea') return 'long_text'
  return 'text'
}

export function formFields(form: FormBlock): FormField[] {
  const properties = form.schema.properties ?? {}
  const named = Object.keys(properties)
  const order = [...(form.ui?.order ?? []).filter((id) => id in properties), ...named.filter((id) => !(form.ui?.order ?? []).includes(id))]
  const required = new Set(form.schema.required ?? [])
  return order.map((id) => {
    const property = properties[id]
    const choices = property.enum ?? property.items?.enum
    return {
      id,
      label: property.title ?? id,
      type: fieldType(property, form.ui?.widgets?.[id]),
      required: required.has(id),
      hint: property.description,
      options: choices?.map((value) => ({ value, label: value })),
      maxLength: property.maxLength,
    }
  })
}

/** The values a form sends: numbers as numbers, the rest as picked. */
export function formValues(form: FormBlock, values: FormValues): Record<string, unknown> {
  const properties = form.schema.properties ?? {}
  return Object.fromEntries(
    Object.entries(values).map(([id, value]) => {
      const type = properties[id]?.type
      if ((type === 'number' || type === 'integer') && typeof value === 'string') {
        return [id, value.trim() === '' ? null : Number(value)]
      }
      return [id, value]
    }),
  )
}
