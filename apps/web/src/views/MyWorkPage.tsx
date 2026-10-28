import { useCallback, useRef, type MouseEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience } from '../experience/layout'
import { AppShell } from '../shell/AppShell'
import { useProjects } from '../projects/hooks'
import { ButtonLink, EmptyState, ErrorState, ListSkeleton, PageHeader, Section, StatusChip, actorKind, countLabel } from '../ui'
import { useListKeys } from '../ui/listKeys'
import { workItemHref, workItemsHref } from '../work-items/links'
import { useOpenAsYouGo } from '../work-items/shortcutKeys'
import { TaskPanel } from '../work-items/TaskPanel'
import { PriorityIcon, StatusIcon } from '../work-items/WorkItemIcons'
import type { WorkItemPriority } from '../work-items/types'
import { getMyWork, viewKeys, type MyWorkSection, type MyWorkTask } from './api'
import { dayLabel } from './dates'
import './views.css'

/** Where "And N more" goes: each project's task list, filtered to that section. */
type MoreFilter = (actorId: string) => Record<string, string>

type TaskPlace = { id: string; projectId: string }
type OpenTask = (task: TaskPlace, event: MouseEvent<HTMLAnchorElement>) => void

/**
 * Everything that's yours across every project: assigned to you, claimed by
 * you or your agents, and waiting for your review.
 */
export function MyWorkPage() {
  const session = useAuthenticatedSession()
  const work = useQuery({ queryKey: viewKeys.myWork, queryFn: getMyWork, refetchInterval: 60_000 })
  const projects = useProjects()
  const firstProject = (projects.data?.projects ?? []).find((project) => !project.archivedAt)
  const empty = work.data && !work.data.assigned.total && !work.data.claimed.total && !work.data.review.total
  const actorId = session.user.actorId

  // A task opens in the side panel, and the panel is in the address.
  const [params, setParams] = useSearchParams()
  const openTaskId = params.get('task')
  const openProjectId = params.get('project')
  const shown = openTaskId && openProjectId ? { id: openTaskId, projectId: openProjectId } : null
  const showTask = useCallback(
    (task: TaskPlace) => {
      const next = new URLSearchParams(params)
      next.set('task', task.id)
      next.set('project', task.projectId)
      setParams(next, { replace: Boolean(openTaskId) })
    },
    [openTaskId, params, setParams],
  )
  const closeTask = useCallback(() => {
    const next = new URLSearchParams(params)
    next.delete('task')
    next.delete('project')
    setParams(next)
  }, [params, setParams])

  // Down and Up (or J and K) walk the tasks and the panel follows; Escape closes it.
  const pageRef = useRef<HTMLDivElement>(null)
  const following = useOpenAsYouGo((task: TaskPlace) => {
    if (task.id !== openTaskId) showTask(task)
  })
  useListKeys(pageRef, '.aw-mywork__title', {
    onMove: (element) => {
      const { projectId, taskId } = element.dataset
      if (taskId && projectId) following.follow({ id: taskId, projectId })
    },
    onEscape: () => {
      following.cancel()
      if (!shown) return false
      closeTask()
      return true
    },
  })
  const openTask: OpenTask = (task, event) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    event.preventDefault()
    following.cancel()
    showTask(task)
  }
  const panel = shown ? (
    <TaskPanel
      onClose={closeTask}
      onOpenTask={(task) => showTask(task)}
      projectArchived={Boolean(projects.data?.projects.find((project) => project.id === shown.projectId)?.archivedAt)}
      projectId={shown.projectId}
      taskId={shown.id}
    />
  ) : undefined

  return (
    <AppShell context={panel} contextKey={shown?.id} home mainClassName="aw-main--page" onContextDismiss={closeTask} user={session.user}>
      <div className="aw-page aw-mywork" ref={pageRef}>
        <PageHeader icon="checklist" title="My work">
          What’s yours in every project, soonest due first.
        </PageHeader>
        {work.isPending ? <ListSkeleton label="Loading your work" rows={6} /> : null}
        {work.isError ? <ErrorState error={work.error} onRetry={() => void work.refetch()} title="Your work didn’t load" /> : null}
        {empty ? (
          <EmptyState
            action={firstProject ? <ButtonLink to={workItemsHref(firstProject.id)}>Open {firstProject.name} tasks</ButtonLink> : undefined}
            icon="circle-check"
            title="Nothing on your plate"
          >
            Tasks assigned to you, claimed by your agents or waiting for your review show up here.
          </EmptyState>
        ) : null}
        {work.data && !empty ? (
          <>
            <TaskSection
              description="Unfinished tasks assigned to you."
              actorId={actorId}
              empty="Nothing is assigned to you."
              id="my-work-assigned"
              onOpen={openTask}
              openTaskId={shown?.id ?? null}
              more={(actor) => ({ assigneeActorId: actor })}
              section={work.data.assigned}
              title="Assigned to you"
            />
            <TaskSection
              description="Tasks you or agents you connected are working on now."
              actorId={actorId}
              empty="Nobody is working on anything for you right now."
              id="my-work-claimed"
              onOpen={openTask}
              openTaskId={shown?.id ?? null}
              more={() => ({ claimed: 'true' })}
              section={work.data.claimed}
              showClaim
              title="Being worked on"
            />
            <TaskSection
              description="In review, and you asked for them or watch them."
              actorId={actorId}
              empty="Nothing is waiting for your review."
              id="my-work-review"
              onOpen={openTask}
              openTaskId={shown?.id ?? null}
              more={() => ({ semanticCategory: 'review' })}
              section={work.data.review}
              title="Waiting for your review"
            />
          </>
        ) : null}
      </div>
    </AppShell>
  )
}

function TaskSection({
  actorId,
  description,
  empty,
  id,
  more: moreFilter,
  onOpen,
  openTaskId,
  section,
  showClaim = false,
  title,
}: {
  actorId: string
  description: string
  empty: string
  id: string
  more: MoreFilter
  onOpen: OpenTask
  openTaskId: string | null
  section: MyWorkSection
  showClaim?: boolean
  title: string
}) {
  const more = section.total - section.items.length
  // The projects the shown rows come from; the rest are most likely there too.
  const projects = [...new Map(section.items.map((task) => [task.projectId, task.projectName])).entries()]
  const moreHref = (projectId: string) => `${workItemsHref(projectId)}?${new URLSearchParams(moreFilter(actorId)).toString()}`
  return (
    <Section description={description} id={id} title={section.total ? `${title} · ${section.total}` : title}>
      {section.items.length ? (
        <ul aria-labelledby={id} className="aw-mywork__list">
          {section.items.map((task) => (
            <TaskRow key={task.id} onOpen={onOpen} open={task.id === openTaskId} showClaim={showClaim} task={task} />
          ))}
        </ul>
      ) : (
        <p className="aw-mywork__none">{empty}</p>
      )}
      {more > 0 ? (
        <p className="aw-mywork__none">
          And {countLabel(more, 'more task', 'more tasks')}. See them all in{' '}
          {projects.map(([projectId, name], index) => (
            <span key={projectId}>
              {index ? ', ' : ''}
              <Link to={moreHref(projectId)}>{name}</Link>
            </span>
          ))}
          .
        </p>
      ) : null}
    </Section>
  )
}

function TaskRow({ onOpen, open, showClaim, task }: { onOpen: OpenTask; open: boolean; showClaim: boolean; task: MyWorkTask }) {
  // Each task follows its own project's layout.
  const experience = useProjectExperience(task.projectId)
  return (
    <li className={open ? 'aw-mywork__row is-open' : 'aw-mywork__row'}>
      {experience.shows('task.priority') ? (
        <PriorityIcon decorative priority={task.priority as WorkItemPriority} />
      ) : (
        <span aria-hidden="true" className="aw-mywork__icon" />
      )}
      <StatusIcon category={task.statusCategory} label={task.statusName} />
      <Link
        aria-current={open || undefined}
        className="aw-mywork__title"
        data-project-id={task.projectId}
        data-task-id={task.id}
        onClick={(event) => onOpen(task, event)}
        to={workItemHref(task.projectId, task.id)}
      >
        {experience.shows('tech.task_keys') ? <span className="aw-mywork__key">{task.key}</span> : null} {task.title}
      </Link>
      <span className="aw-mywork__meta">
        {task.blocked ? <StatusChip tone="warning">Blocked</StatusChip> : null}
        {showClaim && task.claimedByName ? (
          <StatusChip icon={actorKind(task.claimedByKind ?? 'human') === 'person' ? undefined : 'bot'} tone={actorKind(task.claimedByKind ?? 'human') === 'person' ? 'muted' : 'agent'}>
            {task.claimedByName}
          </StatusChip>
        ) : null}
        <span className="aw-mywork__project">{task.projectName}</span>
        {task.dueDate && experience.shows('task.dates') ? (
          <span className={task.overdue ? 'aw-mywork__due is-overdue' : 'aw-mywork__due'}>
            {task.overdue ? 'Overdue · ' : ''}
            {dayLabel(task.dueDate)}
          </span>
        ) : null}
      </span>
    </li>
  )
}
