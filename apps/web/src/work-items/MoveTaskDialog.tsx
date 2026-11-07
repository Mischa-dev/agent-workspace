import { useMemo, useRef, useState, useSyncExternalStore, type RefObject } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useFields } from '../fields/api'
import type { FieldDefinition, FieldValue } from '../fields/fieldModel'
import { useProjects } from '../projects/hooks'
import { Banner, Button, Dialog, SelectField, countLabel, describeError, showToast } from '../ui'
import { workItemHref } from './links'
import { canMoveInto, moveTask, previewMove, type MoveDone, type MoveIssue, type MovePlan, type MoveResolution } from './moveApi'
import { pendingMove, rememberMove, settleMove, subscribeMoves, type PendingMove } from './moveRecovery'
import { FieldEditor } from './TaskCustomFields'
import './moveTask.css'

/** What the mover chose for one issue; `label` is how a picked person, task or page reads. */
type Choice = { kind: 'leave' } | { kind: 'set'; targetField: string; value?: FieldValue; label?: string | null }

type MovingTask = { id: string; key: string; title: string; version: number }

/** An answer that doesn't say whether the move happened: no response, or the server failing. */
function unknownOutcome(error: unknown) {
  return !isApiError(error) || error.status >= 500
}

/**
 * Moving a task, with its subtasks, to another project (AWD-102): pick the
 * project, read a private preview of what carries over and what needs a
 * choice, settle each issue (put it in a target field, or leave it on the
 * original), then confirm once. Nothing moves until then. If the answer to
 * the confirmation never arrives, the dialog only offers to send that same
 * request again, until it's settled.
 */
export default function MoveTaskDialog({ onClose, projectId, task }: { onClose: () => void; projectId: string; task: MovingTask }) {
  // Every step is its own dialog: focus goes back to whatever opened the first.
  const [opener] = useState<RefObject<HTMLElement | null>>(() => ({ current: document.activeElement as HTMLElement | null }))
  const pending = useSyncExternalStore(subscribeMoves, () => pendingMove(projectId, task.id))
  const [target, setTarget] = useState<string | null>(null)
  const [picked, setPicked] = useState('')
  const projects = useProjects()
  const destinations = (projects.data?.projects ?? []).filter((project) => canMoveInto(project, projectId))

  if (pending) return <MoveRecovery move={pending} onClose={onClose} opener={opener} onStartOver={() => setTarget(null)} projectId={projectId} task={task} />
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
        returnFocus={opener}
      >
        <p className="aw-move__lede">It moves with its subtasks. You’ll see what carries over before anything changes.</p>
        <SelectField label="Project" onChange={(event) => setPicked(event.target.value)} value={picked}>
          <option value="">{projects.isLoading ? 'Loading projects…' : destinations.length ? 'Choose a project' : 'No project you can add tasks to'}</option>
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
      // A different task, a new version of it, or another project: a new review.
      key={`${task.id}:${task.version}:${target}`}
      onBack={() => setTarget(null)}
      onClose={onClose}
      opener={opener}
      projectId={projectId}
      targetProjectId={target}
      task={task}
    />
  )
}

function useMoved(projectId: string, task: MovingTask, onClose: () => void) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  return (done: MoveDone, targetProjectId: string) => {
    // Both projects' lists, boards and task pages, the original's included.
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(targetProjectId) })
    const first = done.moved[0]
    showToast({
      tone: 'success',
      title: first ? `Moved ${first.from} to ${first.to}` : `${task.key} was already moved`,
      description: done.leftBehind ? `${countLabel(done.leftBehind, 'field value')} stayed on the original.` : undefined,
    })
    onClose()
    if (first) navigate(workItemHref(targetProjectId, first.id))
  }
}

function MoveRecovery({
  move,
  onClose,
  onStartOver,
  opener,
  projectId,
  task,
}: {
  move: PendingMove
  onClose: () => void
  opener: RefObject<HTMLElement | null>
  onStartOver: () => void
  projectId: string
  task: MovingTask
}) {
  const moved = useMoved(projectId, task, onClose)
  const [settled, setSettled] = useState<string | null>(null)
  const check = useMutation({
    mutationFn: () => moveTask(projectId, task.id, move.body, move.key),
    onSuccess: (done) => {
      settleMove(projectId, task.id)
      moved(done, move.targetProjectId)
    },
    onError: (error) => {
      // A clear refusal means nothing moved: it's settled, and a new move can start.
      if (!unknownOutcome(error)) {
        setSettled(isApiError(error) ? error.message : describeError(error).message)
      }
    },
  })
  if (settled) {
    return (
      <Dialog
        footer={
          <>
            <Button onClick={onClose} variant="ghost">
              Close
            </Button>
            <Button
              onClick={() => {
                settleMove(projectId, task.id)
                onStartOver()
              }}
              variant="primary"
            >
              Start over
            </Button>
          </>
        }
        onClose={onClose}
        open
        title={`${task.key} didn’t move`}
        returnFocus={opener}
      >
        <Banner title={settled} tone="warning" />
      </Dialog>
    )
  }
  return (
    <Dialog
      footer={
        <>
          <Button disabled={check.isPending} onClick={onClose} variant="ghost">
            Close
          </Button>
          <Button busy={check.isPending} onClick={() => check.mutate()} variant="primary">
            Check again
          </Button>
        </>
      }
      onClose={() => {
        if (!check.isPending) onClose()
      }}
      open
      title={`Did ${task.key} move to ${move.targetProjectKey}?`}
      returnFocus={opener}
    >
      <p className="aw-move__lede">
        The answer to that move never arrived, so it may or may not have happened. Checking sends the same move again: if it
        happened, you get its result, and it can’t happen twice. Until then, nothing else can be sent for {task.key}.
      </p>
      {check.isError ? <Banner title={describeError(check.error).message} tone="danger" /> : null}
    </Dialog>
  )
}

function MoveReview({
  onBack,
  onClose,
  opener,
  projectId,
  targetProjectId,
  task,
}: {
  onBack: () => void
  onClose: () => void
  opener: RefObject<HTMLElement | null>
  projectId: string
  targetProjectId: string
  task: MovingTask
}) {
  const queryClient = useQueryClient()
  const moved = useMoved(projectId, task, onClose)
  const previewKey = ['task-move-preview', projectId, task.id, task.version, targetProjectId] as const
  const preview = useQuery({
    queryKey: previewKey,
    queryFn: () => previewMove(projectId, task.id, targetProjectId),
    // A preview is private and goes stale fast: never reuse an old one, and
    // replace it only when asked.
    gcTime: 0,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  })
  const targetFields = useFields({ projectId: targetProjectId })
  // Choices belong to the preview they were made in; another preview starts over.
  const [choices, setChoices] = useState<{ snapshot: string; byIssue: Record<string, Choice> }>({ snapshot: '', byIssue: {} })
  const [stale, setStale] = useState(false)

  // Only a finished, successful preview can be moved on.
  const plan = preview.isSuccess && !preview.isFetching ? preview.data : undefined
  const chosen = plan && choices.snapshot === plan.snapshot ? choices.byIssue : {}
  const issues = plan?.issues ?? []
  const choose = (issue: string, choice: Choice | null) =>
    plan &&
    setChoices((current) => {
      const byIssue = { ...(current.snapshot === plan.snapshot ? current.byIssue : {}) }
      if (choice) byIssue[issue] = choice
      else delete byIssue[issue]
      return { snapshot: plan.snapshot, byIssue }
    })
  // Settled: left, or put in a field with a value (its own, when it fits).
  const settled = (issue: MoveIssue) => {
    const choice = chosen[issue.id]
    if (!choice) return false
    if (choice.kind === 'leave' || choice.value !== undefined) return true
    return Boolean(issue.candidates?.find((candidate) => candidate.id === choice.targetField)?.takesValue)
  }
  const unresolved = issues.filter((issue) => !settled(issue)).length

  const move = useMutation({
    mutationFn: async (current: MovePlan) => {
      const body = {
        targetProjectId,
        snapshot: current.snapshot,
        leaveUnresolved: false,
        resolutions: (current.issues ?? []).flatMap((issue): MoveResolution[] => {
          const choice = chosen[issue.id]
          if (!choice) return []
          if (choice.kind === 'leave') return [{ issue: issue.id, action: 'leave' }]
          return [{ issue: issue.id, action: 'set', targetField: choice.targetField, ...(choice.value === undefined ? {} : { value: choice.value }) }]
        }),
      }
      const key = crypto.randomUUID()
      try {
        return await moveTask(projectId, task.id, body, key)
      } catch (error) {
        // No answer: keep this exact request until it's settled.
        if (unknownOutcome(error)) rememberMove(projectId, task.id, { targetProjectId, targetProjectKey: current.targetProjectKey, body, key })
        throw error
      }
    },
    onSuccess: (done) => moved(done, targetProjectId),
    onError: (error) => {
      if (isApiError(error) && error.status === 409) setStale(true)
    },
  })

  const refresh = () => {
    setStale(false)
    move.reset()
    // Drop the old plan before asking again, so nothing is sent against it.
    void queryClient.resetQueries({ queryKey: previewKey, exact: true })
  }

  const leaveRest = () => {
    for (const issue of issues) if (!settled(issue)) choose(issue.id, { kind: 'leave' })
  }

  const moveError = move.error && !unknownOutcome(move.error) ? (isApiError(move.error) ? move.error.message : describeError(move.error).message) : null
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
          <Button disabled={!plan || unresolved > 0 || move.isPending || stale} onClick={() => plan && move.mutate(plan)} variant="primary">
            {move.isPending ? 'Moving…' : plan && plan.tasks.length > 1 ? `Move ${plan.tasks.length} tasks` : 'Move task'}
          </Button>
        </>
      }
      onClose={() => {
        if (!move.isPending) onClose()
      }}
      open
      title={plan ? `Move ${task.key} to ${plan.targetProjectKey}` : `Move ${task.key}`}
      returnFocus={opener}
      width={640}
    >
      {preview.isFetching ? <p className="aw-move__lede">Checking what carries over…</p> : null}
      {previewError && !preview.isFetching ? <Banner onRetry={refresh} tone="danger" title={previewError} /> : null}
      {plan ? (
        <div className="aw-move">
          <p className="aw-move__lede">
            {plan.tasks.length > 1 ? `${task.key} and ${countLabel(plan.tasks.length - 1, 'subtask')} move.` : `${task.key} moves.`} The original is archived
            with its full history and every field value. Only you see this preview.
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
                    choice={chosen[issue.id]}
                    disabled={move.isPending}
                    issue={issue}
                    key={issue.id}
                    onChoose={(choice) => choose(issue.id, choice)}
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
  disabled,
  issue,
  onChoose,
  targetFields,
  targetProjectId,
}: {
  choice: Choice | undefined
  disabled: boolean
  issue: MoveIssue
  onChoose: (choice: Choice | null) => void
  targetFields: readonly FieldDefinition[]
  targetProjectId: string
}) {
  const [editing, setEditing] = useState<HTMLElement | null>(null)
  const latest = useRef<{ value: FieldValue | null; label: string | null } | null>(null)
  const candidates = issue.candidates ?? []
  const selected = choice?.kind === 'set' ? candidates.find((candidate) => candidate.id === choice.targetField) : undefined
  const definition = useMemo(() => targetFields.find((field) => field.id === selected?.id), [targetFields, selected])
  const where = choice ? (choice.kind === 'leave' ? 'leave' : `set:${choice.targetField}`) : ''

  const finish = () => {
    setEditing(null)
    const picked = latest.current
    latest.current = null
    if (selected && picked && picked.value !== null && !(Array.isArray(picked.value) && picked.value.length === 0)) {
      onChoose({ kind: 'set', targetField: selected.id, value: picked.value, label: picked.label })
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
          disabled={disabled}
          onChange={(event) => {
            const next = event.target.value
            if (!next) return onChoose(null)
            if (next === 'leave') return onChoose({ kind: 'leave' })
            const candidate = candidates.find((entry) => `set:${entry.id}` === next)
            // A field the value fits as it is keeps it; otherwise a value is chosen next.
            if (candidate) onChoose({ kind: 'set', targetField: candidate.id })
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
              <span>{chosenText(definition, choice.value, choice.label)}</span>
            ) : selected.takesValue ? (
              <span>Same value</span>
            ) : (
              <span className="aw-move__needed">Choose a value</span>
            )}
            {definition ? (
              <Button disabled={disabled} onClick={(event) => setEditing(event.currentTarget)} size="sm" variant="ghost">
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
          onLabel={(label) => {
            latest.current = { value: latest.current?.value ?? null, label }
          }}
          onSet={(value) => {
            latest.current = { value, label: latest.current?.label ?? null }
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

/** A chosen target value: option names for a choice, a picked record by its picker's label, otherwise as typed. */
function chosenText(field: FieldDefinition | undefined, value: FieldValue, label: string | null | undefined): string {
  if (label) return label
  if (!field) return shown(value)
  const ids = Array.isArray(value) ? value : [String(value)]
  if (field.type === 'single_choice' || field.type === 'multiple_choice') {
    return ids.map((id) => field.options.find((option) => option.id === id)?.name ?? id).join(', ')
  }
  return shown(value)
}
