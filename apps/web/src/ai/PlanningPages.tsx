import { useState, type KeyboardEvent } from 'react'
import { useInfiniteQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { useMilestoneList } from '../work-items/planningHooks'
import { Banner, Button, Dots, EmptyState, ErrorState, Icon, Kbd, ListSkeleton, PageHeader, SegmentedControl, SelectField, StatusChip, formatRelative } from '../ui'
import { aiKeys, listSuggestions, type AiRun, type ProjectAi, type RankScope, type SuggestionStatus } from './api'
import { PlanningFrame } from './frame'
import { useAnalyze, useDecideSuggestion, useProjectAi, useRanking } from './hooks'
import { AiNotConfigured, ReadinessChecks, SuggestionCard } from './parts'
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

function AnalyzeButton({ projectId, status }: { projectId: string; status: ProjectAi | undefined }) {
  const analyze = useAnalyze(projectId)
  if (!status?.canEdit || !status.configured) return null
  const running = status.runs.some((run) => run.mine && run.kind === 'analyze' && (run.state === 'queued' || run.state === 'running'))
  return (
    <>
      <Button busy={analyze.isPending || running} icon="bot" onClick={() => analyze.mutate()} variant="primary">
        {running ? 'Analyzing' : 'Analyze project'}
      </Button>
      {analyze.isError ? <span className="visually-hidden" role="alert">{aiErrorMessage(analyze.error, 'Analyzing')}</span> : null}
    </>
  )
}

function runSentence(run: AiRun) {
  if (run.state === 'queued' || run.state === 'running') return 'Looking for dependencies, blockers and missing details…'
  if (run.state === 'failed') return run.error ?? 'The last run failed.'
  const found = run.suggestionsCreated
  const applied = run.suggestionsApplied ? `, ${run.suggestionsApplied} applied automatically` : ''
  return `Checked ${run.tasksConsidered} ${run.tasksConsidered === 1 ? 'task' : 'tasks'} ${formatRelative(run.finishedAt ?? run.createdAt)}: ${found ? `${found} ${found === 1 ? 'suggestion' : 'suggestions'}` : 'nothing new'}${applied}.`
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

/** AI5: the open tasks that can start now, best first, and why. */
export function NextPage() {
  return <PlanningFrame label="Loading what’s next" tab="next">{(project) => <NextBody projectId={project.id} />}</PlanningFrame>
}

function NextBody({ projectId }: { projectId: string }) {
  const [scope, setScope] = useState<RankScope>('ready')
  const [milestoneId, setMilestoneId] = useState('')
  const [limit, setLimit] = useState(PAGE)
  const status = useProjectAi(projectId)
  const milestones = useMilestoneList(projectId)
  const ranking = useRanking(projectId, { scope, milestoneId: milestoneId || null, limit })
  const tasks = ranking.data?.tasks ?? []

  return (
    <>
      <PageHeader actions={<AnalyzeButton projectId={projectId} status={status.data} />} icon="play" title="Up next">
        Open tasks whose blockers are done, ranked by priority, how much they unblock and how soon they’re due.
      </PageHeader>
      {status.data ? <AiNotConfigured projectId={projectId} status={status.data} /> : null}
      <div className="aw-ai-toolbar">
        <SegmentedControl<RankScope> label="Which tasks" onChange={(value) => (setScope(value), setLimit(PAGE))} options={SCOPES} value={scope} />
        {milestones.data?.milestones.length ? (
          <SelectField className="aw-ai-toolbar__select" label="Milestone" onChange={(event) => (setMilestoneId(event.target.value), setLimit(PAGE))} value={milestoneId}>
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
      {ranking.isError ? (
        <ErrorState action={<Button onClick={() => void ranking.refetch()}>Try again</Button>} title="The ranking didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {ranking.isSuccess && !tasks.length ? (
        <EmptyState icon="circle-check" title={scope === 'all' ? 'No open tasks' : 'Nothing is ready to start'}>
          {scope === 'agents'
            ? 'Tasks become ready for agents once they have acceptance criteria, a description and a repository.'
            : scope === 'ready'
              ? 'Every open task is waiting on something. The graph shows what.'
              : 'Create tasks or import a plan to get started.'}
        </EmptyState>
      ) : null}
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
                  {task.priority !== 'no_priority' ? <span className={`aw-ai-priority aw-ai-priority--${task.priority}`}>{task.priority}</span> : null}
                  {task.milestoneName ? <span>{task.milestoneName}</span> : null}
                  {task.assigneeName ? <span>{task.assigneeName}</span> : null}
                  {task.claimed ? <StatusChip icon="bot" tone="agent">Claimed</StatusChip> : null}
                </div>
                {task.reasons.length ? <p className="aw-ai-rank__why">{task.reasons.join(' · ')}</p> : null}
                {scope === 'all' || !task.readiness.readyForAgents ? <ReadinessChecks compact readiness={task.readiness} /> : null}
              </div>
            </li>
          ))}
        </ol>
      ) : null}
      {ranking.data?.nextOffset != null ? (
        <Button busy={ranking.isFetching} className="aw-ai-more" onClick={() => setLimit((current) => current + PAGE)}>
          Show more ({ranking.data.total - tasks.length} left)
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
  { value: 'applied', label: 'Applied' },
  { value: 'dismissed', label: 'Dismissed' },
]

/** AI2 and AI4: what the AI found, to accept or dismiss. A for accept, D for dismiss. */
export function SuggestionsPage() {
  return <PlanningFrame label="Loading suggestions" tab="suggestions">{(project) => <SuggestionsBody projectId={project.id} />}</PlanningFrame>
}

function SuggestionsBody({ projectId }: { projectId: string }) {
  const [shown, setShown] = useState<Shown>('pending')
  const status = useProjectAi(projectId)
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
      <PageHeader actions={<AnalyzeButton projectId={projectId} status={status.data} />} icon="bot" title="Suggestions">
        Dependencies, blockers, labels, priorities and criteria the AI found. Nothing changes until someone accepts it, unless the
        project turned on applying sure ones automatically.
      </PageHeader>
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
      {page.isError ? (
        <ErrorState action={<Button onClick={() => void page.refetch()}>Try again</Button>} title="Suggestions didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {page.isSuccess && !suggestions.length ? (
        <EmptyState icon="circle-check" title={shown === 'pending' ? 'Nothing to review' : 'None here yet'}>
          {shown === 'pending'
            ? status.data?.configured
              ? 'New suggestions show up here as tasks change, after each nightly check, or when someone analyzes the project.'
              : 'Once AI is set up, it looks for dependencies and blockers as tasks change.'
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
