import { Fragment, useMemo, useState } from 'react'
import { useInfiniteQuery, useQuery } from '@tanstack/react-query'

import {
  Avatar,
  Button,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  SelectField,
  StatusChip,
  TextField,
  dayKey,
  dayLabel,
  formatDateTime,
  formatTime,
} from '../../ui'
import { OTHER_CHANGE, actionDetail, actionPhrase, targetWords } from '../auditActions'
import type { SettingsPageProps } from '../types'
import { getAuditFacets, listAuditEvents, workspaceKeys, type AuditEvent, type AuditFilters } from './api'

function appLabel(app: AuditEvent['app']) {
  if (app.clientName) return app.clientName
  switch (app.channel) {
    case 'browser':
      return 'Browser'
    case 'mcp':
      return 'Connected app'
    case 'host_cli':
      return 'Server command line'
    case 'system':
      return 'System'
    default:
      return null
  }
}

/** The server names some filters its own way; these read the way the app does. */
const APP_FILTER_LABELS: Record<string, string> = { mcp: 'Connected apps', host_cli: 'Server command line' }
const KIND_FILTER_LABELS: Record<string, string> = {
  work_item: 'Tasks',
  work_item_label: 'Labels',
  external_agent: 'Agents',
  external_execution: 'Run requests',
  agent_profile: 'Agent profiles',
  project_template: 'Project templates',
  task_template: 'Task templates',
  recurring_task: 'Recurring tasks',
  saved_view: 'Saved views',
  webhook: 'Webhooks',
  harness: 'Harnesses',
  automation: 'Automation rules',
  environment: 'Environments',
}

/** A date input's day as the start (or end) of that day here, in RFC 3339. */
function dayBoundary(day: string, end: boolean) {
  if (!day) return undefined
  const [year, month, date] = day.split('-').map(Number)
  const local = end ? new Date(year, month - 1, date, 23, 59, 59, 999) : new Date(year, month - 1, date)
  return Number.isNaN(local.getTime()) ? undefined : local.toISOString()
}

/** Workspace › Audit log: who changed what, through which app, and when. */
export default function AuditLogPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const [actorId, setActorId] = useState('')
  const [app, setApp] = useState('')
  const [kind, setKind] = useState('')
  const [projectId, setProjectId] = useState('')
  const [since, setSince] = useState('')
  const [until, setUntil] = useState('')
  const filters = useMemo<AuditFilters>(
    () => ({
      actorId: actorId || undefined,
      app: app || undefined,
      kind: kind || undefined,
      projectId: projectId || undefined,
      since: dayBoundary(since, false),
      until: dayBoundary(until, true),
    }),
    [actorId, app, kind, projectId, since, until],
  )
  const facets = useQuery({
    queryKey: workspaceKeys.auditFacets(organizationId),
    queryFn: () => getAuditFacets(organizationId),
    enabled: Boolean(organizationId),
  })
  const events = useInfiniteQuery({
    queryKey: workspaceKeys.audit(organizationId, filters),
    queryFn: ({ pageParam }) => listAuditEvents(organizationId, filters, pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    enabled: Boolean(organizationId),
  })
  const filtered = Boolean(actorId || app || kind || projectId || since || until)
  const all = useMemo(() => events.data?.pages.flatMap((page) => page.events) ?? [], [events.data])
  const days = useMemo(() => {
    const groups: { key: string; label: string; events: AuditEvent[] }[] = []
    for (const event of all) {
      const key = dayKey(event.occurredAt)
      const last = groups[groups.length - 1]
      if (last?.key === key) last.events.push(event)
      else groups.push({ key, label: dayLabel(event.occurredAt), events: [event] })
    }
    return groups
  }, [all])
  if (!workspace) return null

  const clear = () => {
    setActorId('')
    setApp('')
    setKind('')
    setProjectId('')
    setSince('')
    setUntil('')
  }

  return (
    <>
      <PageHeader icon="clock" title="Audit log">
        What people, agents and apps changed in {workspace.name}, and through which app.
      </PageHeader>
      <div aria-label="Filter the audit log" className="aw-audit__filters" role="group">
        <SelectField id="audit-actor" label="Who" onChange={(event) => setActorId(event.target.value)} value={actorId}>
          <option value="">Anyone</option>
          {facets.data?.actors.map((actor) => (
            <option key={actor.actorId} value={actor.actorId}>
              {actor.displayName}
              {actor.kind === 'external_agent' ? ' (agent)' : ''}
            </option>
          ))}
        </SelectField>
        <SelectField id="audit-app" label="App" onChange={(event) => setApp(event.target.value)} value={app}>
          <option value="">Any app</option>
          {facets.data?.apps.map((option) => (
            <option key={option.value} value={option.value}>
              {APP_FILTER_LABELS[option.value] ?? option.label}
            </option>
          ))}
        </SelectField>
        <SelectField id="audit-kind" label="What" onChange={(event) => setKind(event.target.value)} value={kind}>
          <option value="">Everything</option>
          {facets.data?.kinds.map((option) => (
            <option key={option.value} value={option.value}>
              {KIND_FILTER_LABELS[option.value] ?? option.label}
            </option>
          ))}
        </SelectField>
        <SelectField id="audit-project" label="Project" onChange={(event) => setProjectId(event.target.value)} value={projectId}>
          <option value="">Any project</option>
          {facets.data?.projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </SelectField>
        <TextField id="audit-since" label="From" max={until || undefined} onChange={(event) => setSince(event.target.value)} type="date" value={since} />
        <TextField id="audit-until" label="To" min={since || undefined} onChange={(event) => setUntil(event.target.value)} type="date" value={until} />
        {filtered && (
          <Button className="aw-audit__clear" onClick={clear} size="sm" variant="ghost">
            Clear filters
          </Button>
        )}
      </div>

      {events.isPending ? (
        <ListSkeleton avatars label="Loading the audit log" rows={6} />
      ) : events.isError ? (
        <ErrorState error={events.error} onRetry={() => void events.refetch()} title="The audit log didn’t load" />
      ) : all.length === 0 ? (
        <EmptyState icon="activity" title={filtered ? 'Nothing matches these filters' : 'Nothing recorded yet'}>
          {filtered ? 'Try a wider date range or fewer filters.' : 'Changes appear here as people and agents make them.'}
        </EmptyState>
      ) : (
        <div className="aw-audit">
          {days.map((day) => (
            <Fragment key={day.key}>
              <h2 className="aw-audit__day">{day.label}</h2>
              <ul aria-label={day.label} className="aw-rows aw-audit__list">
                {day.events.map((event) => (
                  <AuditRow event={event} key={event.id} />
                ))}
              </ul>
            </Fragment>
          ))}
          {events.hasNextPage && (
            <div className="aw-audit__more">
              <Button busy={events.isFetchingNextPage} onClick={() => void events.fetchNextPage()}>
                Load more
              </Button>
            </div>
          )}
        </div>
      )}
    </>
  )
}

function AuditRow({ event }: { event: AuditEvent }) {
  const [open, setOpen] = useState(false)
  const phrase = actionPhrase(event.action)
  const detail = actionDetail(event.action, event.metadata)
  const via = appLabel(event.app)
  const who = event.actor?.displayName ?? 'The server'
  const hasMetadata = Object.keys(event.metadata ?? {}).length > 0
  return (
    <li className="aw-row aw-audit__row">
      <span className="aw-row__lead">
        {event.actor ? (
          <Avatar
            kind={event.actor.kind === 'external_agent' ? 'agent' : 'person'}
            name={event.actor.displayName}
            seed={event.actor.actorId}
            size={28}
          />
        ) : (
          <span className="aw-row__tile">
            <Icon name="settings" size={14} />
          </span>
        )}
      </span>
      <div className="aw-row__main">
        <div className="aw-row__title">
          <strong>{who}</strong> {phrase ?? <span title={event.action}>{OTHER_CHANGE}</span>}
          {detail && (
            <>
              {' '}
              <code className="aw-inline-code">{detail}</code>
            </>
          )}
        </div>
        <div className="aw-row__meta aw-row__metaline aw-wrap">
          <time dateTime={event.occurredAt} title={formatDateTime(event.occurredAt)}>
            {formatTime(event.occurredAt)}
          </time>
          {via && <span>via {via}</span>}
          {event.project && <span>{event.project.name}</span>}
          {event.target.kind && <span>{targetWords(event.target.kind)}</span>}
        </div>
        {open && hasMetadata && <pre className="aw-code aw-audit__metadata">{JSON.stringify(event.metadata, null, 2)}</pre>}
      </div>
      <div className="aw-row__actions">
        {event.outcome !== 'succeeded' && (
          <StatusChip icon="triangle-alert" tone={event.outcome === 'denied' ? 'warning' : 'danger'}>
            {event.outcome === 'denied' ? 'Denied' : 'Failed'}
          </StatusChip>
        )}
        {hasMetadata && (
          <Button aria-expanded={open} onClick={() => setOpen((value) => !value)} size="sm" variant="ghost">
            {open ? 'Hide details' : 'Details'}
          </Button>
        )}
      </div>
    </li>
  )
}
