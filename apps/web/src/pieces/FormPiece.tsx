import { useEffect, useId, useRef, useState, type FormEvent } from 'react'

import { Banner, Button, Checkbox, SelectField, TextArea, TextField } from '../ui'
import { fieldLabel, firstMissing, type FormField, type FormValue, type FormValues } from './formFields'
import './pieces.css'

/**
 * The form piece (W1.22): one way to ask for a new thing, used by a
 * collection's public form link and by a module's in-app create. It checks
 * the required fields, says which one is missing, sends through `onSubmit`
 * (the operation) and shows why a send failed.
 */
export function FormPiece({
  autoFocus = false,
  describeFailure = defaultFailure,
  fields,
  initialValues,
  label,
  labelledBy,
  onSubmit,
  pendingLabel = 'Sending…',
  submitLabel,
  trap,
}: {
  autoFocus?: boolean
  /** A failed send in words. */
  describeFailure?: (error: unknown) => string
  fields: readonly FormField[]
  /** What the fields hold when the form opens, such as a panel's last values. */
  initialValues?: FormValues
  /** The form's name for screen readers, unless `labelledBy` names a heading. */
  label?: string
  labelledBy?: string
  /** Sends the values; a rejection shows as the form's error, success clears it. */
  onSubmit: (values: FormValues) => Promise<unknown> | unknown
  pendingLabel?: string
  submitLabel: string
  /** The name of a field people never see (T1.29); only a script fills it in. */
  trap?: string
}) {
  const [values, setValues] = useState<FormValues>(() => initialValues ?? {})
  const [error, setError] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  const errorRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const set = (id: string, value: FormValue) => {
    setValues((current) => ({ ...current, [id]: value }))
    setError(null)
  }

  async function submit(event: FormEvent) {
    event.preventDefault()
    if (sending) return
    const problem = firstMissing(fields, values)
    if (problem) {
      setError(problem)
      return
    }
    setError(null)
    setSending(true)
    try {
      await onSubmit(values)
      setValues({})
    } catch (reason) {
      setError(describeFailure(reason))
    } finally {
      setSending(false)
    }
  }

  return (
    <form aria-label={labelledBy ? undefined : label} aria-labelledby={labelledBy} className="aw-piece__form" noValidate onSubmit={submit}>
      {fields.map((field, index) => (
        <FieldInput
          autoFocus={autoFocus && index === 0}
          field={field}
          key={field.id}
          onChange={(value) => set(field.id, value)}
          value={values[field.id]}
        />
      ))}
      {trap ? (
        <div aria-hidden="true" className="aw-piece__trap">
          <label>
            Leave this empty
            <input
              autoComplete="off"
              name={trap}
              onChange={(event) => set(trap, event.target.value)}
              tabIndex={-1}
              type="text"
              value={typeof values[trap] === 'string' ? (values[trap] as string) : ''}
            />
          </label>
        </div>
      ) : null}
      {error ? (
        <div ref={errorRef} tabIndex={-1}>
          <Banner tone="danger">{error}</Banner>
        </div>
      ) : null}
      <div className="aw-piece__form-actions">
        <Button disabled={sending} type="submit" variant="primary">
          {sending ? pendingLabel : submitLabel}
        </Button>
      </div>
    </form>
  )
}

function defaultFailure(error: unknown) {
  return error instanceof Error && error.message ? error.message : 'It wasn’t sent. Check your connection and try again.'
}

/** One question's input, by its type. T1.13a's field editors replace these. */
function FieldInput({
  autoFocus,
  field,
  onChange,
  value,
}: {
  autoFocus: boolean
  field: FormField
  onChange: (value: FormValue) => void
  value: FormValue | undefined
}) {
  const hintId = useId()
  const label = fieldLabel(field)
  const hint = field.hint || undefined
  const text = typeof value === 'string' ? value : ''
  const typed = (type: string, inputMode?: 'email' | 'tel' | 'decimal', placeholder?: string) => (
    <TextField
      autoFocus={autoFocus}
      hint={hint}
      inputMode={inputMode}
      label={label}
      maxLength={field.maxLength}
      onChange={(event) => onChange(event.target.value)}
      placeholder={placeholder}
      type={type}
      value={text}
    />
  )
  switch (field.type) {
    case 'long_text':
      return (
        <TextArea
          autoFocus={autoFocus}
          hint={hint}
          label={label}
          maxLength={field.maxLength ?? 10000}
          onChange={(event) => onChange(event.target.value)}
          rows={6}
          value={text}
        />
      )
    case 'email':
      return typed('email', 'email')
    case 'phone':
      return typed('tel', 'tel')
    case 'number':
      return typed('number', 'decimal')
    case 'date':
      return typed('date')
    case 'url':
      return typed('url', undefined, 'https://')
    case 'single_choice':
    case 'status':
      return (
        <SelectField autoFocus={autoFocus} hint={hint} label={label} onChange={(event) => onChange(event.target.value)} value={text}>
          <option value="">Choose…</option>
          {(field.options ?? []).map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </SelectField>
      )
    case 'multiple_choice': {
      const picked = Array.isArray(value) ? value : []
      return (
        <fieldset aria-describedby={hint ? hintId : undefined} className="aw-piece__choices">
          <legend className="aw-piece__legend">{label}</legend>
          {hint ? (
            <p className="aw-piece__hint" id={hintId}>
              {hint}
            </p>
          ) : null}
          {(field.options ?? []).map((option) => (
            <Checkbox
              checked={picked.includes(option.value)}
              key={option.value}
              label={option.label}
              onChange={(checked) => onChange(checked ? [...picked, option.value] : picked.filter((id) => id !== option.value))}
            />
          ))}
        </fieldset>
      )
    }
    case 'checkbox':
      return <Checkbox checked={value === true} description={hint} label={field.label} onChange={(checked) => onChange(checked)} />
    default:
      return typed('text')
  }
}
