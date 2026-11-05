import { Icon } from '../ui'
import { chosenOptions, displayText, isEmpty, type FieldDefinition, type FieldReference, type FieldValue } from './fieldModel'

/**
 * A field's value as list cells, board cards and read-only pages show it:
 * choices as the same colored chips labels use, a checkbox as a check, a URL
 * as a link, and anything else as text. Nothing when it's empty, unless
 * `empty` says what to show.
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
  if (isEmpty(value)) return empty ? <span className="aw-prop__empty">{empty}</span> : null
  if (field.type === 'single_choice' || field.type === 'multiple_choice') {
    return (
      <ul aria-label={field.name} className="aw-labels">
        {chosenOptions(field, value).map((option) => (
          <li className="aw-label" key={option.id}>
            <span aria-hidden className="aw-label__dot" style={{ backgroundColor: option.color }} />
            <span className="aw-label__name">{option.name}</span>
          </li>
        ))}
      </ul>
    )
  }
  if (field.type === 'checkbox') {
    return (
      <span className="aw-prop__text">
        <Icon name="check" size={14} />
        <span className="aw-sr-only">Yes</span>
      </span>
    )
  }
  if (field.type === 'url' && typeof value === 'string') {
    let host = value
    try {
      host = new URL(value).host
    } catch {
      // Shown as written.
    }
    return (
      <a className="aw-prop__text" href={value} onClick={(event) => event.stopPropagation()} rel="noreferrer noopener" target="_blank">
        {host}
      </a>
    )
  }
  return <span className="aw-prop__text">{displayText(field, value, references)}</span>
}
