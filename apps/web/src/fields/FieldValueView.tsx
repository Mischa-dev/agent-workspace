import { isEmpty, type FieldDefinition, type FieldReference, type FieldValue } from './fieldModel'
import { fieldSpec } from './registry'
import '../styles/work.css'

/**
 * A field's value as list cells, board cards and read-only pages show it,
 * drawn by its type's renderer in the field registry: choices and statuses
 * as the same colored chips labels use, a checkbox as a check, a URL, email
 * or phone as a link, and anything else as text. Nothing when it's empty,
 * unless `empty` says what to show.
 */
export function FieldValueView({
  empty,
  field,
  references = [],
  value,
}: {
  empty?: string
  field: FieldDefinition
  references?: readonly FieldReference[]
  value: FieldValue | undefined
}) {
  if (value === undefined || isEmpty(value)) return empty ? <span className="aw-prop__empty">{empty}</span> : null
  const { Renderer } = fieldSpec(field.type)
  return <Renderer field={field} references={references} value={value} />
}
