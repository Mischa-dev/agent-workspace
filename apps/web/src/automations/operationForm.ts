import automations from '../api/generated/automations.json'
import type { FormField, FormValue, FormValues } from '../pieces/formFields'
import type { OpenApiDocument, OpenApiSchema } from './api'

// W1.42: a rule's "Run an operation" action, built from the operation
// registry instead of a list kept by hand. The catalogue says which
// operations an automation may run; the v2 document gives each one's input,
// which becomes the form piece's fields. A module that registers an
// operation shows up here with no web change.

export type CatalogueEntry = { name: string; title: string; module: string; kind: string }

/**
 * The operations a rule may run, by module, then title. The generated slice
 * holds only these (marked for automations, not reads, not automations' own),
 * so the builder's chunk doesn't carry the whole catalogue.
 */
export const AUTOMATION_OPERATIONS: readonly CatalogueEntry[] = [...(automations as CatalogueEntry[])]
  .sort((a, b) => a.module.localeCompare(b.module) || a.title.localeCompare(b.title))

/** Filled in by the rule itself, never asked for: where it acts and how a retry is told apart. */
const SPACE_FIELDS = new Set(['projectId', 'spaceId'])
const HIDDEN_FIELDS = new Set(['workspaceId', 'clientRequestId', 'expectedVersion', 'preview'])
/** Text an operation takes that's usually more than a line. */
const LONG_TEXT = /^(text|body|content|description|prompt|message|markdown|note|instructions)$/i

const MAX_DEPTH = 8

/** Follows `$ref`s into the document's components, so the schema reads on its own. */
export function resolveSchema(document: OpenApiDocument, schema: OpenApiSchema | undefined, depth = 0): OpenApiSchema {
  if (!schema || depth > MAX_DEPTH) return {}
  if (schema.$ref) {
    const name = schema.$ref.split('/').pop() ?? ''
    return resolveSchema(document, document.components?.schemas?.[name], depth + 1)
  }
  const resolve = (inner: OpenApiSchema) => resolveSchema(document, inner, depth + 1)
  return {
    ...schema,
    ...(schema.properties
      ? { properties: Object.fromEntries(Object.entries(schema.properties).map(([key, value]) => [key, resolve(value)])) }
      : {}),
    ...(schema.items ? { items: resolve(schema.items) } : {}),
    ...(schema.anyOf ? { anyOf: schema.anyOf.map(resolve) } : {}),
    ...(schema.oneOf ? { oneOf: schema.oneOf.map(resolve) } : {}),
  }
}

/** An operation's input schema, or null when the document doesn't have it. */
export function operationInputSchema(document: OpenApiDocument, name: string): OpenApiSchema | null {
  const entry = Object.entries(document.paths).find(([path]) => path.endsWith(`/ops/${name}`))?.[1]
  const schema = entry?.post?.requestBody?.content?.['application/json']?.schema
  return schema ? resolveSchema(document, schema) : null
}

/** The schema without its "or nothing": `["string", "null"]` reads as a string, `anyOf [X, null]` as X. */
function plain(schema: OpenApiSchema): OpenApiSchema {
  if (schema.anyOf) {
    const real = schema.anyOf.filter((option) => option.type !== 'null')
    if (real.length === 1) return plain({ ...real[0], description: schema.description ?? real[0].description })
  }
  if (Array.isArray(schema.type)) {
    const real = schema.type.filter((type) => type !== 'null')
    return { ...schema, type: real.length === 1 ? real[0] : real }
  }
  return schema
}

type Shape = 'text' | 'long_text' | 'number' | 'checkbox' | 'choice' | 'choices' | 'lines' | 'json'

function shapeOf(id: string, schema: OpenApiSchema): Shape {
  const property = plain(schema)
  if (property.enum) return 'choice'
  if (property.type === 'boolean') return 'checkbox'
  if (property.type === 'integer' || property.type === 'number') return 'number'
  if (property.type === 'array') {
    const items = plain(property.items ?? {})
    if (items.enum) return 'choices'
    return items.type === 'string' ? 'lines' : 'json'
  }
  if (property.type === 'string') return LONG_TEXT.test(id) ? 'long_text' : 'text'
  return 'json'
}

/** "taskId" as people read it: "Task ID". */
export function fieldName(id: string): string {
  const words = id
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/_/g, ' ')
    .toLowerCase()
    .replace(/\bids\b/g, 'IDs')
    .replace(/\bid\b/g, 'ID')
    .replace(/\burl\b/g, 'URL')
  return words.charAt(0).toUpperCase() + words.slice(1)
}

const oneLine = (text: string | undefined) => text?.replace(/\s*\n\s*/g, ' ').trim() || undefined

/** The questions an operation's input asks, as the form piece draws them. */
export function operationFields(schema: OpenApiSchema): FormField[] {
  const required = new Set(schema.required ?? [])
  return Object.entries(schema.properties ?? {})
    .filter(([id]) => !SPACE_FIELDS.has(id) && !HIDDEN_FIELDS.has(id))
    .map(([id, property]) => {
      const shape = shapeOf(id, property)
      const flat = plain(property)
      const hint = oneLine(flat.description)
      const choices = (shape === 'choices' ? plain(flat.items ?? {}).enum : flat.enum)?.map(String)
      const extra = shape === 'lines' ? 'One per line.' : shape === 'json' ? 'As JSON.' : undefined
      return {
        id,
        label: fieldName(id),
        type:
          shape === 'choice'
            ? 'single_choice'
            : shape === 'choices'
              ? 'multiple_choice'
              : shape === 'lines' || shape === 'json'
                ? 'long_text'
                : shape,
        required: required.has(id) && shape !== 'checkbox',
        hint: [hint, extra].filter(Boolean).join(' ') || undefined,
        options: choices?.map((value) => ({ value, label: value })),
      } satisfies FormField
    })
}

/** A saved input as the form shows it again. */
export function operationFormValues(schema: OpenApiSchema, input: Record<string, unknown>): FormValues {
  const values: FormValues = {}
  for (const [id, property] of Object.entries(schema.properties ?? {})) {
    const value = input[id]
    if (value === undefined || value === null) continue
    const shape = shapeOf(id, property)
    if (shape === 'checkbox') values[id] = value === true
    else if (shape === 'choices') values[id] = Array.isArray(value) ? value.map(String) : []
    else if (shape === 'lines') values[id] = Array.isArray(value) ? value.map(String).join('\n') : String(value)
    else if (shape === 'json') values[id] = JSON.stringify(value, null, 2)
    else values[id] = String(value)
  }
  return values
}

/**
 * What the form's values send: numbers as numbers, lines as lists, JSON
 * parsed, empty answers left out, and the rule's own space filled in where
 * the operation asks for one. Text keeps its {{variables}} for the rule to
 * fill when it fires. Throws with the field to fix.
 */
export function operationInput(schema: OpenApiSchema, values: FormValues, spaceId: string): Record<string, unknown> {
  const input: Record<string, unknown> = {}
  for (const [id, property] of Object.entries(schema.properties ?? {})) {
    if (SPACE_FIELDS.has(id)) {
      input[id] = spaceId
      continue
    }
    if (HIDDEN_FIELDS.has(id)) continue
    const value: FormValue | undefined = values[id]
    const shape = shapeOf(id, property)
    if (shape === 'checkbox') {
      if (value === true) input[id] = true
      continue
    }
    if (value === undefined || (typeof value === 'string' && value.trim() === '') || (Array.isArray(value) && value.length === 0)) continue
    if (shape === 'number') {
      const number = Number(value)
      if (!Number.isFinite(number)) throw new Error(`${fieldName(id)} needs a number.`)
      input[id] = number
    } else if (shape === 'lines' && typeof value === 'string') {
      input[id] = value
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
    } else if (shape === 'json' && typeof value === 'string') {
      try {
        input[id] = JSON.parse(value)
      } catch {
        throw new Error(`${fieldName(id)} isn’t valid JSON.`)
      }
    } else {
      input[id] = value
    }
  }
  return input
}
