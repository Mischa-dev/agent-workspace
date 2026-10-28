import { useInfiniteQuery } from '@tanstack/react-query'
import { Link, useParams, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useHomeExperience, useProjectExperience } from '../experience/layout'
import { LaunchQueue } from '../launch/LaunchQueue'
import { launchesHref } from '../launch/launchModel'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject, useProjects } from '../projects/hooks'
import { RememberPlace } from '../projects/RememberPlace'
import type { Project } from '../projects/types'
import { AppShell } from '../shell/AppShell'
import {
  Button,
  ButtonLink,
  EmptyState,
  ErrorState,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  SegmentedControl,
  SelectField,
  StatusChip,
  Switch,
  formatFullDate,
  formatRelative,
} from '../ui'
import { listRuns, runKeys, type RunListFilter, type RunStatusFilter, type RunSummary } from './api'
import { RunUsage } from './RunUsage'
import { formatCost, formatTokens, isLiveRun, runStatus, runTitle, totalTokens } from './runModel'
import './runs.css'

const TABS: { value: RunStatusFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Active' },
  { value: 'needs_you', label: 'Needs you' },
  { value: 'finished', label: 'Done' },
  { value: 'failed', label: 'Failed' },
]

const EMPTY: Record<RunStatusFilter, string> = {
  all: 'Runs agents do on your tasks show up here.',
  active: 'Nothing is running right now.',
  needs_you: 'No run is waiting on you.',
  finished: 'No run is done yet.',
  failed: 'No run has failed.',
}

/**
 * Runs refresh on their own while any of them is still going; a run that only
 * waits on someone changes rarely, so it's checked less often.
 */
const LIVE_REFRESH_MS = 15_000
const WAITING_REFRESH_MS = 60_000

function refreshInterval(runs: RunSummary[]) {
  if (runs.some((run) => isLiveRun(run.state) && run.state !== 'blocked')) return LIVE_REFRESH_MS
  return runs.some((run) => run.state === 'blocked') ? WAITING_REFRESH_MS : false
}

function statusParam(value: string | null): RunStatusFilter {
  return TABS.some((tab) => tab.value === value) ? (value as RunStatusFilter) : 'all'
}

/** A project's Runs: the one Runs page, kept to that project and inside it. */
export function ProjectRunsPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading runs" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }
  return <RunsPage project={project.data} />
}

/**
 * Every agent run you can see, newest first: across your projects from Home,
 * or one project's from inside it. With a project, what waits to start comes
 * first.
 */
export function RunsPage({ project }: { project?: Project } = {}) {
  const session = useAuthenticatedSession()
  const [params, setParams] = useSearchParams()
  const status = statusParam(params.get('status'))
  const mine = params.get('mine') === '1'
  const projectId = project?.id ?? params.get('project') ?? undefined
  const filter: RunListFilter = { status, mine, projectId }
  // Tokens and cost follow the project's layout when one is picked, else the workspace's.
  const projectExperience = useProjectExperience(projectId)
  const homeExperience = useHomeExperience()
  const usage = (projectId ? projectExperience : homeExperience).shows('tech.run_usage')

  const runs = useInfiniteQuery({
    queryKey: runKeys.list(filter),
    queryFn: ({ pageParam }) => listRuns(filter, pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    refetchInterval: (query) => refreshInterval(query.state.data?.pages.flatMap((page) => page.runs) ?? []),
  })
  const list = runs.data?.pages.flatMap((page) => page.runs) ?? []
  const projects = useProjects()
  const choices = (projects.data?.projects ?? []).filter((project) => !project.archivedAt)
  // Where a new run starts: the chosen project, or the first one you're in.
  const launchProject = projectId ?? choices[0]?.id

  const update = (key: string, value: string | null) => {
    const next = new URLSearchParams(params)
    if (value) next.set(key, value)
    else next.delete(key)
    setParams(next, { replace: true })
  }

  return (
    <AppShell home={!project} mainClassName="aw-main--page" project={project} user={session.user}>
      <div className="aw-page aw-runs">
        <PageHeader icon="activity" title="Runs">
          {project
            ? `Agents working on tasks in ${project.name}: what waits to start, what ran, what it used and what it cost.`
            : 'What agents did on your tasks, what they used and what it cost.'}
        </PageHeader>
        {project ? <RememberPlace location={{ kind: 'runs' }} project={project} /> : null}
        {projectId ? <LaunchQueue projectId={projectId} /> : null}
        <div className="aw-runs__filters">
          <SegmentedControl label="Show runs" onChange={(value) => update('status', value === 'all' ? null : value)} options={TABS} value={status} />
          {!project && choices.length > 1 ? (
            <SelectField aria-label="Project" compact onChange={(event) => update('project', event.target.value || null)} value={projectId ?? ''}>
              <option value="">All projects</option>
              {choices.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </SelectField>
          ) : null}
          <Switch checked={mine} label="Only runs I started" onChange={(checked) => update('mine', checked ? '1' : null)} />
        </div>
        {runs.isPending ? <ListSkeleton label="Loading runs" rows={6} /> : null}
        {runs.isError ? <ErrorState error={runs.error} onRetry={() => void runs.refetch()} title="Runs didn’t load" /> : null}
        {runs.data && list.length === 0 ? (
          <EmptyState
            action={
              status === 'all' && !mine && launchProject ? (
                <ButtonLink icon="play" to={launchesHref(launchProject)} variant="primary">
                  Launch agents on ready tasks
                </ButtonLink>
              ) : status !== 'all' || mine ? (
                <Button onClick={() => setParams(projectId ? { project: projectId } : {}, { replace: true })}>Show all runs</Button>
              ) : null
            }
            icon="activity"
            title="No runs here"
          >
            {EMPTY[status]}
          </EmptyState>
        ) : null}
        {list.length > 0 ? (
          <RowList label="Runs">
            {list.map((run) => (
              <RunRow key={run.id} run={run} usage={usage} />
            ))}
          </RowList>
        ) : null}
        {runs.hasNextPage ? (
          <div className="aw-runs__more">
            <Button busy={runs.isFetchingNextPage} onClick={() => void runs.fetchNextPage()}>
              Show more runs
            </Button>
          </div>
        ) : null}
        {usage ? <RunUsage projectId={projectId} /> : null}
      </div>
    </AppShell>
  )
}

function RunRow({ run, usage }: { run: RunSummary; usage: boolean }) {
  const status = runStatus(run.state)
  const cost = usage ? formatCost(run.cost, run.currency) : null
  const tokens = usage ? totalTokens(run) : 0
  return (
    <Row
      actions={<StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>{status.label}</StatusChip>}
      className="aw-runs__row"
      meta={
        <>
          <span>{run.projectName}</span>
          {run.harnessName ? <span>{run.harnessName}</span> : null}
          {run.agentName ? <span>{run.agentName}</span> : null}
          {tokens > 0 ? <span>{formatTokens(tokens)} tokens</span> : usage && isLiveRun(run.state) ? <span>No tokens yet</span> : null}
          {cost ? <span>{cost}</span> : null}
          <time dateTime={run.createdAt} title={formatFullDate(run.createdAt)}>
            {formatRelative(run.createdAt)}
          </time>
        </>
      }
      title={
        <Link className="aw-runs__link" to={`/runs/${run.id}`}>
          {runTitle(run)}
        </Link>
      }
    />
  )
}
