import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience } from '../experience/layout'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { RememberPlace } from '../projects/RememberPlace'
import { AppShell } from '../shell/AppShell'
import { WorkTabs } from '../shell/ProjectTabs'
import { Banner, Button, ButtonLink, Checkbox, EmptyState, ErrorState, ListSkeleton, PageHeader, SegmentedControl, SelectField, countLabel, describeError, showToast } from '../ui'
import { updateWorkItem } from '../work-items/api'
import { workItemHref } from '../work-items/links'
import { updateMilestone } from '../work-items/planningApi'
import { getTimeline, viewKeys, type Timeline, type TimelineTask } from './api'
import { addDays, dayFromNumber, dayLabel, dayNumber, today } from './dates'
import { slips, spansWith, targetWith, type PendingChange } from './timelineModel'
import './views.css'

type Zoom = 'weeks' | 'months' | 'quarters'

const ZOOMS: Record<Zoom, { width: number; before: number; after: number }> = {
  weeks: { width: 28, before: 14, after: 90 },
  months: { width: 10, before: 30, after: 180 },
  quarters: { width: 4, before: 60, after: 540 },
}

const ROW_HEIGHT = 34
const HEADER_HEIGHT = 40

type Row =
  | { kind: 'milestone'; id: string; name: string; targetDate: string | null; overdue: boolean; version: number }
  | { kind: 'task'; task: TimelineTask }
  | { kind: 'heading'; id: string; name: string }

function rowsOf(timeline: Timeline): Row[] {
  const rows: Row[] = []
  const byMilestone = new Map<string | null, TimelineTask[]>()
  for (const task of timeline.tasks) {
    const list = byMilestone.get(task.milestoneId) ?? []
    list.push(task)
    byMilestone.set(task.milestoneId, list)
  }
  const milestones = [...timeline.milestones].sort((left, right) => (left.targetDate ?? '9999').localeCompare(right.targetDate ?? '9999'))
  for (const milestone of milestones) {
    const tasks = byMilestone.get(milestone.id) ?? []
    if (!tasks.length && !milestone.targetDate) continue
    rows.push({ kind: 'milestone', id: milestone.id, name: milestone.name, targetDate: milestone.targetDate, overdue: milestone.overdue, version: milestone.version })
    for (const task of tasks) rows.push({ kind: 'task', task })
  }
  const loose = byMilestone.get(null) ?? []
  if (loose.length) {
    rows.push({ kind: 'heading', id: 'none', name: 'No milestone' })
    for (const task of loose) rows.push({ kind: 'task', task })
  }
  return rows
}

/**
 * Milestones and tasks on a time axis from their dates, with what blocks what
 * drawn as arrows. Drag a milestone or a task, or focus it and use the arrow
 * keys, to see what slips before saving.
 */
export function TimelinePage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const showKeys = useProjectExperience(projectId).shows('tech.task_keys')
  const named = (task: TimelineTask) => (showKeys ? `${task.key} ${task.title}` : task.title)
  const [parameters, setParameters] = useSearchParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const zoom: Zoom = parameters.get('zoom') === 'months' ? 'months' : parameters.get('zoom') === 'quarters' ? 'quarters' : 'weeks'
  const milestoneId = parameters.get('milestoneId') ?? ''
  const includeDone = parameters.get('includeDone') !== 'false'
  const scale = ZOOMS[zoom]
  const start = addDays(today(), -scale.before)
  const query: Record<string, string> = {
    from: start,
    to: addDays(today(), scale.after),
    ...(milestoneId ? { milestoneId } : {}),
    ...(includeDone ? {} : { includeDone: 'false' }),
  }
  const timeline = useQuery({ queryKey: viewKeys.timeline(projectId, query), queryFn: () => getTimeline(projectId, query) })
  const [change, setChange] = useState<PendingChange | null>(null)
  const [saving, setSaving] = useState(false)
  const drag = useRef<{ x: number; moved: boolean } | null>(null)
  const scroller = useRef<HTMLDivElement>(null)
  // Open on this week, with a few days before it in view.
  const dataFrom = timeline.data?.from
  useEffect(() => {
    if (!dataFrom || !scroller.current) return
    scroller.current.scrollLeft = Math.max(0, (dayNumber(today()) - dayNumber(dataFrom) - 3) * scale.width)
  }, [dataFrom, scale.width])

  const setParameter = (key: string, value: string | null) =>
    setParameters((previous) => {
      const next = new URLSearchParams(previous)
      if (value === null) next.delete(key)
      else next.set(key, value)
      return next
    }, { replace: true })

  const data = timeline.data
  const rows = useMemo(() => (data ? rowsOf(data) : []), [data])
  const spans = useMemo(() => (data ? spansWith(data.tasks, change) : new Map()), [data, change])
  const slipped = useMemo(() => (data ? slips(data.tasks, data.milestones, data.dependencies, change) : null), [data, change])

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading the timeline" />
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

  const canEdit = Boolean(data?.canEdit)
  // Phones keep more of the chart and less of the names.
  const labelWidth = typeof window !== 'undefined' && window.innerWidth < 640 ? 140 : 260
  const first = data ? dayNumber(data.from) : 0
  const last = data ? dayNumber(data.to) : 0
  const days = last - first + 1
  const x = (day: number) => labelWidth + (day - first) * scale.width
  const todayX = x(dayNumber(today()))
  const rowIndex = new Map<string, number>()
  rows.forEach((row, index) => {
    if (row.kind === 'task') rowIndex.set(row.task.id, index)
  })

  const changing = change && change.days !== 0 ? change : null
  const changedName =
    changing?.kind === 'milestone'
      ? data?.milestones.find((milestone) => milestone.id === changing.id)?.name
      : data?.tasks.find((task) => task.id === changing?.id)?.key
  const changedDay = (() => {
    if (!changing || !data) return null
    if (changing.kind === 'milestone') {
      const milestone = data.milestones.find((entry) => entry.id === changing.id)
      const target = milestone ? targetWith(milestone, changing) : null
      return target === null ? null : dayFromNumber(target)
    }
    const span = spans.get(changing.id)
    return span ? dayFromNumber(changing.edge === 'end' ? span.end : span.start) : null
  })()

  const begin = (event: PointerEvent<HTMLElement>, next: PendingChange) => {
    if (!canEdit || event.button !== 0) return
    event.currentTarget.setPointerCapture(event.pointerId)
    drag.current = { x: event.clientX, moved: false }
    setChange(next)
  }
  const move = (event: PointerEvent<HTMLElement>) => {
    if (!drag.current) return
    const delta = event.clientX - drag.current.x
    if (Math.abs(delta) > 3) drag.current.moved = true
    const shift = Math.round(delta / scale.width)
    setChange((current) => (current ? { ...current, days: shift } : current))
  }
  const end = (open: () => void) => {
    const moved = drag.current?.moved
    drag.current = null
    if (!moved) {
      setChange(null)
      open()
    }
  }
  const keys = (event: KeyboardEvent<HTMLElement>, next: (days: number, wide: boolean) => PendingChange) => {
    if (event.key === 'Enter' && changing) {
      event.preventDefault()
      void save()
    } else if (event.key === 'Escape' && change) {
      event.preventDefault()
      setChange(null)
    } else if ((event.key === 'ArrowLeft' || event.key === 'ArrowRight') && canEdit) {
      event.preventDefault()
      const step = event.key === 'ArrowLeft' ? -1 : 1
      const current = next(0, event.shiftKey)
      const same = change && change.kind === current.kind && change.id === current.id && (change.kind === 'milestone' || (current.kind === 'task' && change.edge === current.edge))
      setChange(next((same ? change.days : 0) + (event.altKey ? step * 7 : step), event.shiftKey))
    }
  }

  const save = async () => {
    if (!changing || !data || saving) return
    setSaving(true)
    try {
      if (changing.kind === 'milestone') {
        const milestone = data.milestones.find((entry) => entry.id === changing.id)
        if (!milestone?.targetDate) return
        await updateMilestone({
          projectId,
          milestoneId: milestone.id,
          expectedVersion: milestone.version,
          targetDate: addDays(milestone.targetDate, changing.days),
          idempotencyKey: crypto.randomUUID(),
        })
      } else {
        const task = data.tasks.find((entry) => entry.id === changing.id)
        if (!task) return
        const patch =
          changing.edge === 'end'
            ? { dueDate: changedDay }
            : {
                startDate: task.startDate ? addDays(task.startDate, changing.days) : null,
                dueDate: task.dueDate ? addDays(task.dueDate, changing.days) : null,
              }
        await updateWorkItem({ projectId, workItemId: task.id, patch: { expectedVersion: task.version, ...patch }, idempotencyKey: crypto.randomUUID() })
      }
      // Wait for the new dates before letting go of the preview, so the bar
      // doesn't jump back to where it was and then forward again.
      await queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
      showToast({ tone: 'success', title: `Moved ${changedName ?? 'it'} to ${changedDay ? dayLabel(changedDay) : 'its new date'}` })
      setChange(null)
    } catch (reason) {
      showToast({
        tone: 'danger',
        title: `${changedName ?? 'It'} wasn’t moved`,
        description:
          isApiError(reason) && reason.code === 'version_conflict'
            ? 'Someone changed it first. The timeline is up to date now, so try again.'
            : describeError(reason).message,
      })
      void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    } finally {
      setSaving(false)
    }
  }

  // Each month spans to the next; its name sticks beside the labels while any of it is in view.
  const months: { label: string; left: number; width: number }[] = []
  const weeks: number[] = []
  for (let day = first; day <= last; day += 1) {
    const iso = dayFromNumber(day)
    if (iso.endsWith('-01') || day === first) {
      const date = new Date(day * 86_400_000)
      const previous = months.at(-1)
      if (previous) previous.width = x(day) - previous.left
      months.push({ label: date.toLocaleDateString([], { month: 'short', year: 'numeric', timeZone: 'UTC' }), left: x(day), width: 0 })
    }
    if (new Date(day * 86_400_000).getUTCDay() === 1) weeks.push(x(day))
  }
  const lastMonth = months.at(-1)
  if (lastMonth) lastMonth.width = x(last + 1) - lastMonth.left

  const slipCount = slipped?.tasks.size ?? 0
  return (
    <AppShell mainClassName="aw-main--page" project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide aw-timeline">
        <PageHeader icon="clock" title="Timeline">
          Milestones and tasks in {project.data.name} by date. Drag one, or focus it and press ← or →, to see what slips.
        </PageHeader>
        <WorkTabs current="timeline" projectId={project.data.id} />
        <RememberPlace location={{ kind: 'timeline' }} project={project.data} />
        <div className="aw-timeline__toolbar">
          <SelectField aria-label="Milestone" compact onChange={(event) => setParameter('milestoneId', event.target.value || null)} value={milestoneId}>
            <option value="">All milestones</option>
            {(data?.milestones ?? []).map((milestone) => (
              <option key={milestone.id} value={milestone.id}>
                {milestone.name}
              </option>
            ))}
          </SelectField>
          <SegmentedControl
            label="Zoom"
            onChange={(value) => setParameter('zoom', value === 'weeks' ? null : value)}
            options={[
              { value: 'weeks', label: 'Weeks' },
              { value: 'months', label: 'Months' },
              { value: 'quarters', label: 'Quarters' },
            ]}
            value={zoom}
          />
          <Checkbox checked={includeDone} label="Show finished" onChange={(checked) => setParameter('includeDone', checked ? null : 'false')} />
        </div>
        {changing ? (
          <div aria-live="polite" className={`aw-timeline__pending${slipCount ? ' is-slipping' : ''}`} role="status">
            <span>
              Moving <strong>{changedName}</strong> to {changedDay ? dayLabel(changedDay) : '…'}.{' '}
              {slipCount ? `${countLabel(slipCount, 'task slips', 'tasks slip')}: ${[...(slipped?.tasks.keys() ?? [])].map((id) => data?.tasks.find((task) => task.id === id)?.key).filter(Boolean).slice(0, 6).join(', ')}${slipCount > 6 ? '…' : ''}.` : 'Nothing slips.'}
            </span>
            <Button busy={saving} onClick={() => void save()} size="sm" variant="primary">
              Save
            </Button>
            <Button onClick={() => setChange(null)} size="sm" variant="secondary">
              Cancel
            </Button>
          </div>
        ) : null}
        {timeline.isPending ? <ListSkeleton label="Loading the timeline" rows={6} /> : null}
        {timeline.isError ? <ErrorState error={timeline.error} onRetry={() => void timeline.refetch()} title="The timeline didn’t load" /> : null}
        {data && !rows.length ? (
          <EmptyState
            action={<ButtonLink to={`/projects/${encodeURIComponent(projectId)}/milestones`}>Open milestones</ButtonLink>}
            icon="clock"
            title="Nothing dated in this stretch"
          >
            Give tasks a start or due date, or milestones a target date, and they show up here.
          </EmptyState>
        ) : null}
        {data?.truncated ? (
          <Banner className="aw-page__banner" icon="info">
            This shows the first 1,000 tasks. Pick a milestone to see the rest.
          </Banner>
        ) : null}
        {data && rows.length ? (
          <div className="aw-timeline__scroll" ref={scroller} style={{ scrollPaddingLeft: labelWidth + scale.width * 2 }} tabIndex={-1}>
            <div className="aw-timeline__chart" style={{ width: labelWidth + days * scale.width, height: HEADER_HEIGHT + rows.length * ROW_HEIGHT }}>
              <div className="aw-timeline__head" style={{ height: HEADER_HEIGHT }}>
                <span className="aw-timeline__corner" style={{ width: labelWidth }} />
                {months.map((month) => (
                  <span className="aw-timeline__month" key={month.left} style={{ left: month.left, width: month.width }}>
                    <span className="aw-timeline__monthname" style={{ left: labelWidth }}>
                      {month.label}
                    </span>
                  </span>
                ))}
              </div>
              {weeks.map((left) => (
                <span aria-hidden="true" className="aw-timeline__week" key={left} style={{ left, top: HEADER_HEIGHT }} />
              ))}
              <span aria-hidden="true" className="aw-timeline__today" style={{ left: todayX, top: HEADER_HEIGHT - 6 }} title="Today" />
              <svg aria-hidden="true" className="aw-timeline__links" height={rows.length * ROW_HEIGHT} style={{ top: HEADER_HEIGHT }} width={labelWidth + days * scale.width}>
                {[...data.dependencies.map((link) => ({ link, suggested: false })), ...data.suggestedDependencies.map((link) => ({ link, suggested: true }))].map(({ link, suggested }) => {
                  const from = spans.get(link.blockingId)
                  const to = spans.get(link.blockedId)
                  const fromRow = rowIndex.get(link.blockingId)
                  const toRow = rowIndex.get(link.blockedId)
                  if (!from || !to || fromRow === undefined || toRow === undefined) return null
                  const x1 = x(from.end + 1)
                  const y1 = fromRow * ROW_HEIGHT + ROW_HEIGHT / 2
                  const x2 = x(to.start)
                  const y2 = toRow * ROW_HEIGHT + ROW_HEIGHT / 2
                  const bend = Math.max(x1 + 8, Math.min(x2 - 8, (x1 + x2) / 2))
                  return (
                    <path
                      className={`aw-timeline__link${suggested ? ' is-suggested' : ''}${slipped?.tasks.has(link.blockedId) ? ' is-slipping' : ''}`}
                      d={`M${x1},${y1} H${bend} V${y2} H${x2}`}
                      key={`${link.blockingId}-${link.blockedId}-${suggested}`}
                    />
                  )
                })}
              </svg>
              <ul aria-label="Timeline" className="aw-timeline__rows" style={{ top: HEADER_HEIGHT }}>
                {rows.map((row) => {
                  if (row.kind === 'heading') {
                    return (
                      <li className="aw-timeline__row aw-timeline__row--heading" key={`h-${row.id}`} style={{ height: ROW_HEIGHT }}>
                        <span className="aw-timeline__label" style={{ width: labelWidth }}>
                          <span className="aw-timeline__name">{row.name}</span>
                        </span>
                      </li>
                    )
                  }
                  if (row.kind === 'milestone') {
                    const milestone = data.milestones.find((entry) => entry.id === row.id)
                    const target = milestone ? targetWith(milestone, change) : null
                    const late = slipped?.milestones.has(row.id)
                    return (
                      <li className="aw-timeline__row aw-timeline__row--milestone" key={`m-${row.id}`} style={{ height: ROW_HEIGHT }}>
                        <span className="aw-timeline__label" style={{ width: labelWidth }} title={row.name}>
                          <span className="aw-timeline__name">{row.name}</span>
                          {row.targetDate ? <span className="aw-timeline__date">{dayLabel(target === null ? row.targetDate : dayFromNumber(target))}</span> : <span className="aw-timeline__date">No target date</span>}
                        </span>
                        {target !== null ? (
                          <button
                            aria-label={`${row.name}, target ${dayLabel(dayFromNumber(target))}${canEdit ? '. Arrow keys move it, Enter saves.' : ''}`}
                            className={`aw-timeline__diamond${late ? ' is-slipping' : ''}${row.overdue ? ' is-overdue' : ''}`}
                            onKeyDown={(event) => keys(event, (shift) => ({ kind: 'milestone', id: row.id, days: shift }))}
                            onPointerDown={(event) => begin(event, { kind: 'milestone', id: row.id, days: 0 })}
                            onPointerMove={move}
                            onPointerUp={() => end(() => navigate(`/projects/${encodeURIComponent(projectId)}/work-items?milestoneId=${encodeURIComponent(row.id)}`))}
                            style={{ left: x(target) + scale.width / 2 }}
                            type="button"
                          />
                        ) : null}
                      </li>
                    )
                  }
                  const task = row.task
                  const span = spans.get(task.id)
                  const reason = slipped?.tasks.get(task.id)
                  const done = task.statusCategory === 'completed' || task.statusCategory === 'canceled'
                  return (
                    <li className="aw-timeline__row" key={task.id} style={{ height: ROW_HEIGHT }}>
                      <span className="aw-timeline__label" style={{ width: labelWidth }} title={named(task)}>
                        {showKeys ? <span className="aw-timeline__key">{task.key}</span> : null}
                        <span className="aw-timeline__name">{task.title}</span>
                      </span>
                      {span ? (
                        <button
                          aria-label={`${named(task)}, ${dayLabel(dayFromNumber(span.start))} to ${dayLabel(dayFromNumber(span.end))}${reason ? `, slips: ${reason}` : ''}${canEdit ? '. Arrow keys move it, Shift with arrows changes the due date, Enter saves.' : ''}`}
                          className={`aw-timeline__bar${done ? ' is-done' : ''}${task.blocked ? ' is-blocked' : ''}${reason ? ' is-slipping' : ''}`}
                          onKeyDown={(event) => keys(event, (shift, wide) => ({ kind: 'task', id: task.id, days: shift, edge: wide ? 'end' : 'move' }))}
                          onPointerDown={(event) => begin(event, { kind: 'task', id: task.id, days: 0, edge: (event.target as HTMLElement).dataset.edge === 'end' ? 'end' : 'move' })}
                          onPointerMove={move}
                          onPointerUp={() => end(() => navigate(workItemHref(projectId, task.id)))}
                          style={{ left: x(span.start), width: Math.max(scale.width, (span.end - span.start + 1) * scale.width) }}
                          title={reason ?? named(task)}
                          type="button"
                        >
                          {task.assigneeName ? (
                            // Sticks just right of the names, so a bar that starts off-screen still shows who has it.
                            <span className="aw-timeline__bartext" style={{ left: labelWidth + 4 }}>
                              {task.assigneeName}
                            </span>
                          ) : null}
                          {canEdit ? <span aria-hidden="true" className="aw-timeline__handle" data-edge="end" /> : null}
                        </button>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            </div>
          </div>
        ) : null}
        {data && data.undated > 0 ? (
          <p className="aw-timeline__note">
            {countLabel(data.undated, 'task has', 'tasks have')} no dates, so {data.undated === 1 ? 'it isn’t' : 'they aren’t'} shown.
          </p>
        ) : null}
      </div>
    </AppShell>
  )
}
