import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type RefObject } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useAuthenticatedSession } from '../auth/session'
import { useFields } from '../fields/api'
import type { FieldDefinition, FieldValue } from '../fields/fieldModel'
import { Banner, Button, Dialog, SelectField, countLabel, describeError, showToast } from '../ui'
import { useWorkItem } from './hooks'
import { listMoveDestinations, movedTo, previewMove, type MoveDone, type MoveIssue, type MovePlan, type MoveResolution } from './moveApi'
import { pendingMove, sendMove, settleMove, subscribeMoves, type PendingMove } from './moveRecovery'
import { FieldEditor } from './TaskCustomFields'
import './moveTask.css'

/** What the mover chose for one issue; `label` is how a picked person, task or page reads. */
type Choice = { kind: 'leave' } | { kind: 'set'; targetField: string; value?: FieldValue; label?: string | null }

type MovingTask = { id: string; key: string; title: string; version: number }

type Refusal = { status: number; message: string }

/**
 * Moving a task, with its subtasks, to another project (AWD-102): pick the
 * project, read a private preview of what carries over and what needs a
 * choice, settle each issue (put it in a target field, or leave it on the
 * original), then confirm once. Nothing moves until then.
 *
 * A confirmation is recorded and sent outside this dialog (moveRecovery),
 * which only shows where it stands: being sent, its result, a refusal, or an
 * answer that never came, when the only thing it offers is to send that same
 * request again.
 */
export default function MoveTaskDialog({ onClose, projectId, task }: { onClose: () => void; projectId: string; task: MovingTask }) {
  // Every step is its own dialog: focus goes back to whatever opened the first.
  const [opener] = useState<RefObject<HTMLElement | null>>(() => ({ current: document.activeElement as HTMLElement | null }))
  // A confirmation belongs to whoever sent it.
  const actorId = useAuthenticatedSession().user.actorId
  const pending = useSyncExternalStore(subscribeMoves, () => pendingMove(actorId, projectId, task.id))
  // A move that finished while this was closed is shown, not followed: opening
  // the dialog again doesn't take anyone to another page.
  const [finishedBefore] = useState(() => pendingMove(actorId, projectId, task.id)?.state.kind === 'done')
  const navigate = useNavigate()
  const [target, setTarget] = useState<string | null>(null)
  const [picked, setPicked] = useState('')
  // A first confirmation's clear refusal (nothing moved) stays in the store,
  // and the review shows it, until the person goes back, closes or previews
  // again; a new confirmation replaces it.
  const refusal: Refusal | null =
    pending?.state.kind === 'refused' && !pending.state.retry ? { status: pending.state.status, message: pending.state.message } : null
  const reviewing = target ?? (refusal ? (pending?.targetProjectId ?? null) : null)
  const dismissRefusal = () => {
    if (refusal) settleMove(actorId, projectId, task.id)
  }
  const moved = useMoved(projectId, task, onClose)
  // Worked out by the server from your actual permissions in each project.
  const projects = useQuery({
    queryKey: ['task-move-destinations', actorId, projectId, task.id] as const,
    queryFn: () => listMoveDestinations(projectId, task.id),
    gcTime: 0,
    retry: false,
  })
  const destinations = projects.data?.projects ?? []

  // A result that arrives while this is open is shown once (toast, then the
  // task where it went), then forgotten.
  useEffect(() => {
    if (pending?.state.kind === 'done' && !finishedBefore) {
      const done = pending.state.done
      settleMove(actorId, projectId, task.id)
      moved(done, pending.targetProjectId)
    }
  }, [actorId, finishedBefore, moved, pending, projectId, task.id])

  if (pending?.state.kind === 'done' && finishedBefore) {
    const done = pending.state.done
    const first = done.moved[0]
    // The returned link: where the task really is, maybe not where it was asked to go.
    const where = first ? movedTo(first) : null
    const forget = () => settleMove(actorId, projectId, task.id)
    return (
      <Dialog
        footer={
          <>
            <Button
              onClick={() => {
                forget()
                onClose()
              }}
              variant="ghost"
            >
              Close
            </Button>
            {first && where ? (
              <Button
                onClick={() => {
                  forget()
                  onClose()
                  navigate(where.path)
                }}
                variant="primary"
              >
                Open {first.to}
              </Button>
            ) : null}
          </>
        }
        onClose={() => {
          forget()
          onClose()
        }}
        open
        returnFocus={opener}
        title={!first ? `${task.key} was already moved` : done.created ? `${task.key} moved to ${first.to}` : `${task.key} was already moved to ${first.to}`}
      >
        <p className="aw-move__lede">The answer came after this dialog was closed.</p>
        {done.notes?.map((note) => (
          <p className="aw-move__lede" key={note}>
            {note}
          </p>
        ))}
      </Dialog>
    )
  }

  if (pending && pending.state.kind !== 'done' && !(pending.state.kind === 'refused' && !pending.state.retry)) {
    return (
      <MoveRecovery
        actorId={actorId}
        move={pending}
        onClose={onClose}
        onStartOver={() => {
          settleMove(actorId, projectId, task.id)
          setTarget(null)
        }}
        opener={opener}
        projectId={projectId}
        task={task}
      />
    )
  }
  if (pending && !refusal) return null
  if (!reviewing) {
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
        returnFocus={opener}
        title={`Move ${task.key} to another project`}
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
        {projects.isError ? (
          <Banner
            tone="danger"
            title={isApiError(projects.error) && projects.error.status < 500 ? projects.error.message : describeError(projects.error).message}
          />
        ) : null}
      </Dialog>
    )
  }
  return (
    <MoveReview
      actorId={actorId}
      // A different task, a new version of it, or another project: a new review.
      key={`${task.id}:${task.version}:${reviewing}`}
      onBack={() => {
        dismissRefusal()
        setTarget(null)
      }}
      onClose={() => {
        dismissRefusal()
        onClose()
      }}
      onDismissRefusal={dismissRefusal}
      opener={opener}
      projectId={projectId}
      refusal={refusal}
      targetProjectId={reviewing}
      task={task}
    />
  )
}

/** Reads both projects' tasks again after a move, whether or not the dialog is still open. */
function useInvalidateMoved() {
  const queryClient = useQueryClient()
  const [invalidate] = useState(() => (source: string, target: string) => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(source) })
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(target) })
  })
  return invalidate
}

function useMoved(projectId: string, task: MovingTask, onClose: () => void) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const latest = useRef({ projectId, task, onClose })
  useEffect(() => {
    latest.current = { projectId, task, onClose }
  })
  // Stable, so effects that call it don't run again on every render.
  const [moved] = useState(() => (done: MoveDone, requested: string) => {
    const { projectId: source, task: moving, onClose: close } = latest.current
    const first = done.moved[0]
    // Where it really is: the returned link, which may not be the project
    // asked for when another request had already moved it.
    const where = first ? movedTo(first) : null
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(source) })
    void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(where?.projectId ?? requested) })
    showToast({
      tone: done.created ? 'success' : 'info',
      title: !first ? `${moving.key} was already moved` : done.created ? `Moved ${first.from} to ${first.to}` : `${first.from} was already moved to ${first.to}`,
      description:
        [...(done.notes ?? []), ...(done.leftBehind ? [`${countLabel(done.leftBehind, 'field value')} stayed on the original.`] : [])].join(' ') ||
        undefined,
    })
    close()
    if (where) navigate(where.path)
  })
  return moved
}

function MoveRecovery({
  actorId,
  move,
  onClose,
  onStartOver,
  opener,
  projectId,
  task,
}: {
  actorId: string
  move: PendingMove
  onClose: () => void
  onStartOver: () => void
  opener: RefObject<HTMLElement | null>
  projectId: string
  task: MovingTask
}) {
  // What the task looks like now: a hint only; Check again gets the answer.
  const current = useWorkItem(projectId, task.id)
  const invalidate = useInvalidateMoved()
  const state = move.state
  const sending = state.kind === 'sending'
  if (state.kind === 'refused') {
    return (
      <Dialog
        footer={
          <>
            <Button onClick={onClose} variant="ghost">
              Close
            </Button>
            <Button onClick={onStartOver} variant="primary">
              Start over
            </Button>
          </>
        }
        onClose={onClose}
        open
        returnFocus={opener}
        title="That move request wasn’t carried out"
      >
        <Banner title={state.message} tone="warning" />
      </Dialog>
    )
  }
  if (sending && !state.retry) {
    return (
      <Dialog onClose={onClose} open returnFocus={opener} title={`Moving ${task.key} to ${move.targetProjectKey}…`}>
        <p className="aw-move__lede">This dialog can close; the move carries on, and opening it again shows how it went.</p>
        <Button onClick={onClose} variant="ghost">
          Close
        </Button>
      </Dialog>
    )
  }
  return (
    <Dialog
      footer={
        <>
          <Button disabled={sending} onClick={onClose} variant="ghost">
            Close
          </Button>
          <Button busy={sending} onClick={() =>
              void sendMove(actorId, projectId, task.id, undefined, (done) =>
                invalidate(projectId, (done.moved[0] && movedTo(done.moved[0])?.projectId) ?? move.targetProjectId),
              )
            } variant="primary">
            Check again
          </Button>
        </>
      }
      onClose={() => {
        if (!sending) onClose()
      }}
      open
      returnFocus={opener}
      title={`Did ${task.key} move to ${move.targetProjectKey}?`}
    >
      <p className="aw-move__lede">
        The answer to that move never arrived, or didn’t say whether it happened, so it may or may not have. Check again sends
        that same move: if it happened, you get its result, and it can’t happen twice. Until it’s settled, nothing else can be sent
        for {task.key}.
      </p>
      <p className="aw-move__lede">
        Only this tab remembers it, and only while you stay signed in: closing this and coming back keeps it, but reloading or
        signing out forgets it, and then it can’t be sent again from here. If that happens, open {task.key}: if it moved, it’s
        archived with a link to its new task; if it didn’t, you can start a new move, which can’t move it twice.
      </p>
      {state.kind === 'unknown' ? <Banner title={`Last answer: ${state.message}`} tone="danger" /> : null}
      {current.data?.archivedAt ? (
        <Banner tone="info" title={`${task.key} is archived now, so it most likely moved.`}>
          Check again to see where it went.
        </Banner>
      ) : current.data ? (
        <Banner tone="info" title={`${task.key} is still here and not archived, so it most likely didn’t move.`}>
          Check again to be sure before moving it another way.
        </Banner>
      ) : null}
    </Dialog>
  )
}

function MoveReview({
  actorId,
  onBack,
  onClose,
  onDismissRefusal,
  opener,
  projectId,
  refusal,
  targetProjectId,
  task,
}: {
  actorId: string
  onBack: () => void
  onClose: () => void
  /** Forgets the refusal below, before a fresh preview. */
  onDismissRefusal: () => void
  opener: RefObject<HTMLElement | null>
  projectId: string
  /** The answer to the last confirmation, when it refused: this request wasn't carried out. */
  refusal: Refusal | null
  targetProjectId: string
  task: MovingTask
}) {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateMoved()
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
  const refused = refusal
  const stale = refused?.status === 409

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

  // Recorded, then sent, outside this dialog: see moveRecovery.
  const send = (current: MovePlan) => {
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
    void sendMove(
      actorId,
      projectId,
      task.id,
      { targetProjectId, targetProjectKey: current.targetProjectKey, body, key: crypto.randomUUID() },
      (done) => invalidate(projectId, (done.moved[0] && movedTo(done.moved[0])?.projectId) ?? targetProjectId),
    )
  }

  const refresh = () => {
    onDismissRefusal()
    // Drop the old plan before asking again, so nothing is sent against it.
    void queryClient.resetQueries({ queryKey: previewKey, exact: true })
  }

  const leaveRest = () => {
    for (const issue of issues) if (!settled(issue)) choose(issue.id, { kind: 'leave' })
  }

  const moveError = refused?.message ?? null
  const previewError = preview.error ? (isApiError(preview.error) && preview.error.status !== 404 ? preview.error.message : describeError(preview.error).message) : null

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onBack} variant="ghost">
            Back
          </Button>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button disabled={!plan || unresolved > 0 || stale} onClick={() => plan && send(plan)} variant="primary">
            {plan && plan.tasks.length > 1 ? `Move ${plan.tasks.length} tasks` : 'Move task'}
          </Button>
        </>
      }
      onClose={onClose}
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
                    disabled={false}
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
