import { useState, type KeyboardEvent } from 'react'
import { useInfiniteQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { useMilestoneList } from '../work-items/planningHooks'
import { Banner, Button, ButtonLink, Dots, EmptyState, ErrorState, Icon, Kbd, ListSkeleton, SegmentedControl, SelectField, StatusChip, countLabel, formatRelative } from '../ui'
import { priorityLabel } from '../work-items/constants'
import { workItemsHref } from '../work-items/links'
import type { WorkItemPriority } from '../work-items/types'
import { aiKeys, listSuggestions, type AiRun, type ProjectAi, type RankScope, type SuggestionStatus } from './api'
import { PlanningFrame, PlanningIntro } from './frame'
import { useAnalyze, useDecideSuggestion, useProjectAi, useRanking } from './hooks'
import { AiNotConfigured, ReadinessChecks, SharedReadiness, SuggestionCard } from './parts'
import { sharedMissingChecks } from './readiness'
import { aiErrorMessage, planningHref } from './links'

const PAGE = 50

/** Moves focus between the rows of a list with the arrow keys (and j / k). */
function moveFocus(event: KeyboardEvent<HTMLElement>, selector: string) {
  const step = event.key === 'ArrowDown' || event.key === 'j' ? 1 : event.key === 'ArrowUp' || event.key === 'k' ? -1 : 0
  if (!step || event.altKey || event.metaKey || event.ctrlKey) return false
  const rows = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(selector))
  const current = rows.findIndex((row) => row.contains(document.activeElement))
  const next = rows[Math.min(rows.length - 1, Math.max(0, current + step))]
  if (!next) return false
  event.preventDefault()
  next.focus()
  return true
}

function AnalyzeButton({ analyze, status }: { analyze: ReturnType<typeof useAnalyze>; status: ProjectAi | undefined }) {
  if (!status?.canEdit || !status.configured) return null
  const running = status.runs.some((run) => run.mine && run.kind === 'analyze' && (run.state === 'queued' || run.state === 'running'))
  return (
    <Button busy={analyze.isPending || running} icon="bot" onClick={() => analyze.mutate()} variant="primary">
      {running ? 'Analyzing' : 'Analyze project'}
    </Button>
  )
}

/** Why Analyze didn't start, where everyone can see it. */
function AnalyzeProblem({ analyze }: { analyze: ReturnType<typeof useAnalyze> }) {
  if (!analyze.isError) return null
  return (
    <Banner className="aw-page__banner" onRetry={() => analyze.mutate()} title="The analysis didn’t start." tone="danger">
      {aiErrorMessage(analyze.error, 'Analyzing')}
    </Banner>
  )
}

function runSentence(run: AiRun) {
  if (run.state === 'queued' || run.state === 'running') return 'Looking for dependencies, blockers and missing details…'
  if (run.state === 'failed') return run.error ?? 'The last run failed.'
  const found = run.suggestionsCreated
  const applied = run.suggestionsApplied ? `, ${run.suggestionsApplied} applied automatically` : ''
  return `Checked ${countLabel(run.tasksConsidered, 'task')} ${formatRelative(run.finishedAt ?? run.createdAt)}: ${found ? countLabel(found, 'suggestion') : 'nothing new'}${applied}.`
}

function LatestRun({ status }: { status: ProjectAi | undefined }) {
  const run = status?.runs[0]
  if (!run) return null
  const busy = run.state === 'queued' || run.state === 'running'
  return (
    <p aria-live="polite" className={`aw-ai-run aw-ai-run--${run.state}`}>
      {busy ? <Dots /> : <Icon name={run.state === 'failed' ? 'circle-alert' : 'circle-check'} size={14} />}
      {runSentence(run)}
    </p>
  )
}

// ---------------------------------------------------------------------------
// Up next

const SCOPES: { value: RankScope; label: string }[] = [
  { value: 'ready', label: 'Ready' },
  { value: 'agents', label: 'For agents' },
  { value: 'all', label: 'All open' },
]

/** The open tasks that can start now, best first, and why. */
export function NextPage() {
  return <PlanningFrame label="Loading what’s next" tab="next">{(project) => <NextBody projectId={project.id} />}</PlanningFrame>
}

function NextBody({ projectId }: { projectId: string }) {
  const [scope, setScope] = useState<RankScope>('ready')
  const [milestoneId, setMilestoneId] = useState('')
  const [limit, setLimit] = useState(PAGE)
  const status = useProjectAi(projectId)
  const analyze = useAnalyze(projectId)
  const milestones = useMilestoneList(projectId)
  const ranking = useRanking(projectId, { scope, milestoneId: milestoneId || null, limit })
  const tasks = ranking.data?.tasks ?? []
  const showsReadiness = (task: (typeof tasks)[number]) => scope === 'all' || !task.readiness.readyForAgents
  // What every row would repeat ("no repository to work in") is said once, above the list.
  const shared = sharedMissingChecks(tasks.filter(showsReadiness).map((task) => task.readiness))
  const sharedIds = new Set(shared.map((check) => check.id))

  return (
    <>
      <PlanningIntro actions={<AnalyzeButton analyze={analyze} status={status.data} />}>
        Open tasks whose blockers are done, ranked by priority, how much they unblock and how soon they’re due.
      </PlanningIntro>
      <AnalyzeProblem analyze={analyze} />
      {status.data ? <AiNotConfigured projectId={projectId} status={status.data} /> : null}
      <div className="aw-ai-toolbar">
        <SegmentedControl<RankScope> label="Which tasks" onChange={(value) => (setScope(value), setLimit(PAGE))} options={SCOPES} value={scope} />
        {milestones.data?.milestones.length ? (
          <SelectField aria-label="Milestone" className="aw-ai-toolbar__select" compact onChange={(event) => (setMilestoneId(event.target.value), setLimit(PAGE))} value={milestoneId}>
            <option value="">All milestones</option>
            {milestones.data.milestones.map((milestone) => (
              <option key={milestone.id} value={milestone.id}>
                {milestone.name}
              </option>
            ))}
          </SelectField>
        ) : null}
      </div>
      {ranking.isPending ? <ListSkeleton label="Ranking tasks" rows={5} /> : null}
      {ranking.isError ? <ErrorState error={ranking.error} onRetry={() => void ranking.refetch()} title="The ranking didn’t load" /> : null}
      {ranking.isSuccess && !tasks.length ? (
        <EmptyState
          action={
            scope === 'ready' ? (
              <ButtonLink to={planningHref(projectId, 'graph')}>Open the graph</ButtonLink>
            ) : scope === 'all' ? (
              <ButtonLink to={`${workItemsHref(projectId)}?create=task`}>New task</ButtonLink>
            ) : (
              <ButtonLink to={workItemsHref(projectId)}>Open the task list</ButtonLink>
            )
          }
          icon="circle-check"
          title={scope === 'all' ? 'No open tasks' : 'Nothing is ready to start'}
        >
          {scope === 'agents'
            ? 'Tasks become ready for agents once they have acceptance criteria, a description and a repository.'
            : scope === 'ready'
              ? 'Every open task is waiting on something. The graph shows what.'
              : 'Create tasks or import a plan to get started.'}
        </EmptyState>
      ) : null}
      {tasks.length ? <SharedReadiness checks={shared} /> : null}
      {tasks.length ? (
        <ol aria-label="Tasks, best first" className="aw-ai-ranking" onKeyDown={(event) => moveFocus(event, '.aw-ai-rank__title')}>
          {tasks.map((task, index) => (
            <li className="aw-ai-rank" key={task.id}>
              <span aria-hidden="true" className="aw-ai-rank__place">
                {index + 1}
              </span>
              <div className="aw-ai-rank__main">
                <Link className="aw-ai-rank__title" to={task.href}>
                  <span className="aw-ai-task__key">{task.key}</span> {task.title}
                </Link>
                <div className="aw-ai-rank__meta">
                  <StatusChip>{task.statusName}</StatusChip>
                  {/* The reasons already say "High priority" when priority helped its rank. */}
                  {task.priority !== 'no_priority' && !task.reasons.some((reason) => /priority/i.test(reason)) ? (
                    <span className={`aw-ai-priority aw-ai-priority--${task.priority}`}>{priorityLabel(task.priority as WorkItemPriority)}</span>
                  ) : null}
                  {task.milestoneName ? <span>{task.milestoneName}</span> : null}
                  {task.assigneeName ? <span>{task.assigneeName}</span> : null}
                  {task.claimed ? <StatusChip icon="lock" tone="agent">Claimed</StatusChip> : null}
                </div>
                {task.reasons.length ? <p className="aw-ai-rank__why">{task.reasons.join(' · ')}</p> : null}
                {showsReadiness(task) ? <ReadinessChecks compact omit={sharedIds} readiness={task.readiness} /> : null}
              </div>
            </li>
          ))}
        </ol>
      ) : null}
      {ranking.data?.nextOffset != null ? (
        <Button busy={ranking.isFetching} className="aw-ai-more" onClick={() => setLimit((current) => current + PAGE)}>
          Show more ({ranking.data.total - tasks.length} more)
        </Button>
      ) : null}
    </>
  )
}

// ---------------------------------------------------------------------------
// Suggestions

type Shown = SuggestionStatus | 'all'

const SHOWN: { value: Shown; label: string }[] = [
  { value: 'pending', label: 'To review' },
  { value: 'accepted', label: 'Accepted' },
  { value: 'applied', label: 'Auto-applied' },
  { value: 'dismissed', label: 'Dismissed' },
]

/** What the model found, to accept or dismiss. A for accept, D for dismiss. */
export function SuggestionsPage() {
  return <PlanningFrame label="Loading suggestions" tab="suggestions">{(project) => <SuggestionsBody projectId={project.id} />}</PlanningFrame>
}

function SuggestionsBody({ projectId }: { projectId: string }) {
  const [shown, setShown] = useState<Shown>('pending')
  const status = useProjectAi(projectId)
  const analyze = useAnalyze(projectId)
  const decide = useDecideSuggestion(projectId)
  const [problem, setProblem] = useState<string | null>(null)
  const page = useInfiniteQuery({
    queryKey: aiKeys.suggestions(projectId, { status: shown, infinite: true }),
    queryFn: ({ pageParam }) => listSuggestions(projectId, { status: shown, cursor: pageParam, limit: PAGE }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    enabled: Boolean(projectId),
  })
  const suggestions = page.data?.pages.flatMap((entry) => entry.suggestions) ?? []
  const canEdit = page.data?.pages[0]?.canEdit ?? false

  const onDecide = (suggestionId: string, decision: 'accept' | 'dismiss') => {
    setProblem(null)
    decide.mutate({ suggestionId, decision }, { onError: (error) => setProblem(aiErrorMessage(error, decision === 'accept' ? 'Accepting' : 'Dismissing')) })
  }
  const onKeyDown = (event: KeyboardEvent<HTMLUListElement>) => {
    if (moveFocus(event, '.aw-ai-suggestion')) return
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-suggestion-id]')
    if (!row || event.target !== row || !canEdit || decide.isPending || shown !== 'pending') return
    const decision = event.key === 'a' ? 'accept' : event.key === 'd' ? 'dismiss' : null
    if (!decision) return
    event.preventDefault()
    // Keep the place in the list: focus moves to the next suggestion.
    const next = (row.nextElementSibling ?? row.previousElementSibling) as HTMLElement | null
    onDecide(row.dataset.suggestionId ?? '', decision)
    next?.focus()
  }

  return (
    <>
      <PlanningIntro actions={<AnalyzeButton analyze={analyze} status={status.data} />}>
        Dependencies, blockers, labels, priorities and criteria the model found. Nothing changes until someone accepts one, unless the
        project applies the most confident ones automatically.
      </PlanningIntro>
      <AnalyzeProblem analyze={analyze} />
      {status.data ? <AiNotConfigured projectId={projectId} status={status.data} /> : null}
      <LatestRun status={status.data} />
      <div className="aw-ai-toolbar">
        <SegmentedControl<Shown> label="Which suggestions" onChange={setShown} options={SHOWN} value={shown} />
        {shown === 'pending' && canEdit && suggestions.length ? (
          <span className="aw-ai-toolbar__hint">
            <Kbd>A</Kbd> accept · <Kbd>D</Kbd> dismiss · <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> move
          </span>
        ) : null}
      </div>
      {problem ? (
        <Banner className="aw-page__banner" tone="danger">
          {problem}
        </Banner>
      ) : null}
      {page.isPending ? <ListSkeleton label="Loading suggestions" rows={4} /> : null}
      {page.isError ? <ErrorState error={page.error} onRetry={() => void page.refetch()} title="Suggestions didn’t load" /> : null}
      {page.isSuccess && !suggestions.length ? (
        <EmptyState icon="circle-check" title={shown === 'pending' ? 'Nothing to review' : 'None here yet'}>
          {shown === 'pending'
            ? status.data?.configured
              ? 'New suggestions show up here as tasks change, after each nightly check, or when someone analyzes the project.'
              : 'Once a model is set up, it looks for dependencies and blockers as tasks change.'
            : 'Suggestions move here once someone decides on them.'}
        </EmptyState>
      ) : null}
      {suggestions.length ? (
        <ul aria-label="Suggestions" className="aw-ai-suggestions" onKeyDown={onKeyDown}>
          {suggestions.map((suggestion) => (
            <SuggestionCard
              busy={decide.isPending && decide.variables?.suggestionId === suggestion.id}
              canEdit={canEdit}
              key={suggestion.id}
              onDecide={(decision) => onDecide(suggestion.id, decision)}
              suggestion={suggestion}
            />
          ))}
        </ul>
      ) : null}
      {page.hasNextPage ? (
        <Button busy={page.isFetchingNextPage} className="aw-ai-more" onClick={() => void page.fetchNextPage()}>
          Show more
        </Button>
      ) : null}
      {shown === 'pending' && suggestions.length ? (
        <p className="aw-quiet">
          The <Link to={planningHref(projectId, 'graph')}>graph</Link> shows suggested dependencies as dashed lines.
        </p>
      ) : null}
    </>
  )
}
