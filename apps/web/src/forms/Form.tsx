import { FormEvent, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { BrandMark } from '../auth/AuthLayout'
import { Banner, Button, TextField } from '../ui'
import { fetchForm, FormProblem, MIN_RENDER_AGE_MS, missing, sendAnswers, type Answers, type PublicForm } from './formModel'
import { Questions } from './Questions'
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
  const loadedAt = useRef(0)

  useEffect(() => {
    let live = true
    fetchForm(token).then(
      (form) => {
        if (!live) return
        loadedAt.current = Date.now()
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
      // The form refuses answers sent sooner after it loaded than a person
      // could type them; a quick person (or autofill) just waits it out.
      const wait = loadedAt.current + MIN_RENDER_AGE_MS + 200 - Date.now()
      if (wait > 0) await new Promise((resolve) => setTimeout(resolve, wait))
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
        <Questions answers={answers} fields={form.fields} onChange={set} />
        {form.honeypot ? (
          <div aria-hidden="true" className="aw-pform__trap">
            <label>
              Leave this empty
              <input
                autoComplete="off"
                name={form.honeypot}
                onChange={(event) => set(form.honeypot, event.target.value)}
                tabIndex={-1}
                type="text"
                value={typeof answers[form.honeypot] === 'string' ? (answers[form.honeypot] as string) : ''}
              />
            </label>
          </div>
        ) : null}
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

// The app loads it lazily, as its default.
export default FormPage
