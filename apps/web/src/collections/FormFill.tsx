import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { opQuery, useOp } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import { answerBody, missing, normalizeQuestions, type Answers } from '../forms/formModel'
import { Questions, type PersonInput } from '../forms/Questions'
import { Banner, Button, Checkbox, Dialog, ErrorState, ListSkeleton, SelectField, TextField, describeError, showToast } from '../ui'
import { useAssignableActors } from '../work-items/hooks'
import { collectionKeys } from './keys'

/**
 * An internal form (T1.28), filled in by someone in the space: its
 * questions as the answers so far show them, and each answer a record
 * written as them.
 */
export function FormFill({ formId, onBack, onClose, projectId }: { formId: string; onBack: () => void; onClose: () => void; projectId: string }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const client = useQueryClient()
  const form = useQuery(opQuery('form_get', { form: formId }, { workspaceId }))
  const [answers, setAnswers] = useState<Answers>({})
  const [problem, setProblem] = useState<string | null>(null)
  const send = useOp('form_answer_send', {
    workspaceId,
    onSuccess: (answered) => {
      void client.invalidateQueries({ queryKey: collectionKeys.allRecords(projectId, answered.collectionId) })
      showToast({ title: answered.thanks || `${answered.title} added`, tone: 'success' })
      setAnswers({})
    },
  })
  if (!form.data) {
    return (
      <Dialog footer={<Button onClick={onBack}>Back</Button>} onClose={onClose} open title="Fill in a form" width={560}>
        {form.isError ? <ErrorState error={form.error} onRetry={() => void form.refetch()} title="The form didn’t load" /> : <ListSkeleton label="Loading the form" rows={3} />}
      </Dialog>
    )
  }
  const data = form.data
  const questions = normalizeQuestions(data.questions, 'internal')
  const set = (id: string, value: Answers[string]) => {
    setAnswers((current) => ({ ...current, [id]: value }))
    setProblem(null)
  }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (send.isPending) return
    const gap = missing({ titleName: data.titleName, fields: questions }, answers)
    if (gap) {
      setProblem(gap)
      return
    }
    send.mutate({ form: data.id, answers: answerBody({ fields: questions }, answers) })
  }
  const closed = !data.canAnswer
  return (
    <Dialog
      description={data.definition && typeof data.definition === 'object' && 'intro' in data.definition ? String(data.definition.intro ?? '') : undefined}
      dirty={Object.keys(answers).length > 0}
      footer={
        <>
          <Button onClick={onBack}>Back</Button>
          <Button busy={send.isPending} disabled={closed} type="submit" variant="primary">
            Send
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={data.name}
      width={560}
    >
      <div className="aw-form">
        {closed ? <Banner tone="warning">{data.archived ? 'This form is archived, so it isn’t taking answers.' : 'Your role here can’t add records, so you can’t fill this in.'}</Banner> : null}
        <TextField
          data-autofocus
          label={`${data.titleName} (required)`}
          maxLength={500}
          onChange={(event) => set('title', event.target.value)}
          value={typeof answers.title === 'string' ? answers.title : ''}
        />
        <Questions answers={answers} fields={questions} onChange={set} personInput={(props) => <PersonQuestion {...props} projectId={projectId} />} />
        {problem ? <Banner tone="danger">{problem}</Banner> : null}
        {send.error ? <Banner tone="danger">{describeError(send.error).message}</Banner> : null}
      </div>
    </Dialog>
  )
}

/** One of the space's people, or several. */
function PersonQuestion({ field, label, onChange, projectId, value }: Parameters<PersonInput>[0] & { projectId: string }) {
  const people = useAssignableActors(projectId, '')
  const list = people.data ?? []
  if (field.type === 'people') {
    const picked = Array.isArray(value) ? value : []
    return (
      <fieldset className="aw-pform__group">
        <legend className="aw-pform__legend">{label}</legend>
        {list.map((person) => (
          <Checkbox
            checked={picked.includes(person.id)}
            key={person.id}
            label={person.displayName}
            onChange={(checked) => onChange(checked ? [...picked, person.id] : picked.filter((id) => id !== person.id))}
          />
        ))}
      </fieldset>
    )
  }
  return (
    <SelectField hint={field.description || undefined} label={label} onChange={(event) => onChange(event.target.value)} value={typeof value === 'string' ? value : ''}>
      <option value="">{people.isPending ? 'Loading people…' : 'Choose someone…'}</option>
      {list.map((person) => (
        <option key={person.id} value={person.id}>
          {person.displayName} (@{person.handle})
        </option>
      ))}
    </SelectField>
  )
}
