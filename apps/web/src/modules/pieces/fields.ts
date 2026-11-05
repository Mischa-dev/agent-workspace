import { SHARED_FIELDS, type FieldDecl, type FieldValue, type Thing, type ThingDecl } from '../contract'

// Reading a thing's fields for the pieces: the shared ones (title, status,
// owner) and the ones its kind declares, in words.

/** A field's declaration: a shared one, or one of the thing's own. */
export function fieldOf(decl: ThingDecl, id: string): FieldDecl {
  return SHARED_FIELDS.find((field) => field.id === id) ?? decl.fields.find((field) => field.id === id) ?? { id, label: id, type: 'text' }
}

export function rawValue(thing: Thing, id: string): FieldValue | undefined {
  if (id === 'title') return thing.title
  if (id === 'status') return thing.status ?? null
  if (id === 'owner') return thing.owner?.name ?? null
  return thing.fields?.[id]
}

/** `in_progress` to "In progress". */
export const words = (value: string) => value.replace(/_/g, ' ').replace(/^\w/, (first) => first.toUpperCase())

/** A field's value in words; null when it has none. */
export function fieldText(field: FieldDecl, value: FieldValue | undefined): string | null {
  if (value === null || value === undefined || value === '') return null
  if (Array.isArray(value)) return value.length ? value.join(', ') : null
  switch (field.type) {
    case 'checkbox':
      return value ? 'Yes' : 'No'
    case 'date': {
      const date = new Date(String(value))
      return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
    }
    case 'choice':
      return words(String(value))
    default:
      return String(value)
  }
}
