import type { EditorProps } from './editors'
import { fieldSpec } from './registry'

/**
 * The popover that changes one field's value, drawn by its type's editor in
 * the field registry: a task's fields, the bulk bar's change to many tasks
 * at once, and a collection's cells and record panel (AWD-90). Nothing for a
 * type nobody fills in.
 */
export function FieldEditor(props: EditorProps) {
  const { Editor } = fieldSpec(props.field.type)
  return Editor ? <Editor {...props} /> : null
}
