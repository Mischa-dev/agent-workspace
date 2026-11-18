import { useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'

import { BrandMark } from '../auth/AuthLayout'
import { FormPiece } from '../pieces/FormPiece'
import { Button } from '../ui'
import { fetchForm, formFields, FormProblem, MIN_RENDER_AGE_MS, sendAnswers, type PublicForm } from './formModel'
import './forms.css'

type Loaded = { state: 'loading' } | { state: 'ready'; form: PublicForm } | { state: 'gone'; message: string }

/**
 * A collection's public form (AWD-110): anyone with the link fills it in,
 * without signing in, and each answer becomes one record.
 */
export function FormPage() {
  const { token = '' } = useParams()
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' })
  const [thanks, setThanks] = useState<string | null>(null)
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
          <Button onClick={() => setThanks(null)} variant="secondary">
            Send another
          </Button>
        </div>
      </FormShell>
    )
  }

  return (
    <FormShell>
      <p className="aw-pform__kicker">{form.collection}</p>
      <h1 className="aw-pform__title" id="form-title">
        {form.name || form.collection}
      </h1>
      {form.intro ? <p className="aw-pform__intro">{form.intro}</p> : null}
      <FormPiece
        autoFocus
        describeFailure={(reason) =>
          reason instanceof FormProblem ? reason.message : 'Your answers weren’t sent. Check your connection and try again.'
        }
        fields={formFields(form)}
        labelledBy="form-title"
        onSubmit={async (answers) => {
          // The form refuses answers sent sooner after it loaded than a person
          // could type them; a quick person (or autofill) just waits it out.
          const wait = loadedAt.current + MIN_RENDER_AGE_MS + 200 - Date.now()
          if (wait > 0) await new Promise((resolve) => setTimeout(resolve, wait))
          setThanks(await sendAnswers(token, form, answers))
        }}
        submitLabel="Send"
        trap={form.honeypot || undefined}
      />
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
