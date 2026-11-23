import { here } from '../app/href'
import { useWorkspaceId } from '../app/workspace'
import { useInfiniteQuery } from '@tanstack/react-query'
import { Link, useParams, useSearchParams } from 'react-router-dom'

import { useHomeExperience, useProjectExperience } from '../experience/layout'
import { LaunchQueue } from '../launch/LaunchQueue'
import { launchesHref } from '../launch/launchModel'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject, useWorkspaceProjects } from '../projects/hooks'
import { RememberPlace } from '../projects/RememberPlace'
import type { Project } from '../projects/types'
import { useShellPage } from '../shell/shellStore'
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
import { listRuns, runKeys, type RunLine, type RunListFilter, type RunStatusFilter } from './api'
import { RunUsage } from './RunUsage'
import { causeLabel, costText, formatTokens, hostLabel, isLiveRun, livenessLabel, runStatus, runTitle, totalTokens, WAITING_STATES } from './runModel'
import './runs.css'

const TABS: { value: RunStatusFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Active' },
  { value: 'needs_you', label: 'Needs you' },
  { value: 'not_responding', label: 'Not responding' },
  { value: 'finished', label: 'Done' },
  { value: 'failed', label: 'Failed' },
]

const EMPTY: Record<RunStatusFilter, string> = {
  all: 'Runs agents do on your tasks show up here.',
  active: 'Nothing is running right now.',
  needs_you: 'No run is waiting on you.',
  not_responding: 'Every running run is still reporting.',
  finished: 'No run is done yet.',
  failed: 'No run has failed.',
}

/**
 * Runs refresh on their own while any of them is still going; a run that only
 * waits on someone changes rarely, so it's checked less often.
 */
const LIVE_REFRESH_MS = 15_000
const WAITING_REFRESH_MS = 60_000

function refreshInterval(runs: RunLine[]) {
  const waiting = (run: RunLine) => WAITING_STATES.includes(run.state)
  if (runs.some((run) => isLiveRun(run.state) && !waiting(run))) return LIVE_REFRESH_MS
  return runs.some(waiting) ? WAITING_REFRESH_MS : false
}

/** The agents or hosts the loaded runs name, for a filter, by name. */
function choicesFrom(runs: RunLine[], pick: (run: RunLine) => [string | null, string | null]) {
  const seen = new Map<string, string>()
  for (const run of runs) {
    const [id, name] = pick(run)
    if (id && !seen.has(id)) seen.set(id, name ?? id)
  }
  return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1]))
}

function statusParam(value: string | null): RunStatusFilter {
  return TABS.some((tab) => tab.value === value) ? (value as RunStatusFilter) : 'all'
}

/** A project's Runs: the one Runs page, kept to that project and inside it. */
export function ProjectRunsPage() {
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  if (project.isPending) {
    return (
      <ProjectPageLoading label="Loading runs" />
    )
  }
  if (project.isError) {
    return (
      <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
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
  const [params, setParams] = useSearchParams()
  const status = statusParam(params.get('status'))
  const mine = params.get('mine') === '1'
  const projectId = project?.id ?? params.get('project') ?? undefined
  const agentId = params.get('agent') ?? undefined
  const hostId = params.get('host') ?? undefined
  // Every run in the workspace the person is in, unless a project names its own.
  const workspaceId = useWorkspaceId() ?? undefined
  const filter: RunListFilter = { workspaceId: projectId ? undefined : workspaceId, status, mine, projectId, agentId, hostId }
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
  // The agent and host pickers offer what the runs on screen name, and keep
  // the one picked even when nothing of it is left on screen.
  const agents = choicesFrom(list, (run) => [run.agentActorId, run.agentName])
  const hosts = choicesFrom(list, (run) => [run.hostId, run.hostName])
  if (agentId && !agents.some(([id]) => id === agentId)) agents.push([agentId, 'The chosen agent'])
  if (hostId && !hosts.some(([id]) => id === hostId)) hosts.push([hostId, 'The chosen host'])
  const { projects: choices } = useWorkspaceProjects()
  // Where a new run starts: the chosen project, or the first one you're in.
  const launchProject = projectId ?? choices[0]?.id

  useShellPage({ home: !project, mainClassName: 'aw-main--page', project })

  const update = (key: string, value: string | null) => {
    const next = new URLSearchParams(params)
    if (value) next.set(key, value)
    else next.delete(key)
    setParams(next, { replace: true })
  }

  return (
    <>
      <div className="aw-page aw-runs">
        <PageHeader icon="activity" title="Runs" />
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
          {agents.length > 1 || agentId ? (
            <SelectField aria-label="Agent" compact onChange={(event) => update('agent', event.target.value || null)} value={agentId ?? ''}>
              <option value="">Every agent</option>
              {agents.map(([id, name]) => (
                <option key={id} value={id}>
                  {name}
                </option>
              ))}
            </SelectField>
          ) : null}
          {hosts.length > 1 || hostId ? (
            <SelectField aria-label="Host" compact onChange={(event) => update('host', event.target.value || null)} value={hostId ?? ''}>
              <option value="">Every host</option>
              {hosts.map(([id, name]) => (
                <option key={id} value={id}>
                  {name}
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
              status === 'all' && !mine && !agentId && !hostId && launchProject ? (
                <ButtonLink icon="play" to={launchesHref(launchProject)} variant="primary">
                  Launch agents on ready tasks
                </ButtonLink>
              ) : status !== 'all' || mine || agentId || hostId ? (
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
    </>
  )
}

function RunRow({ run, usage }: { run: RunLine; usage: boolean }) {
  const status = runStatus(run.state)
  const quiet = livenessLabel(run)
  const cost = usage ? costText(run.cost, run.currency, run.costConfidence) : null
  const tokens = usage ? totalTokens(run) : 0
  return (
    <Row
      actions={
        <span className="aw-runs__chips">
          {quiet ? <StatusChip tone={quiet.tone}>{quiet.label}</StatusChip> : null}
          <StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>
            {status.label}
          </StatusChip>
        </span>
      }
      className="aw-runs__row"
      meta={
        <>
          {run.projectName ? <span>{run.projectName}</span> : null}
          <span>{causeLabel(run)}</span>
          <span>{run.agentName}</span>
          <span>{hostLabel(run)}</span>
          {run.waitingOnTitle ? <span className="aw-runs__waiting">Waiting on: {run.waitingOnTitle}</span> : null}
          {!run.waitingOnTitle && run.statusLine && isLiveRun(run.state) ? <span>{run.statusLine}</span> : null}
          {tokens > 0 ? <span>{formatTokens(tokens)} tokens</span> : null}
          {cost ? <span title={cost.hint ?? undefined}>{cost.text}</span> : null}
          <time dateTime={run.createdAt} title={formatFullDate(run.createdAt)}>
            {formatRelative(run.createdAt)}
          </time>
        </>
      }
      title={
        <Link className="aw-runs__link" to={here(`/runs/${encodeURIComponent(run.id)}`)}>
          {runTitle(run)}
        </Link>
      }
    />
  )
}
