import type { FieldType } from '../fields/fieldModel'

// What the form piece (W1.22) asks for, whoever draws it: the public form
// link and a module's in-app create. Until T1's field registry lands its
// editors (T1.13a), each type gets the plain input below.

/** One question on a form. */
export type FormField = {
  id: string
  label: string
  type: FieldType
  required?: boolean
  /** A line under the input: what to write, or why it's asked. */
  hint?: string
  options?: { value: string; label: string }[]
  maxLength?: number
}

/** What the person typed or picked so far, by field id. Numbers stay as typed until sent. */
export type FormValue = string | string[] | boolean
export type FormValues = Record<string, FormValue>

/** Whether a value counts as filled in. A checkbox always does: unticked is an answer. */
export function filled(field: FormField, value: FormValue | undefined) {
  if (field.type === 'checkbox') return true
  if (value === undefined) return false
  if (typeof value === 'string') return value.trim() !== ''
  if (Array.isArray(value)) return value.length > 0
  return true
}

/** The first required field still empty, in words ("Fill in Severity."), or null. */
export function firstMissing(fields: readonly FormField[], values: FormValues): string | null {
  const field = fields.find((candidate) => candidate.required && !filled(candidate, values[candidate.id]))
  return field ? `Fill in ${field.label}.` : null
}

/** How a field is labelled: its name, and "(required)" when it is. */
export const fieldLabel = (field: FormField) => (field.required ? `${field.label} (required)` : field.label)
