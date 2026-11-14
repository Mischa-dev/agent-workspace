import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { listPages, memoryKeys } from '../memory/api'
import {
  Banner,
  Button,
  ButtonLink,
  Dots,
  EmptyState,
  ErrorState,
  IconButton,
  ListSkeleton,
  SegmentedControl,
  SelectField,
  StatusChip,
  TextArea,
  TextField,
  countLabel,
  formatRelative,
} from '../ui'
import { priorityLabel, workItemPriorities } from '../work-items/constants'
import type { WorkItemPriority } from '../work-items/types'
import type { Draft, DraftTask, PlanImport, PlanSource } from './api'
import { PlanningFrame, PlanningIntro } from './frame'
import { useCommitPlan, useCreatePlan, usePlan, usePlans, useProjectAi, useSavePlan } from './hooks'
import { aiErrorMessage, planningHref } from './links'
import '../styles/work.css'

type SourceKind = PlanSource['kind']

const STATE_LABELS: Record<PlanImport['state'], string> = {
  drafting: 'Reading',
  draft: 'Draft',
  committing: 'Creating tasks',
  committed: 'Done',
  failed: 'Failed',
}

/** A plan (pasted text, a page, or JSON) turned into milestones, tasks and dependencies after a review. */
export function PlanImportPage() {
  return (
    <PlanningFrame label="Loading plan import" tab="plan-import">
      {(project) => <PlanImportBody archived={Boolean(project.archivedAt)} projectId={project.id} />}
    </PlanningFrame>
  )
}

function PlanImportBody({ archived, projectId }: { archived: boolean; projectId: string }) {
  const [params] = useSearchParams()
  const importId = params.get('plan')
  return importId ? <PlanReview importId={importId} projectId={projectId} /> : <PlanStart archived={archived} projectId={projectId} />
}

function PlanStart({ archived, projectId }: { archived: boolean; projectId: string }) {
  const navigate = useNavigate()
  const status = useProjectAi(projectId)
  const plans = usePlans(projectId)
  const create = useCreatePlan(projectId)
  const [kind, setKind] = useState<SourceKind>('text')
  const [title, setTitle] = useState('')
  const [text, setText] = useState('')
  const [pageId, setPageId] = useState('')
  const [json, setJson] = useState<{ name: string; value: unknown } | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const pages = useQuery({ queryKey: memoryKeys.pages(projectId), queryFn: () => listPages(projectId), enabled: kind === 'page' })
  const canEdit = plans.data?.canEdit ?? false
  const needsAi = kind !== 'structured'
  const aiReady = status.data?.configured ?? false

  const readFile = async (file: File | undefined) => {
    setProblem(null)
    setJson(null)
    if (!file) return
    try {
      setJson({ name: file.name, value: JSON.parse(await file.text()) })
      if (!title.trim()) setTitle(file.name.replace(/\.json$/i, ''))
    } catch {
      setProblem('That file isn’t valid JSON.')
    }
  }
  const source: PlanSource | null =
    kind === 'text' ? (text.trim() ? { kind, text } : null) : kind === 'page' ? (pageId ? { kind, pageId } : null) : json ? { kind, plan: json.value } : null
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!source || !title.trim() || create.isPending) return
    setProblem(null)
    create.mutate(
      { title: title.trim(), source },
      {
        onSuccess: ({ plan }) => navigate(`${planningHref(projectId, 'plan-import')}?plan=${encodeURIComponent(plan.id)}`),
        onError: (error) => setProblem(aiErrorMessage(error, 'Reading the plan')),
      },
    )
  }

  return (
    <>
      <PlanningIntro>
        Turn a plan into milestones, tasks and the dependencies between them. You review and edit everything before any task is created.
      </PlanningIntro>
      {plans.isSuccess && canEdit && !archived ? (
        <form className="aw-ai-planform" onSubmit={submit}>
          <SegmentedControl<SourceKind>
            label="Where the plan comes from"
            onChange={setKind}
            options={[
              { value: 'text', label: 'Paste text' },
              { value: 'page', label: 'A project page' },
              { value: 'structured', label: 'JSON file' },
            ]}
            value={kind}
          />
          <TextField label="Name" maxLength={200} onChange={(event) => setTitle(event.target.value)} required value={title} />
          {kind === 'text' ? (
            <TextArea
              hint="A roadmap, a spec, meeting notes: anything with the work spelled out."
              label="Plan"
              maxLength={200_000}
              onChange={(event) => setText(event.target.value)}
              rows={10}
              value={text}
            />
          ) : kind === 'page' ? (
            <SelectField label="Page" onChange={(event) => setPageId(event.target.value)} value={pageId}>
              <option value="">{pages.isPending ? 'Loading pages…' : 'Choose a page'}</option>
              {(pages.data?.pages ?? []).map((page) => (
                <option key={page.id} value={page.id}>
                  {page.title}
                </option>
              ))}
            </SelectField>
          ) : (
            <div className="aw-field">
              <span className="aw-field__label" id="plan-file-label">
                File
              </span>
              <div className="aw-ai-file">
                <Button aria-describedby="plan-file-hint" aria-labelledby="plan-file-label plan-file-button" icon="plus" id="plan-file-button" onClick={() => fileInput.current?.click()} type="button">
                  {json ? 'Choose another file' : 'Choose a JSON file'}
                </Button>
                {json ? <span className="aw-ai-file__name">{json.name}</span> : null}
              </div>
              <input
                accept="application/json,.json"
                aria-hidden="true"
                className="visually-hidden"
                onChange={(event) => {
                  void readFile(event.target.files?.[0])
                  event.target.value = ''
                }}
                ref={fileInput}
                tabIndex={-1}
                type="file"
              />
              <span className="aw-field__note" id="plan-file-hint">
                A roadmap in JSON, or a draft with milestones and tasks. No model needed.
              </span>
            </div>
          )}
          {needsAi && status.isSuccess && !aiReady ? (
            <Banner role="note" tone="info">
              Reading text needs a model, and none is set up for this project. {status.data.problem ?? ''} A JSON file works without one.
            </Banner>
          ) : null}
          {problem ? <Banner tone="danger">{problem}</Banner> : null}
          <div className="aw-ai-planform__actions">
            <Button busy={create.isPending} disabled={!source || !title.trim() || (needsAi && !aiReady)} type="submit" variant="primary">
              {needsAi ? 'Read the plan' : 'Review the plan'}
            </Button>
          </div>
        </form>
      ) : plans.isSuccess ? (
        <Banner role="note" tone="info">
          {archived ? 'This project is archived.' : 'Only people who can create tasks here can import a plan.'}
        </Banner>
      ) : null}
      <h2 className="aw-worksection__title">Earlier imports</h2>
      {plans.isPending ? <ListSkeleton label="Loading imports" rows={2} /> : null}
      {plans.isError ? <ErrorState error={plans.error} onRetry={() => void plans.refetch()} title="Imports didn’t load" /> : null}
      {plans.isSuccess && !plans.data.plans.length ? (
        <EmptyState icon="list" title="No imports yet">
          Imports you start show up here, so you can come back to a draft.
        </EmptyState>
      ) : null}
      {plans.data?.plans.length ? (
        <ul className="aw-ai-plans">
          {plans.data.plans.map((plan) => (
            <li key={plan.id}>
              <Link to={`${planningHref(projectId, 'plan-import')}?plan=${encodeURIComponent(plan.id)}`}>{plan.title}</Link>
              <StatusChip tone={plan.state === 'committed' ? 'success' : plan.state === 'failed' ? 'danger' : 'muted'}>{STATE_LABELS[plan.state]}</StatusChip>
              <span className="aw-quiet">
                {countLabel(plan.draft.tasks.length, 'task')} · {formatRelative(plan.createdAt)}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </>
  )
}

function PlanReview({ importId, projectId }: { importId: string; projectId: string }) {
  const queryClient = useQueryClient()
  const loaded = usePlan(projectId, importId)
  const state = loaded.data?.plan.state
  const previous = useRef(state)
  // Tasks appear once the import finishes; refresh the task views then.
  useEffect(() => {
    if (previous.current && previous.current !== 'committed' && state === 'committed') {
      void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    }
    previous.current = state
  }, [projectId, queryClient, state])

  const back = (
    <Link className="aw-ai-back" to={planningHref(projectId, 'plan-import')}>
      All imports
    </Link>
  )
  if (loaded.isPending) return <ListSkeleton label="Loading the plan" rows={4} />
  if (loaded.isError) {
    return <ErrorState action={<ButtonLink to={planningHref(projectId, 'plan-import')}>All imports</ButtonLink>} error={loaded.error} title="This plan didn’t load" />
  }
  const { plan, canEdit } = loaded.data
  if (plan.state === 'drafting') {
    return (
      <>
        <PlanningIntro title={plan.title}>{back}</PlanningIntro>
        <p aria-live="polite" className="aw-ai-run aw-ai-run--running">
          <Dots /> Reading the plan and drafting milestones and tasks. This page updates when it’s done.
        </p>
      </>
    )
  }
  if (plan.state === 'failed') {
    return (
      <>
        <PlanningIntro title={plan.title}>{back}</PlanningIntro>
        <Banner title="This import stopped." tone="danger">
          {plan.error ?? 'Something went wrong.'}{' '}
          {plan.createdTasks ? `${countLabel(plan.createdTasks, 'task was', 'tasks were')} created before it stopped.` : ''}
        </Banner>
      </>
    )
  }
  if (plan.state === 'committing' || plan.state === 'committed') {
    const total = plan.draft.tasks.length
    const done = plan.state === 'committed'
    return (
      <>
        <PlanningIntro title={plan.title}>{back}</PlanningIntro>
        <div aria-live="polite" className="aw-ai-progress">
          {done ? (
            <Banner icon="circle-check" title="Your plan is now tasks." tone="success">
              Created {countLabel(plan.createdMilestones, 'milestone')}, {countLabel(plan.createdTasks, 'task')} and{' '}
              {countLabel(plan.createdDependencies, 'dependency', 'dependencies')}.
            </Banner>
          ) : (
            <p className="aw-ai-run aw-ai-run--running">
              <Dots /> Creating tasks: {plan.createdTasks} of {total}. You can leave this page; you’ll get a notification when it’s done.
            </p>
          )}
          <progress aria-label="Tasks created" className="aw-progress__bar" max={Math.max(1, total)} value={plan.createdTasks} />
        </div>
        {done ? (
          <div className="aw-ai-planform__actions">
            <ButtonLink to={`/projects/${encodeURIComponent(projectId)}/work-items`} variant="primary">
              Open tasks
            </ButtonLink>
            <ButtonLink to={planningHref(projectId, 'graph')}>See the graph</ButtonLink>
          </div>
        ) : null}
      </>
    )
  }
  return <DraftEditor canEdit={canEdit} key={`${plan.id}:${plan.version}`} plan={plan} projectId={projectId} />
}

function DraftEditor({ canEdit, plan, projectId }: { canEdit: boolean; plan: PlanImport; projectId: string }) {
  const [draft, setDraft] = useState<Draft>(plan.draft)
  const [title, setTitle] = useState(plan.title)
  const [problem, setProblem] = useState<string | null>(null)
  const save = useSavePlan(projectId)
  const commit = useCommitPlan(projectId)
  const dirty = draft !== plan.draft || title !== plan.title
  const titles = new Map(draft.tasks.map((task) => [task.ref, task.title]))

  const updateTask = (ref: string, change: Partial<DraftTask>) =>
    setDraft((current) => ({ ...current, tasks: current.tasks.map((task) => (task.ref === ref ? { ...task, ...change } : task)) }))
  const removeTask = (ref: string) =>
    setDraft((current) => ({
      ...current,
      tasks: current.tasks
        .filter((task) => task.ref !== ref)
        .map((task) => ({ ...task, parent: task.parent === ref ? null : task.parent, blockedBy: task.blockedBy.filter((other) => other !== ref) })),
    }))
  const removeMilestone = (ref: string) =>
    setDraft((current) => ({
      milestones: current.milestones.filter((milestone) => milestone.ref !== ref),
      tasks: current.tasks.map((task) => (task.milestone === ref ? { ...task, milestone: null } : task)),
    }))
  const renameMilestone = (ref: string, name: string) =>
    setDraft((current) => ({ ...current, milestones: current.milestones.map((milestone) => (milestone.ref === ref ? { ...milestone, name } : milestone)) }))

  const onSave = (then?: (saved: PlanImport) => void) => {
    setProblem(null)
    save.mutate(
      { plan, draft, title: title.trim() || undefined },
      { onSuccess: ({ plan: saved }) => then?.(saved), onError: (error) => setProblem(aiErrorMessage(error, 'Saving the draft')) },
    )
  }
  const onCommit = () => {
    setProblem(null)
    const go = (current: PlanImport) => commit.mutate(current, { onError: (error) => setProblem(aiErrorMessage(error, 'Creating the tasks')) })
    if (dirty) onSave(go)
    else go(plan)
  }
  const readOnly = !canEdit
  // Unsaved edits to the draft live only on this page; the browser asks before closing it.
  useEffect(() => {
    if (!dirty) return undefined
    const warn = (event: BeforeUnloadEvent) => event.preventDefault()
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [dirty])

  return (
    <>
      <PlanningIntro
        actions={
          readOnly ? null : (
            <>
              <Button busy={save.isPending && !commit.isPending} disabled={!dirty} onClick={() => onSave()}>
                Save draft
              </Button>
              <Button busy={commit.isPending} disabled={!draft.tasks.length} onClick={onCommit} variant="primary">
                Create {countLabel(draft.tasks.length, 'task')}
              </Button>
            </>
          )
        }
        title={plan.title}
      >
        Review the draft: rename, reprioritize or remove anything before it becomes tasks.{' '}
        <Link to={planningHref(projectId, 'plan-import')}>All imports</Link>
      </PlanningIntro>
      {problem ? (
        <Banner className="aw-page__banner" tone="danger">
          {problem}
        </Banner>
      ) : null}
      {readOnly ? null : <TextField label="Import name" maxLength={200} onChange={(event) => setTitle(event.target.value)} value={title} />}
      <section aria-labelledby="plan-milestones" className="aw-ai-draft">
        <h2 className="aw-worksection__title" id="plan-milestones">
          Milestones ({draft.milestones.length})
        </h2>
        {draft.milestones.length ? (
          <ul className="aw-ai-draft__milestones">
            {draft.milestones.map((milestone) => (
              <li key={milestone.ref}>
                <TextField
                  aria-label={`Milestone ${milestone.ref}`}
                  disabled={readOnly}
                  maxLength={120}
                  onChange={(event) => renameMilestone(milestone.ref, event.target.value)}
                  value={milestone.name}
                />
                <span className="aw-quiet">{countLabel(draft.tasks.filter((task) => task.milestone === milestone.ref).length, 'task')}</span>
                {readOnly ? null : <IconButton icon="trash" label={`Remove ${milestone.name}`} onClick={() => removeMilestone(milestone.ref)} size="sm" />}
              </li>
            ))}
          </ul>
        ) : (
          <p className="aw-quiet">No milestones in this plan.</p>
        )}
      </section>
      <section aria-labelledby="plan-tasks" className="aw-ai-draft">
        <h2 className="aw-worksection__title" id="plan-tasks">
          Tasks ({draft.tasks.length})
        </h2>
        <ol className="aw-ai-draft__tasks">
          {draft.tasks.map((task) => (
            <li key={task.ref}>
              <div className="aw-ai-draft__row">
                <TextField
                  aria-label={`Title of ${task.ref}`}
                  disabled={readOnly}
                  maxLength={200}
                  onChange={(event) => updateTask(task.ref, { title: event.target.value })}
                  value={task.title}
                />
                <SelectField aria-label={`Priority of ${task.ref}`} disabled={readOnly} onChange={(event) => updateTask(task.ref, { priority: event.target.value })} value={task.priority}>
                  {workItemPriorities.map((priority) => (
                    <option key={priority.value} value={priority.value}>
                      {priorityLabel(priority.value as WorkItemPriority)}
                    </option>
                  ))}
                </SelectField>
                {draft.milestones.length ? (
                  <SelectField
                    aria-label={`Milestone of ${task.ref}`}
                    disabled={readOnly}
                    onChange={(event) => updateTask(task.ref, { milestone: event.target.value || null })}
                    value={task.milestone ?? ''}
                  >
                    <option value="">No milestone</option>
                    {draft.milestones.map((milestone) => (
                      <option key={milestone.ref} value={milestone.ref}>
                        {milestone.name}
                      </option>
                    ))}
                  </SelectField>
                ) : null}
                {readOnly ? null : <IconButton icon="trash" label={`Remove ${task.title}`} onClick={() => removeTask(task.ref)} size="sm" />}
              </div>
              <div className="aw-ai-draft__meta">
                {task.parent ? <span>Subtask of {titles.get(task.parent) ?? task.parent}</span> : null}
                {task.blockedBy.length ? <span>Blocked by {task.blockedBy.map((ref) => titles.get(ref) ?? ref).join(', ')}</span> : null}
                {task.labels.length ? <span>{task.labels.join(', ')}</span> : null}
                {task.criteria.length ? <span>{countLabel(task.criteria.length, 'criterion', 'criteria')}</span> : null}
              </div>
              {task.description ? (
                <details className="aw-ai-draft__details">
                  <summary>Description</summary>
                  <TextArea
                    aria-label={`Description of ${task.ref}`}
                    disabled={readOnly}
                    maxLength={20_000}
                    onChange={(event) => updateTask(task.ref, { description: event.target.value })}
                    rows={4}
                    value={task.description}
                  />
                </details>
              ) : null}
            </li>
          ))}
        </ol>
      </section>
    </>
  )
}
