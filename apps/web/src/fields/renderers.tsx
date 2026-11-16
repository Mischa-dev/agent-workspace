import { Icon } from '../ui'
import { chosenOptions, displayText, isFormulaError, type FieldDefinition, type FieldReference, type FieldValue } from './fieldModel'
import '../styles/work.css'

/**
 * How a value of each kind of field reads (T1.13a): the renderers the
 * field registry (`registry.ts`) gives each type. `FieldValueView` picks
 * one; a value is never empty here.
 */
export type RendererProps = {
  field: FieldDefinition
  references: readonly FieldReference[]
  value: FieldValue
}

/** Choices and statuses: the same colored chips labels use. */
export function ChipsRenderer({ field, value }: RendererProps) {
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

/** A checked checkbox: a check. */
export function CheckRenderer() {
  return (
    <span className="aw-prop__text">
      <Icon name="check" size={14} />
      <span className="aw-sr-only">Yes</span>
    </span>
  )
}

const keepRowClosed = (event: { stopPropagation: () => void }) => event.stopPropagation()

/** A web address: a link that shows its host. */
export function UrlRenderer({ field, references, value }: RendererProps) {
  if (typeof value !== 'string') return <TextRenderer field={field} references={references} value={value} />
  let host = value
  try {
    host = new URL(value).host
  } catch {
    // Shown as written.
  }
  return (
    <a className="aw-prop__text" href={value} onClick={keepRowClosed} rel="noreferrer noopener" target="_blank">
      {host}
    </a>
  )
}

/** An email address: a mail link. */
export function EmailRenderer({ field, references, value }: RendererProps) {
  if (typeof value !== 'string') return <TextRenderer field={field} references={references} value={value} />
  return (
    <a className="aw-prop__text" href={`mailto:${value}`} onClick={keepRowClosed}>
      {value}
    </a>
  )
}

/** A phone number: a call link, dialling its digits. */
export function PhoneRenderer({ field, references, value }: RendererProps) {
  if (typeof value !== 'string') return <TextRenderer field={field} references={references} value={value} />
  return (
    <a className="aw-prop__text" href={`tel:${value.replace(/[^0-9+]/g, '')}`} onClick={keepRowClosed}>
      {value}
    </a>
  )
}

/** Paragraphs, with their line breaks. */
export function LongTextRenderer({ field, references, value }: RendererProps) {
  return (
    <span className="aw-prop__text" style={{ whiteSpace: 'pre-line' }}>
      {displayText(field, value, references)}
    </span>
  )
}

/** Anything else: the value as one line of text (a number, a date, a person or a linked task, page or record by name). */
export function TextRenderer({ field, references, value }: RendererProps) {
  return <span className="aw-prop__text">{displayText(field, value, references)}</span>
}

/** A formula's result, by what it gives; an error for this record says so, with its message. */
export function FormulaRenderer({ field, references, value }: RendererProps) {
  if (isFormulaError(value)) {
    return (
      <span className="aw-prop__text aw-prop__formula-error" title={value.error}>
        Error<span className="aw-sr-only">: {value.error}</span>
      </span>
    )
  }
  if (field.formula?.result === 'boolean' && value === true) return <CheckRenderer />
  return <span className="aw-prop__text">{displayText(field, value, references)}</span>
}
