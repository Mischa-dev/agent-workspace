import { useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useFields } from '../fields/api'
import type { FieldDefinition, FieldValue } from '../fields/fieldModel'
import { useProjects } from '../projects/hooks'
import { Banner, Button, Dialog, SelectField, countLabel, describeError, showToast } from '../ui'
import { workItemHref } from './links'
import { moveTask, previewMove, type MoveIssue, type MovePlan, type MoveResolution } from './moveApi'
import { FieldEditor } from './TaskCustomFields'
import './moveTask.css'

/** What the mover chose for one issue. */
type Choice = { kind: 'leave' } | { kind: 'set'; targetField: string; value?: FieldValue }

/**
 * Moving a task, with its subtasks, to another project (AWD-102): pick the
 * project, read a private preview of what carries over and what needs a
 * choice, settle each issue (put it in a target field, or leave it on the
 * original), then confirm once. Nothing moves until then.
 */
export default function MoveTaskDialog({
  onClose,
  projectId,
  task,
}: {
  onClose: () => void
  projectId: string
  task: { id: string; key: string; title: string }
}) {
  const [target, setTarget] = useState<string | null>(null)
  const [picked, setPicked] = useState('')
  const projects = useProjects()
  const destinations = (projects.data?.projects ?? []).filter((project) => project.id !== projectId && !project.archivedAt)

  if (!target) {
    return (
      <Dialog
        footer={
          <>
            <Button onClick={onClose} variant="ghost">
              Cancel
            </Button>
            <Button disabled={!picked} onClick={() => setTarget(picked)} variant="primary">
              Review the move
            </Button>
          </>
        }
        onClose={onClose}
        open
        title={`Move ${task.key} to another project`}
      >
        <p className="aw-move__lede">
          It moves with its subtasks. You’ll see what carries over before anything changes.
        </p>
        <SelectField label="Project" onChange={(event) => setPicked(event.target.value)} value={picked}>
          <option value="">{projects.isLoading ? 'Loading projects…' : 'Choose a project'}</option>
          {destinations.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name} ({project.key})
            </option>
          ))}
        </SelectField>
        {projects.isError ? <Banner tone="danger" title={describeError(projects.error).message} /> : null}
      </Dialog>
    )
  }
  return (
    <MoveReview
      key={target}
      onBack={() => setTarget(null)}
      onClose={onClose}
      projectId={projectId}
      targetProjectId={target}
      task={task}
    />
  )
}

function MoveReview({
  onBack,
  onClose,
  projectId,
  targetProjectId,
  task,
}: {
  onBack: () => void
  onClose: () => void
  projectId: string
  targetProjectId: string
  task: { id: string; key: string; title: string }
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const preview = useQuery({
    queryKey: ['task-move-preview', projectId, task.id, targetProjectId],
    queryFn: () => previewMove(projectId, task.id, targetProjectId),
    // A preview is private and goes stale fast: never reuse an old one.
    gcTime: 0,
    staleTime: 0,
    retry: false,
  })
  const targetFields = useFields({ projectId: targetProjectId })
  const [choices, setChoices] = useState<Record<string, Choice>>({})
  const [stale, setStale] = useState(false)
  // One key per request: the same choices retried (after a lost response)
  // send the same key and get the first result; changed choices get a new one.
  const sent = useRef<{ signature: string; key: string } | null>(null)

  const plan = preview.data
  const issues = plan?.issues ?? []
  // Settled: left, or put in a field with a value (its own, when it fits).
  const settled = (issue: MoveIssue) => {
    const choice = choices[issue.id]
    if (!choice) return false
    if (choice.kind === 'leave' || choice.value !== undefined) return true
    return Boolean(issue.candidates?.find((candidate) => candidate.id === choice.targetField)?.takesValue)
  }
  const unresolved = issues.filter((issue) => !settled(issue)).length

  const resolutions = (current: MovePlan): MoveResolution[] =>
    (current.issues ?? []).flatMap((issue): MoveResolution[] => {
      const choice = choices[issue.id]
      if (!choice) return []
      if (choice.kind === 'leave') return [{ issue: issue.id, action: 'leave' }]
      return [{ issue: issue.id, action: 'set', targetField: choice.targetField, ...(choice.value === undefined ? {} : { value: choice.value }) }]
    })

  const move = useMutation({
    mutationFn: async (current: MovePlan) => {
      const input = { targetProjectId, snapshot: current.snapshot, resolutions: resolutions(current), leaveUnresolved: false }
      const signature = JSON.stringify(input)
      if (sent.current?.signature !== signature) sent.current = { signature, key: crypto.randomUUID() }
      return moveTask(projectId, task.id, input, sent.current.key)
    },
    onSuccess: (done) => {
      void queryClient.invalidateQueries({ queryKey: ['work-items'] })
      const first = done.moved[0]
      showToast({
        tone: 'success',
        title: first ? `Moved ${first.from} to ${first.to}` : `${task.key} was already moved`,
        description: done.leftBehind ? `${countLabel(done.leftBehind, 'field value')} stayed on the original.` : undefined,
      })
      onClose()
      if (first) navigate(workItemHref(targetProjectId, first.id))
    },
    onError: (error) => {
      // A changed preview needs a fresh one; a lost response keeps its key,
      // so trying again can't move twice.
      if (isApiError(error) && error.status === 409) setStale(true)
    },
  })

  const refresh = () => {
    setStale(false)
    setChoices({})
    sent.current = null
    move.reset()
    void preview.refetch()
  }

  const leaveRest = () =>
    setChoices((current) => {
      const next = { ...current }
      for (const issue of issues) if (!settled(issue)) next[issue.id] = { kind: 'leave' }
      return next
    })

  const moveError = move.error ? (isApiError(move.error) && move.error.status < 500 ? move.error.message : describeError(move.error).message) : null
  const previewError = preview.error ? (isApiError(preview.error) && preview.error.status !== 404 ? preview.error.message : describeError(preview.error).message) : null

  return (
    <Dialog
      footer={
        <>
          <Button disabled={move.isPending} onClick={onBack} variant="ghost">
            Back
          </Button>
          <Button disabled={move.isPending} onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button
            disabled={!plan || unresolved > 0 || move.isPending || stale}
            onClick={() => plan && move.mutate(plan)}
            variant="primary"
          >
            {move.isPending ? 'Moving…' : plan && plan.tasks.length > 1 ? `Move ${plan.tasks.length} tasks` : 'Move task'}
          </Button>
        </>
      }
      onClose={() => {
        if (!move.isPending) onClose()
      }}
      open
      title={plan ? `Move ${task.key} to ${plan.targetProjectKey}` : `Move ${task.key}`}
      width={640}
    >
      {preview.isLoading ? <p className="aw-move__lede">Checking what carries over…</p> : null}
      {previewError ? <Banner tone="danger" title={previewError} /> : null}
      {plan ? (
        <div className="aw-move">
          <p className="aw-move__lede">
            {plan.tasks.length > 1 ? `${task.key} and ${countLabel(plan.tasks.length - 1, 'subtask')} move.` : `${task.key} moves.`} The original is archived with its full history and every field value. Only you see this preview.
          </p>
          {stale ? (
            <Banner
              action={
                <Button onClick={refresh} size="sm">
                  Preview again
                </Button>
              }
              title={moveError ?? 'Something changed since this preview.'}
              tone="warning"
            />
          ) : moveError ? (
            <Banner tone="danger" title={moveError} />
          ) : null}
          {issues.length ? (
            <section aria-labelledby="move-issues" className="aw-move__section">
              <div className="aw-move__heading">
                <h3 id="move-issues">Needs a choice</h3>
                {unresolved > 0 ? (
                  <Button onClick={leaveRest} size="sm" variant="ghost">
                    Leave all unresolved
                  </Button>
                ) : null}
              </div>
              <ul className="aw-move__issues">
                {issues.map((issue) => (
                  <IssueRow
                    choice={choices[issue.id]}
                    issue={issue}
                    key={issue.id}
                    onChoose={(choice) => setChoices((current) => ({ ...current, [issue.id]: choice }))}
                    onClear={() =>
                      setChoices((current) => {
                        const next = { ...current }
                        delete next[issue.id]
                        return next
                      })
                    }
                    targetFields={targetFields.data?.fields ?? []}
                    targetProjectId={targetProjectId}
                  />
                ))}
              </ul>
            </section>
          ) : null}
          {plan.carried?.length ? (
            <section aria-labelledby="move-carried" className="aw-move__section">
              <h3 id="move-carried">Carries over</h3>
              <ul className="aw-move__list">
                {plan.carried.map((value) => (
                  <li key={`${value.taskId}:${value.field}`}>
                    <span className="aw-move__key">{value.taskKey}</span> {value.field}: {shown(value.value)}
                    {value.targetField !== value.field ? ` → ${value.targetField}` : ''}
                    {value.followsMove ? ' (links to its moved copy)' : ''}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {plan.notes?.length || plan.archivedFieldValues ? (
            <section aria-labelledby="move-notes" className="aw-move__section">
              <h3 id="move-notes">Also</h3>
              <ul className="aw-move__list">
                {plan.notes?.map((note) => <li key={note}>{note}</li>)}
                {plan.archivedFieldValues ? <li>{countLabel(plan.archivedFieldValues, 'value')} in archived fields stay on the original.</li> : null}
              </ul>
            </section>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  )
}

function IssueRow({
  choice,
  issue,
  onChoose,
  onClear,
  targetFields,
  targetProjectId,
}: {
  choice: Choice | undefined
  issue: MoveIssue
  onChoose: (choice: Choice) => void
  onClear: () => void
  targetFields: readonly FieldDefinition[]
  targetProjectId: string
}) {
  const [editing, setEditing] = useState<HTMLElement | null>(null)
  const latest = useRef<FieldValue | null>(null)
  const candidates = issue.candidates ?? []
  const selected = choice?.kind === 'set' ? candidates.find((candidate) => candidate.id === choice.targetField) : undefined
  const definition = useMemo(() => targetFields.find((field) => field.id === selected?.id), [targetFields, selected])
  const where = choice ? (choice.kind === 'leave' ? 'leave' : `set:${choice.targetField}`) : ''

  const finish = () => {
    setEditing(null)
    const value = latest.current
    latest.current = null
    if (selected && value !== null && !(Array.isArray(value) && value.length === 0)) {
      onChoose({ kind: 'set', targetField: selected.id, value })
    }
  }

  return (
    <li className="aw-move__issue">
      <div className="aw-move__what">
        <span className="aw-move__key">{issue.taskKey}</span> <strong>{issue.field}</strong>: {shown(issue.value)}
        <div className="aw-move__why">{issue.message}</div>
      </div>
      <div className="aw-move__how">
        <SelectField
          aria-label={`What happens to ${issue.field} on ${issue.taskKey}`}
          compact
          onChange={(event) => {
            const next = event.target.value
            if (!next) return onClear()
            if (next === 'leave') return onChoose({ kind: 'leave' })
            const candidate = candidates.find((entry) => `set:${entry.id}` === next)
            // A field the value fits as it is keeps it; otherwise a value is chosen next.
            if (candidate?.takesValue) onChoose({ kind: 'set', targetField: candidate.id })
            else if (candidate) onChoose({ kind: 'set', targetField: candidate.id, value: undefined })
          }}
          value={where}
        >
          <option value="">Choose…</option>
          <option value="leave">Leave on the original</option>
          {candidates.map((candidate) => (
            <option key={candidate.id} value={`set:${candidate.id}`}>
              Put in {candidate.name}
            </option>
          ))}
        </SelectField>
        {selected ? (
          <div className="aw-move__value">
            {choice?.kind === 'set' && choice.value !== undefined ? (
              <span>{chosenText(definition, choice.value)}</span>
            ) : selected.takesValue ? (
              <span>Same value</span>
            ) : (
              <span className="aw-move__needed">Choose a value</span>
            )}
            {definition ? (
              <Button onClick={(event) => setEditing(event.currentTarget)} size="sm" variant="ghost">
                {choice?.kind === 'set' && choice.value !== undefined ? 'Change' : selected.takesValue ? 'Another value…' : 'Choose…'}
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
      {editing && definition ? (
        <FieldEditor
          anchor={editing}
          field={definition}
          onClose={finish}
          onSet={(value) => {
            latest.current = value
          }}
          projectId={targetProjectId}
          references={[]}
          taskId=""
          value={undefined}
        />
      ) : null}
    </li>
  )
}

/** A value from the preview, as one line. */
function shown(value: unknown): string {
  if (value === null || value === undefined || value === '') return '—'
  if (Array.isArray(value)) return value.map(shown).join(', ')
  if (value === true) return 'Yes'
  return String(value)
}

/** A chosen target value: option names for a choice; otherwise as typed, or "Chosen". */
function chosenText(field: FieldDefinition | undefined, value: FieldValue): string {
  if (!field) return 'Chosen'
  const ids = Array.isArray(value) ? value : [String(value)]
  if (field.type === 'single_choice' || field.type === 'multiple_choice') {
    return ids.map((id) => field.options.find((option) => option.id === id)?.name ?? id).join(', ')
  }
  if (field.type === 'person' || field.type === 'link') return 'Chosen'
  return shown(value)
}
