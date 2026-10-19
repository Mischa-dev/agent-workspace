import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { useProjects } from '../projects/hooks'
import { ButtonLink, EmptyState, ErrorState, ListSkeleton, PageHeader, Section, StatusChip, actorKind, countLabel } from '../ui'
import { workItemHref, workItemsHref } from '../work-items/links'
import { PriorityIcon, StatusIcon } from '../work-items/WorkItemIcons'
import type { WorkItemPriority } from '../work-items/types'
import { getMyWork, viewKeys, type MyWorkSection, type MyWorkTask } from './api'
import { dayLabel } from './dates'
import './views.css'

/** Where "And N more" goes: each project's task list, filtered to that section. */
type MoreFilter = (actorId: string) => Record<string, string>

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

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-mywork">
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
              more={(actor) => ({ assigneeActorId: actor })}
              section={work.data.assigned}
              title="Assigned to you"
            />
            <TaskSection
              description="Tasks you or agents you connected are working on now."
              actorId={actorId}
              empty="Nobody is working on anything for you right now."
              id="my-work-claimed"
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
  section,
  showClaim = false,
  title,
}: {
  actorId: string
  description: string
  empty: string
  id: string
  more: MoreFilter
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
            <TaskRow key={task.id} showClaim={showClaim} task={task} />
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

function TaskRow({ showClaim, task }: { showClaim: boolean; task: MyWorkTask }) {
  return (
    <li className="aw-mywork__row">
      <PriorityIcon decorative priority={task.priority as WorkItemPriority} />
      <StatusIcon category={task.statusCategory} label={task.statusName} />
      <Link className="aw-mywork__title" to={workItemHref(task.projectId, task.id)}>
        <span className="aw-mywork__key">{task.key}</span> {task.title}
      </Link>
      <span className="aw-mywork__meta">
        {task.blocked ? <StatusChip tone="warning">Blocked</StatusChip> : null}
        {showClaim && task.claimedByName ? (
          <StatusChip icon={actorKind(task.claimedByKind ?? 'human') === 'person' ? undefined : 'bot'} tone={actorKind(task.claimedByKind ?? 'human') === 'person' ? 'muted' : 'agent'}>
            {task.claimedByName}
          </StatusChip>
        ) : null}
        <span className="aw-mywork__project">{task.projectName}</span>
        {task.dueDate ? (
          <span className={task.overdue ? 'aw-mywork__due is-overdue' : 'aw-mywork__due'}>
            {task.overdue ? 'Overdue · ' : ''}
            {dayLabel(task.dueDate)}
          </span>
        ) : null}
      </span>
    </li>
  )
}
