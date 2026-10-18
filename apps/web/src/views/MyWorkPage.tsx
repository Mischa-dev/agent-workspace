import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, ListSkeleton, PageHeader, Section, StatusChip } from '../ui'
import { workItemHref } from '../work-items/links'
import { PriorityIcon, StatusIcon } from '../work-items/WorkItemIcons'
import type { WorkItemPriority } from '../work-items/types'
import { getMyWork, viewKeys, type MyWorkSection, type MyWorkTask } from './api'
import { dayLabel } from './dates'
import './views.css'

/**
 * Everything that's yours across every Project: assigned to you, claimed by
 * you or your agents, and waiting for your review.
 */
export function MyWorkPage() {
  const session = useAuthenticatedSession()
  const work = useQuery({ queryKey: viewKeys.myWork, queryFn: getMyWork, refetchInterval: 60_000 })
  const empty = work.data && !work.data.assigned.total && !work.data.claimed.total && !work.data.review.total

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-mywork">
        <PageHeader icon="checklist" title="My work">
          What’s yours in every Project, soonest due first.
        </PageHeader>
        {work.isPending ? <ListSkeleton label="Loading your work" rows={6} /> : null}
        {work.isError ? (
          <ErrorState action={<Button onClick={() => void work.refetch()}>Try again</Button>} title="Your work didn’t load">
            Check your connection and try again.
          </ErrorState>
        ) : null}
        {empty ? (
          <EmptyState icon="circle-check" title="Nothing on your plate">
            Tasks assigned to you, claimed by your agents or waiting for your review show up here.
          </EmptyState>
        ) : null}
        {work.data && !empty ? (
          <>
            <TaskSection
              description="Unfinished tasks assigned to you."
              empty="Nothing is assigned to you."
              id="my-work-assigned"
              section={work.data.assigned}
              title="Assigned to you"
            />
            <TaskSection
              description="Tasks you or agents you connected are working on now."
              empty="Nobody is working on anything for you right now."
              id="my-work-claimed"
              section={work.data.claimed}
              showClaim
              title="Being worked on"
            />
            <TaskSection
              description="In review, and you asked for them or watch them."
              empty="Nothing is waiting for your review."
              id="my-work-review"
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
  description,
  empty,
  id,
  section,
  showClaim = false,
  title,
}: {
  description: string
  empty: string
  id: string
  section: MyWorkSection
  showClaim?: boolean
  title: string
}) {
  const more = section.total - section.items.length
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
      {more > 0 ? <p className="aw-mywork__none">And {more.toLocaleString()} more. Open a Project’s work items to see them all.</p> : null}
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
          <StatusChip icon={task.claimedByKind === 'human' ? undefined : 'bot'} tone={task.claimedByKind === 'human' ? 'muted' : 'agent'}>
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
