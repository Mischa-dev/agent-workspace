import type { ReactNode } from 'react'

import { Checkbox, SelectField, TextArea, TextField } from '../ui'
import { shownFields, type Answers, type PublicField } from './formModel'
import './forms.css'

/** How an internal form asks for one of the space's people. */
export type PersonInput = (props: { field: PublicField; label: string; onChange: (value: Answers[string]) => void; value: Answers[string] | undefined }) => ReactNode

/**
 * A form's questions after the title (T1.28): only those whose conditions
 * the answers so far meet, each section's heading before its first.
 */
export function Questions({
  answers,
  fields,
  onChange,
  personInput,
}: {
  answers: Answers
  fields: readonly PublicField[]
  onChange: (id: string, value: Answers[string]) => void
  personInput?: PersonInput
}) {
  return (
    <>
      {shownFields(fields, answers).map((field) => (
        <div className="aw-pform__question" key={field.id}>
          {field.section ? (
            <div className="aw-pform__section">
              <h2 className="aw-pform__section-title">{field.section.title}</h2>
              {field.section.description ? <p className="aw-pform__hint">{field.section.description}</p> : null}
            </div>
          ) : null}
          <FieldInput field={field} onChange={(value) => onChange(field.id, value)} personInput={personInput} value={answers[field.id]} />
        </div>
      ))}
    </>
  )
}

function FieldInput({
  field,
  onChange,
  personInput,
  value,
}: {
  field: PublicField
  onChange: (value: Answers[string]) => void
  personInput?: PersonInput
  value: Answers[string] | undefined
}) {
  const label = field.required ? `${field.name} (required)` : field.name
  const hint = field.description || undefined
  const text = typeof value === 'string' ? value : ''
  switch (field.type) {
    case 'text':
    case 'long_text':
      return (
        <TextArea hint={hint} label={label} maxLength={10000} onChange={(event) => onChange(event.target.value)} rows={field.type === 'long_text' ? 6 : 3} value={text} />
      )
    case 'email':
      return <TextField hint={hint} inputMode="email" label={label} onChange={(event) => onChange(event.target.value)} type="email" value={text} />
    case 'phone':
      return <TextField hint={hint} inputMode="tel" label={label} onChange={(event) => onChange(event.target.value)} type="tel" value={text} />
    case 'number':
      return <TextField hint={hint} inputMode="decimal" label={label} onChange={(event) => onChange(event.target.value)} type="number" value={text} />
    case 'date':
      return <TextField hint={hint} label={label} onChange={(event) => onChange(event.target.value)} type="date" value={text} />
    case 'url':
      return <TextField hint={hint} label={label} onChange={(event) => onChange(event.target.value)} placeholder="https://" type="url" value={text} />
    case 'single_choice':
    case 'status':
      return (
        <SelectField hint={hint} label={label} onChange={(event) => onChange(event.target.value)} value={text}>
          <option value="">Choose…</option>
          {field.options.map((option) => (
            <option key={option.id} value={option.id}>
              {option.name}
            </option>
          ))}
        </SelectField>
      )
    case 'multiple_choice': {
      const picked = Array.isArray(value) ? value : []
      return (
        <fieldset className="aw-pform__group">
          <legend className="aw-pform__legend">{label}</legend>
          {hint ? <p className="aw-pform__hint">{hint}</p> : null}
          {field.options.map((option) => (
            <Checkbox
              checked={picked.includes(option.id)}
              key={option.id}
              label={option.name}
              onChange={(checked) => onChange(checked ? [...picked, option.id] : picked.filter((id) => id !== option.id))}
            />
          ))}
        </fieldset>
      )
    }
    case 'checkbox':
      return <Checkbox checked={value === true} description={hint} label={field.name} onChange={(checked) => onChange(checked)} />
    case 'person':
    case 'people':
      return personInput ? personInput({ field, label, onChange, value }) : null
  }
}

