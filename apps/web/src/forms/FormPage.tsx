import { FormEvent, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { BrandMark } from '../auth/AuthLayout'
import { Banner, Button, Checkbox, SelectField, TextArea, TextField } from '../ui'
import { fetchForm, FormProblem, missing, sendAnswers, type Answers, type PublicField, type PublicForm } from './formModel'
import './forms.css'

type Loaded = { state: 'loading' } | { state: 'ready'; form: PublicForm } | { state: 'gone'; message: string }

/**
 * A collection's public form (AWD-110): anyone with the link fills it in,
 * without signing in, and each answer becomes one record.
 */
export function FormPage() {
  const { token = '' } = useParams()
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' })
  const [answers, setAnswers] = useState<Answers>({})
  const [error, setError] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  const [thanks, setThanks] = useState<string | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let live = true
    fetchForm(token).then(
      (form) => {
        if (!live) return
        setLoaded({ state: 'ready', form })
        document.title = form.name || form.collection
      },
      (reason: unknown) => {
        if (!live) return
        setLoaded({
          state: 'gone',
          message: reason instanceof FormProblem ? reason.message : 'This form didn’t load. Check your connection and reload.',
        })
      },
    )
    return () => {
      live = false
    }
  }, [token])

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  if (loaded.state === 'loading') {
    return (
      <FormShell>
        <p aria-busy="true" className="aw-pform__quiet">
          Opening the form…
        </p>
      </FormShell>
    )
  }
  if (loaded.state === 'gone') {
    return (
      <FormShell>
        <h1 className="aw-pform__title">Form unavailable</h1>
        <p className="aw-pform__intro">{loaded.message}</p>
      </FormShell>
    )
  }

  const form = loaded.form
  if (thanks !== null) {
    return (
      <FormShell>
        <h1 className="aw-pform__title">Sent</h1>
        <p className="aw-pform__intro">{thanks || 'Thanks, your answers are in.'}</p>
        <div className="aw-pform__actions">
          <Button
            onClick={() => {
              setAnswers({})
              setThanks(null)
            }}
            variant="secondary"
          >
            Send another
          </Button>
        </div>
      </FormShell>
    )
  }

  const set = (id: string, value: Answers[string]) => {
    setAnswers((current) => ({ ...current, [id]: value }))
    setError(null)
  }

  async function submit(event: FormEvent) {
    event.preventDefault()
    const problem = missing(form, answers)
    if (problem) {
      setError(problem)
      return
    }
    setError(null)
    setSending(true)
    try {
      setThanks(await sendAnswers(token, form, answers))
    } catch (reason) {
      setError(reason instanceof FormProblem ? reason.message : 'Your answers weren’t sent. Check your connection and try again.')
    } finally {
      setSending(false)
    }
  }

  return (
    <FormShell>
      <p className="aw-pform__kicker">{form.collection}</p>
      <h1 className="aw-pform__title" id="form-title">
        {form.name || form.collection}
      </h1>
      {form.intro ? <p className="aw-pform__intro">{form.intro}</p> : null}
      <form aria-labelledby="form-title" className="aw-pform__fields" noValidate onSubmit={submit}>
        <TextField
          autoFocus
          label={`${form.titleName} (required)`}
          maxLength={500}
          onChange={(event) => set('title', event.target.value)}
          value={typeof answers.title === 'string' ? answers.title : ''}
        />
        {form.fields.map((field) => (
          <FieldInput field={field} key={field.id} onChange={(value) => set(field.id, value)} value={answers[field.id]} />
        ))}
        {error ? (
          <div ref={errorRef} tabIndex={-1}>
            <Banner tone="danger">{error}</Banner>
          </div>
        ) : null}
        <div className="aw-pform__actions">
          <Button disabled={sending} type="submit" variant="primary">
            {sending ? 'Sending…' : 'Send'}
          </Button>
        </div>
      </form>
    </FormShell>
  )
}

function FormShell({ children }: { children: React.ReactNode }) {
  return (
    <main className="aw-pform">
      <section className="aw-pform__card">{children}</section>
      <p className="aw-pform__brand">
        <BrandMark size={18} />
        <span>Made with Agent Workspace</span>
      </p>
    </main>
  )
}

function FieldInput({
  field,
  onChange,
  value,
}: {
  field: PublicField
  onChange: (value: Answers[string]) => void
  value: Answers[string] | undefined
}) {
  const label = field.required ? `${field.name} (required)` : field.name
  const hint = field.description || undefined
  const text = typeof value === 'string' ? value : ''
  switch (field.type) {
    case 'text':
      return <TextArea hint={hint} label={label} maxLength={10000} onChange={(event) => onChange(event.target.value)} rows={3} value={text} />
    case 'number':
      return <TextField hint={hint} inputMode="decimal" label={label} onChange={(event) => onChange(event.target.value)} type="number" value={text} />
    case 'date':
      return <TextField hint={hint} label={label} onChange={(event) => onChange(event.target.value)} type="date" value={text} />
    case 'url':
      return <TextField hint={hint} label={label} onChange={(event) => onChange(event.target.value)} placeholder="https://" type="url" value={text} />
    case 'single_choice':
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
  }
}
